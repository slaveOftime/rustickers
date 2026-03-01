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

pub fn signal_open(id: i64) {
    match crate::ipc::send_ipc_command("rustickers", &format!("OPEN_STICKER {id}")) {
        Ok(true) => {
            println!("Signaled running Rustickers to open the sticker.");
        }
        Ok(false) => {
            println!("Rustickers is not running — sticker will open on next launch.");
        }
        Err(err) => {
            println!("Note: could not signal running instance: {err}");
        }
    }
}
