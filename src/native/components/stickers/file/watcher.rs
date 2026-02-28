use futures::StreamExt;
use futures::channel::mpsc as async_mpsc;
use gpui::{Context, Window};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

const FILE_WATCH_DEBOUNCE: Duration = Duration::from_millis(100);

impl super::FileSticker {
    pub(super) fn init_file_watcher(&mut self) {
        let (event_tx, event_rx) = async_mpsc::unbounded::<()>();
        let watch_pending = Arc::clone(&self.watch_pending);
        let watch_stop = Arc::clone(&self.watch_stop);

        let mut watcher = match RecommendedWatcher::new(
            move |result: notify::Result<notify::Event>| {
                if watch_stop.load(Ordering::Acquire) {
                    return;
                }
                let Ok(event) = result else { return };
                if matches!(&event.kind, notify::event::EventKind::Access(_)) {
                    return;
                }
                if !watch_pending.swap(true, Ordering::AcqRel) {
                    let _ = event_tx.unbounded_send(());
                }
            },
            Config::default(),
        ) {
            Ok(watcher) => watcher,
            Err(err) => {
                self.error = Some(format!("Failed to initialize file watcher: {err}"));
                return;
            }
        };

        for raw_path in &self.source_paths {
            let Some((watch_path, mode)) = build_watch_target(raw_path) else {
                continue;
            };
            if let Err(err) = watcher.watch(&watch_path, mode) {
                tracing::warn!(
                    path = %watch_path.to_string_lossy(),
                    error = %err,
                    "Failed to watch sticker source path"
                );
            }
        }

        self.watcher = Some(watcher);
        self.watch_events_rx = Some(event_rx);
    }

    pub(super) fn ensure_watch_loop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.watch_loop_started {
            return;
        }
        let Some(mut event_rx) = self.watch_events_rx.take() else {
            return;
        };
        self.watch_loop_started = true;
        let entity = cx.entity();
        let watch_stop = Arc::clone(&self.watch_stop);
        window
            .spawn(cx, async move |cx| {
                while event_rx.next().await.is_some() {
                    if watch_stop.load(Ordering::Acquire) {
                        break;
                    }
                    loop {
                        cx.background_executor().timer(FILE_WATCH_DEBOUNCE).await;
                        if watch_stop.load(Ordering::Acquire) {
                            break;
                        }
                        let should_break = entity
                            .update_in(cx, |this, window, cx| {
                                this.refresh_from_watch_if_ready(window, cx)
                            })
                            .unwrap_or(true);
                        if should_break {
                            break;
                        }
                    }
                }
            })
            .detach();
    }

    pub(super) fn refresh_from_watch_if_ready(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.watch_pending.load(Ordering::Acquire) {
            return true;
        }
        if self.refreshing {
            return false;
        }
        self.watch_pending.store(false, Ordering::Release);
        self.spawn_refresh_preview(window, cx);
        self.spawn_refresh_summaries(window, cx);
        true
    }
}

fn build_watch_target(raw_path: &str) -> Option<(PathBuf, RecursiveMode)> {
    if crate::utils::url::is_url(raw_path) {
        return None;
    }
    let path = PathBuf::from(raw_path);
    if path.is_dir() {
        return Some((path, RecursiveMode::Recursive));
    }
    if path.is_file() {
        return Some((path, RecursiveMode::NonRecursive));
    }
    path.parent()
        .filter(|parent| parent.exists())
        .map(|parent| (parent.to_path_buf(), RecursiveMode::NonRecursive))
}
