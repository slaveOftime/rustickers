//! Cross-platform clipboard watcher.
//!
//! A single observer thread polls the system clipboard at a low frequency. Each new text the
//! clipboard has not seen before is forwarded through a shared [`CaptureFilter`] which is
//! responsible for:
//!
//! 1. Suppressing writes that the application itself produced (sentinel markers, restored
//!    selections, password manager reveals).
//! 2. Dropping large oversized captures.
//! 3. Forwarding the accepted payload to the persistence layer.
//!
//! The [`CaptureFilter`] is a single shared object that lives inside the native event loop so
//! GUI-mediated writes can mark themselves as internal before they reach the clipboard. This is
//! how the existing selection-capture helper ([`crate::native::selection`]) avoids polluting the
//! history with the temporary marker it sets on the clipboard.
//!
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::clipboard::settings::ClipboardSettings;
use crate::text::TextRepository;

/// Per-capture flag — text the system clipboard just received.
#[derive(Debug, Clone)]
pub struct CapturedText {
    pub content: String,
    pub byte_size: i64,
    /// Source annotation so future macros (Cmd+C, OS side panel, password manager, …) can be
    /// recorded without affecting storage shape.
    pub source: CaptureSource,
}

/// Where the captured text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureSource {
    /// A normal "user copied something" event.
    UserCopy,
    /// A write performed by the application itself, suppressed trivially.
    Internal,
    /// A write acknowledged by another trustworthy source (e.g. cmd-line `--import`).
    TrustedImport,
}

/// Surface that callers can use to control which writes the history collector accepts.
///
/// The canonical instance lives in the GUI bootstrap. The CLI never instantiates one, but it
/// does open the same store when it needs to record a `--import` so the dedupe rules apply
/// consistently.
#[derive(Default)]
pub struct CaptureFilter {
    inner: Mutex<CaptureFilterState>,
}

#[derive(Default)]
struct CaptureFilterState {
    internal_writes_this_tick: u64,
    internal_generation: Option<u32>,
}

impl CaptureFilter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Mark that the caller is about to write to the clipboard. The watcher skips the next poll
    /// that would otherwise see *that* write as a fresh copy.
    pub fn note_internal_write(&self) {
        let mut state = self.inner.lock().expect("CaptureFilter mutex poisoned");
        state.internal_writes_this_tick = state.internal_writes_this_tick.saturating_add(1);
    }

    pub fn transaction(&self) -> std::sync::MutexGuard<'_, ()> {
        static TRANSACTION: Mutex<()> = Mutex::new(());
        TRANSACTION.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn record_internal_generation(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .internal_generation = clipboard_generation();
    }
    fn is_internal_generation(&self, generation: Option<u32>) -> bool {
        generation.is_some()
            && self
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .internal_generation
                == generation
    }

    /// Returns the number of internal writes observed since the last call. Watcher drains the
    /// counter alongside this check.
    pub fn consume_pending_internal_writes(&self) -> u64 {
        let mut state = self.inner.lock().expect("CaptureFilter mutex poisoned");
        let pending = state.internal_writes_this_tick;
        state.internal_writes_this_tick = 0;
        pending
    }
}

/// Detector: don't double-write if the clipboard hasn't changed since the last poll.
#[derive(Debug, Clone, Default)]
pub struct LastSeenText(pub Option<String>);

impl LastSeenText {
    pub fn new() -> Self {
        Self(None)
    }

    /// Returns true if `text` is something we should record (i.e. distinct from prior). Always
    /// updates the buffered value.
    pub fn accept(&mut self, text: &str) -> bool {
        let changed = self.0.as_deref() != Some(text);
        self.0 = Some(text.to_owned());
        changed
    }
}

/// Persist [`CapturedText`] into the provided store. Returns the row id and outcome so the caller
/// can update the GUI's chooser / settings in real time.
pub async fn persist_capture(
    store: &TextRepository,
    captured: &CapturedText,
    settings: &ClipboardSettings,
) -> anyhow::Result<Option<i64>> {
    if !settings.capture_enabled
        || captured.source != CaptureSource::UserCopy
        || captured.content.trim().is_empty()
    {
        return Ok(None);
    }
    Ok(Some(
        store
            .capture(&captured.content, crate::utils::time::now_unix_millis())
            .await?,
    ))
}

/// Drives the watcher loop. Designed for parking on a background thread; the GUI owns the
/// `CaptureFilter` and the shutdown signal.
pub struct WatcherHandle {
    stop: Arc<std::sync::atomic::AtomicBool>,
    poll_interval: Duration,
}

