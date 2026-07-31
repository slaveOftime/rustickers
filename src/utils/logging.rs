use crate::storage::paths::AppPaths;

use anyhow::Context as _;
use std::{
    fs::OpenOptions,
    io::Write as _,
    path::{Path, PathBuf},
};
use tracing_subscriber::prelude::*;

pub struct LoggingGuards {
    _file: tracing_appender::non_blocking::WorkerGuard,
}

impl LoggingGuards {
    pub fn init(app_paths: &AppPaths) -> anyhow::Result<Self> {
        let rustickers_log_value = std::env::var("RUSTICKERS_LOG").ok();
        let rust_log_value = std::env::var("RUST_LOG").ok();

        // Log level precedence:
        // 1) RUSTICKERS_LOG
        // 2) RUST_LOG
        // 3) debug (development) / info (release)
        let (env_filter, filter_source, filter_parse_error) =
            match tracing_subscriber::EnvFilter::try_from_env("RUSTICKERS_LOG") {
                Ok(filter) => (filter, "RUSTICKERS_LOG", None),
                Err(err) => match tracing_subscriber::EnvFilter::try_from_default_env() {
                    Ok(filter) => {
                        let parse_error = rustickers_log_value.as_ref().map(|_| err.to_string());
                        (filter, "RUST_LOG", parse_error)
                    }
                    Err(_) => {
                        let parse_error = rustickers_log_value.as_ref().map(|_| err.to_string());
                        let fallback = if cfg!(debug_assertions) {
                            "debug"
                        } else {
                            "info"
                        };
                        (
                            tracing_subscriber::EnvFilter::new(fallback),
                            "fallback",
                            parse_error,
                        )
                    }
                },
            };
        let env_filter_str = env_filter.to_string();

        // Always log to file (important for Windows GUI builds).
        let log_dir = app_paths.log_dir();
        std::fs::create_dir_all(&log_dir).context("create log directory")?;
        let file_appender = tracing_appender::rolling::daily(&log_dir, "rustickers.log");
        let (file_writer, file_guard) = tracing_appender::non_blocking(file_appender);

        let file_layer = tracing_subscriber::fmt::layer()
            .with_writer(file_writer)
            .with_ansi(false)
            .with_target(true)
            .with_thread_names(true)
            .with_thread_ids(true)
            .with_line_number(true)
            .with_file(true);

        // Debug builds use the console subsystem, so cargo run displays development logs.
        // Windows release builds remain GUI applications and may have no attached console.
        let stderr_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_ansi(cfg!(debug_assertions))
            .with_target(true)
            .with_thread_names(true)
            .with_thread_ids(true)
            .with_line_number(true)
            .with_file(true);

        let subscriber = tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_error::ErrorLayer::default())
            .with(file_layer)
            .with(stderr_layer);

        tracing::subscriber::set_global_default(subscriber)
            .context("set global tracing subscriber")?;

        install_panic_hook(log_dir.clone());

        tracing::info!(
            app_version = env!("CARGO_PKG_VERSION"),
            debug_build = cfg!(debug_assertions),
            process_id = std::process::id(),
            db_path = %app_paths.db_path.display(),
            log_dir = %log_dir.display(),
            filter_source,
            filter = %env_filter_str,
            rustickers_log = rustickers_log_value.as_deref().unwrap_or(""),
            rust_log = rust_log_value.as_deref().unwrap_or(""),
            "Logging initialized"
        );

        if let Some(err) = filter_parse_error {
            tracing::warn!(error = %err, filter_source = "RUSTICKERS_LOG", "Failed to parse RUSTICKERS_LOG; continuing");
        }

        Ok(Self { _file: file_guard })
    }
}

fn install_panic_hook(log_dir: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        write_panic_report(&log_dir, &info.to_string(), &backtrace);
        tracing::error!(panic = ?info, backtrace = ?backtrace, "panic");
        previous(info);
    }));
}

fn write_panic_report(log_dir: &Path, panic_message: &str, backtrace: &std::backtrace::Backtrace) {
    let report = format!(
        "\n=== {} ===\nprocess_id={}\npanic={}\nbacktrace:\n{}\n",
        chrono::Local::now().to_rfc3339(),
        std::process::id(),
        panic_message,
        backtrace
    );
    let path = log_dir.join("rustickers-crash.log");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(report.as_bytes());
        let _ = file.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_report_is_written_synchronously() {
        let directory = std::env::temp_dir().join(format!(
            "rustickers-logging-test-{}-{}",
            std::process::id(),
            crate::utils::time::now_unix_millis()
        ));
        std::fs::create_dir_all(&directory).unwrap();

        write_panic_report(
            &directory,
            "test panic",
            &std::backtrace::Backtrace::disabled(),
        );

        let crash_log = directory.join("rustickers-crash.log");
        let contents = std::fs::read_to_string(&crash_log).unwrap();
        assert!(contents.contains("test panic"));
        assert!(contents.contains("process_id="));

        std::fs::remove_file(crash_log).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
