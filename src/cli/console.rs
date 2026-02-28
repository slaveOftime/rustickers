/// On Windows, attach to the parent process console so that output is visible
/// when the binary is invoked from a terminal.
pub fn setup() {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        unsafe {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

/// Returns a writer that reaches the console.
///
/// On Windows, `CONOUT$` is opened directly because stdout may not be
/// connected after `AttachConsole`.
pub fn console_writer() -> Box<dyn std::io::Write> {
    #[cfg(target_os = "windows")]
    {
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
            return Box::new(f);
        }
    }
    Box::new(std::io::stdout())
}

pub fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}

pub fn format_ts(ts: i64) -> String {
    use chrono::{DateTime, Utc};
    DateTime::from_timestamp_millis(ts)
        .map(|dt: DateTime<Utc>| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| ts.to_string())
}

pub fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

pub fn signal_open(id: i64, out: &mut dyn std::io::Write) {
    match crate::ipc::send_ipc_command("rustickers", &format!("OPEN_STICKER {id}")) {
        Ok(true) => {
            let _ = writeln!(out, "Signaled running Rustickers to open the sticker.");
        }
        Ok(false) => {
            let _ = writeln!(
                out,
                "Rustickers is not running — sticker will open on next launch."
            );
        }
        Err(err) => {
            let _ = writeln!(out, "Note: could not signal running instance: {err}");
        }
    }
}