impl WatcherHandle {
    pub fn new(poll_interval: Duration) -> (Self, Arc<std::sync::atomic::AtomicBool>) {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        (
            Self {
                stop: stop.clone(),
                poll_interval,
            },
            stop,
        )
    }

    /// Spawn the polling thread. It writes accepted captures into the provided store
    /// and pushes a content-free [`WatcherSignal`] to a global queue whenever the
    /// visible clipboard state changes (a new row arrived; the collection paused; a
    /// capture error). Subscribers -- such as the chooser snapshot loop -- drain
    /// that queue on their own cadence. Returns the join handle so callers can stop
    /// the watcher before the process exits.
    ///
    /// Note: the watcher no longer takes an mpsc Sender of `CapturedText`. The
    /// previous channel carried content the chooser already has access to, kept
    /// aliases inside `Arc<Mutex<Vec<CapturedText>>>` if the receiver was dropped,
    /// and could outlive the picker on the way to a never-read endpoint.
    /// Replacing it with a content-free signal avoids the leak surface and keeps
    /// the chooser's snapshot read on its own schedule.
    pub fn spawn(
        self,
        filter: Arc<CaptureFilter>,
        store: Arc<TextRepository>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::Builder::new().name("clipboard-watcher".into()).spawn(move || {
            let mut last_seen = LastSeenText::new();
            let mut last_generation = None;
            let mut active = false;
            while !self.stop.load(std::sync::atomic::Ordering::Relaxed) {
                let tick = (|| -> anyhow::Result<()> {
                    let settings = futures::executor::block_on(store.settings())?;
                    if !settings.capture_enabled {
                        if active { push_watcher_signal(WatcherSignal::CapturePaused); }
                        active = false;
                        set_capture_status("Collection paused. Enable to capture copied text (including secrets).");
                        return Ok(());
                    }
                    if !active { last_seen = LastSeenText::new(); last_generation = None; active = true; }
                    let transaction = filter.transaction();
                    let before = clipboard_generation();
                    let text = read_clipboard_text()?;
                    let generation = clipboard_generation();
                    if before != generation { return Ok(()); } // racing external writer; retry next tick
                    let internal = filter.is_internal_generation(generation);
                    let fallback_internal = filter.consume_pending_internal_writes() > 0;
                    if internal || (generation.is_none() && fallback_internal) {
                        if let Some(text) = text { last_seen.accept(&text); }
                        last_generation = generation; return Ok(());
                    }
                    let changed_generation = generation.is_some() && generation != last_generation;
                    last_generation = generation;
                    drop(transaction);
                    if let Some(text) = text {
                        let changed_text = last_seen.accept(&text);
                        if changed_generation || changed_text {
                            if text.len() > 1024 * 1024 {
                                set_capture_status("Capture skipped: text exceeds 1 MiB; nothing was truncated."); return Ok(());
                            }
                            let captured = CapturedText { byte_size: text.len() as i64, content: text, source: CaptureSource::UserCopy };
                            futures::executor::block_on(persist_capture(&store, &captured, &settings))?;
                            push_watcher_signal(WatcherSignal::Captured);
                        }
                    } else { last_seen = LastSeenText::new(); }
                    set_capture_status("Collecting copied text. Secrets can enter ordinary history.");
                    Ok(())
                })();
                if tick.is_err() { last_seen = LastSeenText::new(); last_generation = None; set_capture_status("Collection failed: clipboard busy, permission denied, or storage unavailable. Retrying."); }
                std::thread::sleep(self.poll_interval);
            }
        }).expect("spawn clipboard watcher")
    }
}

/// Synchronous, best-effort read of the host clipboard text. Returns `Ok(None)` on platforms and
/// configurations where the selection is empty, non-text, or unavailable.
pub fn read_clipboard_text() -> anyhow::Result<Option<String>> {
    let mut clipboard = arboard::Clipboard::new()?;
    match clipboard.get_text() {
        Ok(text) => Ok(Some(text)),
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        Err(err) => Err(anyhow::anyhow!("clipboard read failed: {err}")),
    }
}

/// Synchronous write helper used both by the selection helper and the imperceptible "GUI typed
/// `Ctrl+V` onto an item" path. Always pairs the write with a
/// [`CaptureFilter::note_internal_write`] so the watcher won't re-ingest it.
pub fn write_clipboard_text(filter: &CaptureFilter, text: &str) -> anyhow::Result<()> {
    write_clipboard_text_owned(filter, text).map(|_| ())
}

