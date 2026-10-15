//! Read text selections without sending Copy (which interrupts idle terminals).
//!
//! UI Automation providers are out-of-process and may hang. Keep all COM objects
//! on one dedicated MTA thread, bound the caller's wait, and never queue probes
//! behind a stalled provider. Unknown is not the same as an empty selection.
use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    time::Duration,
};
use windows::{
    Win32::{
        Foundation::HWND,
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
            CoUninitialize,
        },
        UI::{
            Accessibility::{
                CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationElement,
                IUIAutomationTextPattern, IUIAutomationTreeWalker, SupportedTextSelection_Multiple,
                SupportedTextSelection_Single, TextPatternRangeEndpoint_End,
                TextPatternRangeEndpoint_Start, UIA_EditControlTypeId, UIA_TextPatternId,
            },
            WindowsAndMessaging::{
                GA_ROOT, GetAncestor, GetForegroundWindow, GetWindowThreadProcessId,
            },
        },
    },
    core::Interface,
};

const PROBE_TIMEOUT: Duration = Duration::from_millis(150);
static PROBE: OnceLock<Option<SelectionProbe>> = OnceLock::new();

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SelectionRead {
    Unavailable,
    Empty,
    Text(String),
}
struct Request {
    source: u64,
    reply: SyncSender<SelectionRead>,
}
struct SelectionProbe {
    sender: SyncSender<Request>,
    busy: Arc<AtomicBool>,
}
impl SelectionProbe {
    fn start(
        initialize: impl FnOnce() -> Box<dyn FnMut(u64) -> SelectionRead> + Send + 'static,
    ) -> std::io::Result<Self> {
        let (sender, requests) = mpsc::sync_channel::<Request>(1);
        let busy = Arc::new(AtomicBool::new(false));
        let running = busy.clone();
        std::thread::Builder::new()
            .name("selection-accessibility".into())
            .spawn(move || {
                let mut read = initialize();
                for request in requests {
                    let result = read(request.source);
                    // A timed-out caller drops its receiver; never reuse that
                    // result for a later foreground window or request.
                    running.store(false, Ordering::Release);
                    let _ = request.reply.try_send(result);
                }
            })?;
        Ok(Self { sender, busy })
    }

    fn read(&self, source: u64, timeout: Duration) -> SelectionRead {
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return SelectionRead::Unavailable;
        }
        let (reply, response) = mpsc::sync_channel(1);
        if self.sender.try_send(Request { source, reply }).is_err() {
            self.busy.store(false, Ordering::Release);
            return SelectionRead::Unavailable;
        }
        response
            .recv_timeout(timeout)
            .unwrap_or(SelectionRead::Unavailable)
    }
}

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}
struct Automation {
    // Fields drop in declaration order: release interfaces before the apartment.
    client: IUIAutomation,
    _apartment: Apartment,
}
impl Automation {
    fn new() -> windows::core::Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
        let apartment = Apartment;
        let client: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }?;
        let timeouts: IUIAutomation2 = client.cast()?;
        unsafe {
            timeouts.SetConnectionTimeout(100)?;
            timeouts.SetTransactionTimeout(100)?;
        }
        Ok(Self {
            client,
            _apartment: apartment,
        })
    }

    fn read(&self, source: u64) -> windows::core::Result<SelectionRead> {
        let hwnd = HWND(source as *mut core::ffi::c_void);
        if unsafe { GetForegroundWindow() } != hwnd {
            return Ok(SelectionRead::Unavailable);
        }
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == 0 {
            return Ok(SelectionRead::Unavailable);
        }
        let focused = unsafe { self.client.GetFocusedElement() }?;
        if unsafe { focused.CurrentProcessId() }? as u32 != pid {
            return Ok(SelectionRead::Unavailable);
        }
        let walker = unsafe { self.client.ControlViewWalker() }?;
        // Several browser windows share a PID, and UIA focus updates can lag a
        // native foreground switch. Reject another window's cached selection.
        if !belongs_to_window(&walker, &focused, hwnd)? {
            return Ok(SelectionRead::Unavailable);
        }
        let mut element = focused.clone();
        let mut result = SelectionRead::Unavailable;
        // Browser focus may be on a link/button inside the document that owns
        // TextPattern. Do not walk beyond the source process or into the desktop.
        for _ in 0..12 {
            if unsafe { element.CurrentProcessId() }? as u32 != pid {
                break;
            }
            if unsafe { element.CurrentIsPassword() }?.as_bool() {
                result = SelectionRead::Empty;
                break;
            }
            if let Ok(pattern) = unsafe {
                element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            } {
                result = read_pattern(&pattern)?;
                break;
            }
            // An unsupported editable field's selection is not represented by
            // its parent document. Do not misclassify it from an empty parent.
            if unsafe { element.CurrentControlType() }? == UIA_EditControlTypeId {
                break;
            }
            let Ok(parent) = (unsafe { walker.GetParentElement(&element) }) else {
                break;
            };
            element = parent;
        }
        // Ignore a provider result if the user changed the source or focused
        // control while its RPC was in flight. Never capture from another app.
        if unsafe { GetForegroundWindow() } != hwnd
            || !unsafe {
                self.client
                    .CompareElements(&focused, &self.client.GetFocusedElement()?)
            }?
            .as_bool()
        {
            return Ok(SelectionRead::Unavailable);
        }
        Ok(result)
    }
}
fn belongs_to_window(
    walker: &IUIAutomationTreeWalker,
    focused: &IUIAutomationElement,
    source: HWND,
) -> windows::core::Result<bool> {
    let mut element = focused.clone();
    for _ in 0..12 {
        let hwnd = unsafe { element.CurrentNativeWindowHandle() }?;
        if !hwnd.0.is_null() {
            return Ok(unsafe { GetAncestor(hwnd, GA_ROOT) } == source);
        }
        let Ok(parent) = (unsafe { walker.GetParentElement(&element) }) else {
            break;
        };
        element = parent;
    }
    Ok(false)
}

