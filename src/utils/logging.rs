use crate::storage::paths::AppPaths;

use anyhow::Context as _;
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
        // 3) trace (debug) / info (release)
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
                            "trace"
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

        // Console logs are helpful in debug/dev; in Windows release GUI builds there may be no console.
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

        install_panic_hook();

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

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Avoid panicking in the panic hook.
        let backtrace = std::backtrace::Backtrace::capture();
        tracing::error!(panic = ?info, backtrace = ?backtrace, "panic");
        previous(info);
    }));
}