/// Return the generation sampled while holding the native clipboard open, not after release.
pub fn write_clipboard_text_owned(
    filter: &CaptureFilter,
    text: &str,
) -> anyhow::Result<Option<u32>> {
    let _transaction = filter.transaction();
    anyhow::ensure!(
        !text.contains('\0'),
        "Clipboard text contains NUL; cannot paste it without truncation"
    );
    #[cfg(target_os = "windows")]
    {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn OpenClipboard(hwnd: *mut core::ffi::c_void) -> i32;
            fn CloseClipboard() -> i32;
            fn EmptyClipboard() -> i32;
            fn SetClipboardData(
                format: u32,
                memory: *mut core::ffi::c_void,
            ) -> *mut core::ffi::c_void;
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GlobalAlloc(flags: u32, bytes: usize) -> *mut core::ffi::c_void;
            fn GlobalLock(memory: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
            fn GlobalUnlock(memory: *mut core::ffi::c_void) -> i32;
            fn GlobalFree(memory: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
        }
        // Allocate before clearing so allocation failure leaves the previous clipboard intact.
        let utf16 = zeroize::Zeroizing::new(
            text.encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>(),
        );
        let memory = unsafe { GlobalAlloc(2, utf16.len() * 2) }; // GMEM_MOVEABLE
        anyhow::ensure!(!memory.is_null(), "Clipboard allocation failed");
        struct Memory(*mut core::ffi::c_void);
        impl Drop for Memory {
            fn drop(&mut self) {
                unsafe {
                    GlobalFree(self.0);
                }
            }
        }
        let allocation = Memory(memory);
        let pointer = unsafe { GlobalLock(memory) };
        anyhow::ensure!(!pointer.is_null(), "Cannot lock clipboard allocation");
        unsafe {
            std::ptr::copy_nonoverlapping(utf16.as_ptr(), pointer.cast::<u16>(), utf16.len());
            GlobalUnlock(memory);
        }
        // Use a live window owner; never hold the clipboard open while waiting for keyboard input.
        let owner = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
        let mut opened = false;
        for _ in 0..5 {
            if unsafe { OpenClipboard(owner) } != 0 {
                opened = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        anyhow::ensure!(opened, "Clipboard busy; retry Copy only");
        struct Close;
        impl Drop for Close {
            fn drop(&mut self) {
                unsafe {
                    CloseClipboard();
                }
            }
        }
        let _close = Close;
        anyhow::ensure!(unsafe { EmptyClipboard() } != 0, "Cannot prepare clipboard");
        anyhow::ensure!(
            !unsafe { SetClipboardData(13, memory) }.is_null(),
            "Cannot write Unicode clipboard text"
        );
        std::mem::forget(allocation); // OS now owns the HGLOBAL.
        filter.note_internal_write();
        filter.record_internal_generation();
        Ok(clipboard_generation())
    }
    #[cfg(not(target_os = "windows"))]
    {
        filter.note_internal_write();
        let mut clipboard = arboard::Clipboard::new()?;
        clipboard.set_text(text.to_owned())?;
        Ok(None)
    }
}

/// Restore the last captured clipboard value, bypassing the watcher via the suppression hook.
pub fn restore_clipboard_text(
    filter: &CaptureFilter,
    previous: Option<&str>,
) -> anyhow::Result<()> {
    let _transaction = filter.transaction();
    filter.note_internal_write();
    let mut clipboard = arboard::Clipboard::new()?;
    match previous {
        Some(text) => clipboard.set_text(text.to_owned())?,
        None => clipboard.clear()?,
    };
    filter.record_internal_generation();
    Ok(())
}

static CAPTURE_STATUS: std::sync::OnceLock<Mutex<String>> = std::sync::OnceLock::new();
pub fn capture_status() -> String {
    CAPTURE_STATUS
        .get_or_init(|| Mutex::new("Collector starting…".into()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}
fn set_capture_status(status: &str) {
    *CAPTURE_STATUS
        .get_or_init(|| Mutex::new(String::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = status.into();
}

/// Content-free signals the watcher publishes onto a global queue when
/// the visible state changes. Carries no payload: subscribers run a
/// snapshot at their own cadence and inspect the row themselves. This
/// avoids both the watchdog leak of the previous `mpsc::Sender<CapturedText>`
/// (whose receiver was dropped) and the temptation to mirror plaintext
/// through a long-lived channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatcherSignal {
    /// A new ordinary capture was persisted to the store.
    Captured,
    /// `has_protected_items()` flipped to true (or to false). The chooser
    /// should refresh; collection is now or is no longer paused on a
    /// safety ground.
    CapturePaused,
    /// A tick failed and the watcher is retrying; the chooser keeps its
    /// last good state but should surface a recoverable status.
    CaptureError,
}

static WATCHER_SIGNALS: OnceLock<Mutex<VecDeque<WatcherSignal>>> = OnceLock::new();

fn signals() -> &'static Mutex<VecDeque<WatcherSignal>> {
    WATCHER_SIGNALS.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// Push a content-free signal from the watcher thread. Coalescing is
/// left to the subscriber: multiple `Captured` events arriving while a
/// subscriber is busy collapse into a single UI refresh because the
/// subscriber drains the entire queue and the existence query inside
/// the chooser sees the latest storage state.
fn push_watcher_signal(signal: WatcherSignal) {
    if let Ok(mut queue) = signals().lock() {
        queue.push_back(signal);
    }
}

/// Drain all currently-queued watcher signals. Returns the drained list
/// so callers can collapse multiple `Captured` (or `CapturePaused`)
/// events into one UI update.
pub fn drain_watcher_signals() -> Vec<WatcherSignal> {
    match signals().lock() {
        Ok(mut queue) => queue.drain(..).collect(),
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            guard.drain(..).collect()
        }
    }
}

/// Number of unread signals. Cheap; intended for tests that want to
/// confirm an event made it onto the queue.
pub fn pending_watcher_signals() -> usize {
    signals().lock().map(|q| q.len()).unwrap_or(0)
}
#[cfg(target_os = "windows")]
pub fn clipboard_generation() -> Option<u32> {
    // Signature verified against the locally cached windows-sys DataExchange bindings.
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetClipboardSequenceNumber() -> u32;
    }
    Some(unsafe { GetClipboardSequenceNumber() })
}
#[cfg(not(target_os = "windows"))]
pub fn clipboard_generation() -> Option<u32> {
    None
}

/// Clear protected plaintext only if no intervening owner/generation has changed it.
pub fn clear_protected_if_owned(
    filter: &CaptureFilter,
    generation: Option<u32>,
) -> anyhow::Result<()> {
    let _transaction = filter.transaction();
    #[cfg(target_os = "windows")]
    {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn OpenClipboard(hwnd: *mut core::ffi::c_void) -> i32;
            fn CloseClipboard() -> i32;
            fn EmptyClipboard() -> i32;
        }
        anyhow::ensure!(generation.is_some(), "Clipboard ownership unavailable");
        // OpenClipboard excludes concurrent external writers during check + clear.
        anyhow::ensure!(
            unsafe { OpenClipboard(std::ptr::null_mut()) } != 0,
            "Clipboard busy"
        );
        struct Close;
        impl Drop for Close {
            fn drop(&mut self) {
                unsafe {
                    CloseClipboard();
                }
            }
        }
        let _close = Close;
        if clipboard_generation() == generation {
            anyhow::ensure!(unsafe { EmptyClipboard() } != 0, "Cannot clear clipboard");
            filter.note_internal_write();
            filter.record_internal_generation();
        }
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = generation;
        anyhow::bail!("Ownership-aware clipboard clearing unavailable");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_seen_skips_repeated_text() {
        let mut last = LastSeenText::new();
        assert!(last.accept("first"));
        assert!(!last.accept("first"));
        assert!(last.accept("second"));
    }

    #[test]
    fn internal_write_marker_drains_on_consume() {
        let filter = CaptureFilter::new();
        filter.note_internal_write();
        filter.note_internal_write();
        assert_eq!(filter.consume_pending_internal_writes(), 2);
        assert_eq!(filter.consume_pending_internal_writes(), 0);
    }

    /// Many `Captured` events between two `drain` calls collapse into a
    /// single render in the chooser: draining returns every queued event,
    /// but `pending_watcher_signals` reports the post-drain depth so a
    /// later caller can see whether anything was lost.
    #[test]
    fn watcher_signal_queue_preserves_each_push_and_drains_atomically() {
        // Drain anything left over from earlier tests in this process.
        let _ = drain_watcher_signals();

        push_watcher_signal(WatcherSignal::Captured);
        push_watcher_signal(WatcherSignal::Captured);
        push_watcher_signal(WatcherSignal::CapturePaused);

        assert_eq!(pending_watcher_signals(), 3);
        let drained = drain_watcher_signals();
        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0], WatcherSignal::Captured);
        assert_eq!(drained[1], WatcherSignal::Captured);
        assert_eq!(drained[2], WatcherSignal::CapturePaused);
        assert_eq!(pending_watcher_signals(), 0);
    }
}