fn read_pattern(pattern: &IUIAutomationTextPattern) -> windows::core::Result<SelectionRead> {
    let support = unsafe { pattern.SupportedTextSelection() }?;
    if support != SupportedTextSelection_Single && support != SupportedTextSelection_Multiple {
        return Ok(SelectionRead::Unavailable);
    }
    let ranges = unsafe { pattern.GetSelection() }?;
    let count = unsafe { ranges.Length() }?;
    let mut parts = Vec::new();
    for ix in 0..count {
        let range = unsafe { ranges.GetElement(ix) }?;
        if unsafe {
            range.CompareEndpoints(
                TextPatternRangeEndpoint_Start,
                &range,
                TextPatternRangeEndpoint_End,
            )
        }? != 0
        {
            parts.push(unsafe { range.GetText(-1) }?.to_string());
        }
    }
    // TextPattern returns no ranges or caret-only ranges when nothing is
    // selected. Never use DocumentRange/GetText to substitute the whole document.
    let text = parts.join("\n");
    Ok(if text.trim().is_empty() {
        SelectionRead::Empty
    } else {
        SelectionRead::Text(text)
    })
}

fn probe() -> &'static Option<SelectionProbe> {
    PROBE.get_or_init(|| {
        SelectionProbe::start(|| {
            let automation = Automation::new().ok();
            Box::new(move |source| {
                automation
                    .as_ref()
                    .and_then(|client| {
                        let result = client.read(source);
                        if let Err(error) = &result {
                            tracing::debug!(%error, "Accessibility selection unavailable; using safe fallback");
                        }
                        result.ok()
                    })
                    .unwrap_or(SelectionRead::Unavailable)
            })
        })
        .ok()
    })
}
pub(super) fn initialize() {
    let _ = probe();
}
pub(super) fn read(source: u64) -> SelectionRead {
    probe()
        .as_ref()
        .map(|probe| probe.read(source, PROBE_TIMEOUT))
        .unwrap_or(SelectionRead::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn provider_results_preserve_request_identity() {
        let probe =
            SelectionProbe::start(|| Box::new(|source| SelectionRead::Text(source.to_string())))
                .unwrap();
        assert_eq!(
            probe.read(12, Duration::from_secs(1)),
            SelectionRead::Text("12".into())
        );
        assert_eq!(
            probe.read(42, Duration::from_secs(1)),
            SelectionRead::Text("42".into())
        );
    }

    #[test]
    fn a_stalled_provider_cannot_block_or_queue_later_requests() {
        let (release, released) = mpsc::sync_channel(1);
        let probe = SelectionProbe::start(move || {
            let mut first = true;
            Box::new(move |source| {
                if first {
                    released.recv().unwrap();
                    first = false;
                }
                SelectionRead::Text(source.to_string())
            })
        })
        .unwrap();
        assert_eq!(
            probe.read(12, Duration::from_millis(20)),
            SelectionRead::Unavailable
        );
        let started = Instant::now();
        assert_eq!(
            probe.read(42, Duration::from_secs(1)),
            SelectionRead::Unavailable
        );
        assert!(started.elapsed() < Duration::from_millis(50));
        release.send(()).unwrap();
        while probe.busy.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        assert_eq!(
            probe.read(42, Duration::from_secs(1)),
            SelectionRead::Text("42".into()),
            "a timed-out result must not be reused for the next source"
        );
    }
}
