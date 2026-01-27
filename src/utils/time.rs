use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local, Utc};

pub fn now_unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn secs_to_hms(secs: i64) -> (i64, i64, i64) {
    let secs = secs.max(0);
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    (h, m, s)
}

pub fn format_unix_millis(ms: i64) -> String {
    if ms <= 0 {
        return "".to_string();
    }

    let Some(dt_utc) = DateTime::<Utc>::from_timestamp_millis(ms) else {
        return "".to_string();
    };
    let dt_local: DateTime<Local> = dt_utc.into();
    dt_local.format("%Y-%m-%d %H:%M").to_string()
}
