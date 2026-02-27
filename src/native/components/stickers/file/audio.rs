use gpui::{
    Context, Image, ImageFormat, ImageSource, ObjectFit, Rgba, Window, div, img, prelude::*, px,
    relative, rgba,
};
use gpui_component::PixelsExt;
use gpui_component::{button::Button, h_flex, v_flex};
use lofty::prelude::*;
use lofty::probe::Probe;
use rodio::{Decoder, Source};
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use crate::model::content::FileStickerContent;
use crate::native::components::IconName;
use crate::native::windows::StickerWindowEvent;

pub(super) const AUDIO_ANIM_INTERVAL_MS: u64 = 80;

pub(super) enum AudioCmd {
    Load(PathBuf),
    Play,
    Pause,
}

pub(super) enum AudioEvent {
    Ended,
    Frame(AudioFrameMetrics),
}

#[derive(Copy, Clone, Debug, Default)]
pub(super) struct AudioFrameMetrics {
    pub(super) energy: f32,
    pub(super) low: f32,
    pub(super) mid: f32,
    pub(super) high: f32,
}

pub(super) struct AudioHandle {
    pub(super) cmd_tx: mpsc::Sender<AudioCmd>,
    event_rx: Option<mpsc::Receiver<AudioEvent>>,
}

impl AudioHandle {
    pub(super) fn take_event_rx(&mut self) -> Option<mpsc::Receiver<AudioEvent>> {
        self.event_rx.take()
    }
}

pub(super) struct AudioMetadata {
    pub(super) title: Option<String>,
    pub(super) artist: Option<String>,
    pub(super) album: Option<String>,
    pub(super) cover: Option<Arc<Image>>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) enum AudioPlayMode {
    Manual,
    Sequential,
    Random,
}

impl AudioPlayMode {
    fn cycle(self) -> Self {
        match self {
            Self::Manual => Self::Sequential,
            Self::Sequential => Self::Random,
            Self::Random => Self::Manual,
        }
    }

    fn is_autoplay(self) -> bool {
        !matches!(self, Self::Manual)
    }

    fn is_random(self) -> bool {
        matches!(self, Self::Random)
    }
}

pub(super) struct AudioState {
    pub(super) handle: Option<AudioHandle>,
    pub(super) event_rx: Option<mpsc::Receiver<AudioEvent>>,
    pub(super) is_playing: bool,
    pub(super) play_mode: AudioPlayMode,
    pub(super) siblings: Vec<PathBuf>,
    pub(super) current_idx: usize,
    pub(super) siblings_loaded: bool,
    pub(super) anim_tick: u64,
    pub(super) anim_loop_started: bool,
    pub(super) frame_metrics: AudioFrameMetrics,
    pub(super) metadata: Option<AudioMetadata>,
}

impl Default for AudioState {
    fn default() -> Self {
        Self {
            handle: None,
            event_rx: None,
            is_playing: false,
            play_mode: AudioPlayMode::Manual,
            siblings: Vec::new(),
            current_idx: 0,
            siblings_loaded: false,
            anim_tick: 0,
            anim_loop_started: false,
            frame_metrics: AudioFrameMetrics::default(),
            metadata: None,
        }
    }
}

struct AudioAnalyzer {
    frames: Vec<AudioFrameMetrics>,
    index: usize,
}

impl AudioAnalyzer {
    fn from_path(path: &Path) -> Self {
        Self {
            frames: decode_audio_frames(path),
            index: 0,
        }
    }

    fn next(&mut self) -> AudioFrameMetrics {
        if self.frames.is_empty() {
            return AudioFrameMetrics::default();
        }
        if self.index >= self.frames.len() {
            return AudioFrameMetrics::default();
        }
        let frame = self.frames[self.index];
        self.index = self.index.saturating_add(1);
        frame
    }
}

fn normalize_metric(value: f32, gain: f32) -> f32 {
    (value.max(0.0) * gain).tanh().clamp(0.0, 1.0)
}

fn chunk_metrics(chunk: &[f32]) -> AudioFrameMetrics {
    if chunk.is_empty() {
        return AudioFrameMetrics::default();
    }

    let mut energy_sum = 0.0f32;
    let mut derivative_sum = 0.0f32;
    let mut zero_crossings = 0usize;
    let mut prev = chunk[0];

    for &sample in chunk {
        energy_sum += sample * sample;
        derivative_sum += (sample - prev).abs();
        if (sample >= 0.0) != (prev >= 0.0) {
            zero_crossings += 1;
        }
        prev = sample;
    }

    let len = chunk.len() as f32;
    let rms = (energy_sum / len).sqrt();
    let derivative = derivative_sum / len;
    let zcr = zero_crossings as f32 / len;

    AudioFrameMetrics {
        energy: normalize_metric(rms, 3.4),
        low: normalize_metric(rms, 4.1),
        mid: normalize_metric(derivative, 14.5),
        high: normalize_metric(zcr, 16.0),
    }
}

fn decode_audio_frames(path: &Path) -> Vec<AudioFrameMetrics> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) => {
            tracing::warn!(
                "Audio: cannot open {} for frame analysis: {e}",
                path.display()
            );
            return Vec::new();
        }
    };
    let decoder = match Decoder::new(BufReader::new(file)) {
        Ok(decoder) => decoder,
        Err(e) => {
            tracing::warn!(
                "Audio: decode error for frame analysis {}: {e}",
                path.display()
            );
            return Vec::new();
        }
    };

    let channels = decoder.channels().get().max(1) as usize;
    let sample_rate = decoder.sample_rate().get().max(1) as usize;
    let chunk_len = ((sample_rate * AUDIO_ANIM_INTERVAL_MS as usize) / 1000).max(128) * channels;

    let samples: Vec<f32> = decoder.collect();
    samples
        .chunks(chunk_len)
        .map(chunk_metrics)
        .collect::<Vec<_>>()
}

pub(super) fn spawn_audio_thread(initial_path: PathBuf) -> AudioHandle {
    let (tx, rx) = mpsc::channel::<AudioCmd>();
    let (event_tx, event_rx) = mpsc::channel::<AudioEvent>();
    std::thread::spawn(move || {
        let device_sink = match rodio::DeviceSinkBuilder::open_default_sink() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Audio: failed to open device sink: {e}");
                return;
            }
        };
        let player = rodio::Player::connect_new(&device_sink.mixer());
        audio_load_file(&player, &initial_path);
        let mut analyzer = AudioAnalyzer::from_path(&initial_path);
        let mut last_frame_sent = Instant::now();
        let mut was_empty = player.empty();
        loop {
            match rx.recv_timeout(Duration::from_millis(AUDIO_ANIM_INTERVAL_MS)) {
                Ok(cmd) => match cmd {
                    AudioCmd::Load(path) => {
                        audio_load_file(&player, &path);
                        analyzer = AudioAnalyzer::from_path(&path);
                        let _ = event_tx.send(AudioEvent::Frame(AudioFrameMetrics::default()));
                        was_empty = player.empty();
                        continue;
                    }
                    AudioCmd::Play => player.play(),
                    AudioCmd::Pause => player.pause(),
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }

            let is_empty = player.empty();
            if !was_empty && is_empty && !player.is_paused() {
                let _ = event_tx.send(AudioEvent::Ended);
                let _ = event_tx.send(AudioEvent::Frame(AudioFrameMetrics::default()));
            }

            if !is_empty
                && !player.is_paused()
                && last_frame_sent.elapsed() >= Duration::from_millis(AUDIO_ANIM_INTERVAL_MS)
            {
                let _ = event_tx.send(AudioEvent::Frame(analyzer.next()));
                last_frame_sent = Instant::now();
            }
            was_empty = is_empty;
        }
    });
    AudioHandle {
        cmd_tx: tx,
        event_rx: Some(event_rx),
    }
}

fn audio_load_file(player: &rodio::Player, path: &Path) {
    player.stop();
    match fs::File::open(path) {
        Ok(file) => match Decoder::new(BufReader::new(file)) {
            Ok(dec) => {
                player.append(dec);
                player.play();
            }
            Err(e) => tracing::warn!("Audio: decode error for {}: {e}", path.display()),
        },
        Err(e) => tracing::warn!("Audio: cannot open {}: {e}", path.display()),
    }
}

pub(super) fn load_audio_metadata(path: &Path) -> AudioMetadata {
    let probe = match Probe::open(path) {
        Ok(p) => p,
        Err(_) => {
            return AudioMetadata {
                title: None,
                artist: None,
                album: None,
                cover: None,
            };
        }
    };
    let tagged = match probe.read() {
        Ok(t) => t,
        Err(_) => {
            return AudioMetadata {
                title: None,
                artist: None,
                album: None,
                cover: None,
            };
        }
    };
    let tag = tagged.primary_tag().or_else(|| tagged.tags().first());
    let Some(tag) = tag else {
        return AudioMetadata {
            title: None,
            artist: None,
            album: None,
            cover: None,
        };
    };
    let title = tag.title().map(|s| s.into_owned());
    let artist = tag.artist().map(|s| s.into_owned());
    let album = tag.album().map(|s| s.into_owned());
    let cover = tag
        .pictures()
        .iter()
        .find(|p| matches!(p.pic_type(), lofty::picture::PictureType::CoverFront))
        .or_else(|| tag.pictures().first())
        .and_then(|pic| {
            let format = match pic.mime_type() {
                Some(lofty::picture::MimeType::Jpeg) => ImageFormat::Jpeg,
                Some(lofty::picture::MimeType::Png) => ImageFormat::Png,
                _ => return None,
            };
            Some(Arc::new(Image::from_bytes(format, pic.data().to_vec())))
        });
    AudioMetadata {
        title,
        artist,
        album,
        cover,
    }
}

// ── FileSticker audio methods ─────────────────────────────────────────────────

impl super::FileSticker {
    pub(super) fn audio_toggle_play(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = &self.audio.handle {
            if self.audio.is_playing {
                let _ = handle.cmd_tx.send(AudioCmd::Pause);
                self.audio.is_playing = false;
            } else {
                let _ = handle.cmd_tx.send(AudioCmd::Play);
                self.audio.is_playing = true;
            }
        }
        cx.notify();
    }

    fn audio_cycle_play_mode(&mut self) {
        self.audio.play_mode = self.audio.play_mode.cycle();
    }

    fn poll_audio_events(&mut self, cx: &mut Context<Self>) {
        let mut playback_ended = false;
        let mut disconnected = false;
        let mut latest_frame = None;

        loop {
            let recv_result = match self.audio.event_rx.as_ref() {
                Some(rx) => rx.try_recv(),
                None => break,
            };

            match recv_result {
                Ok(AudioEvent::Ended) => playback_ended = true,
                Ok(AudioEvent::Frame(frame)) => latest_frame = Some(frame),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }

        if disconnected {
            self.audio.event_rx = None;
        }

        if let Some(frame) = latest_frame {
            self.audio.frame_metrics = frame;
        }

        if playback_ended {
            if self.audio.play_mode.is_autoplay() {
                self.audio_navigate(1, cx);
            } else {
                self.audio.is_playing = false;
                self.audio.frame_metrics = AudioFrameMetrics::default();
                cx.notify();
            }
        }
    }

    pub(super) fn audio_navigate(&mut self, delta: i64, cx: &mut Context<Self>) {
        if !self.audio.siblings_loaded {
            self.audio_discover_siblings();
        }
        if self.audio.siblings.is_empty() {
            return;
        }

        let next_idx = if self.audio.play_mode.is_random() {
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as usize)
                .unwrap_or(0);
            seed % self.audio.siblings.len()
        } else {
            let len = self.audio.siblings.len() as i64;
            ((self.audio.current_idx as i64 + delta).rem_euclid(len)) as usize
        };

        self.audio.current_idx = next_idx;
        let new_path = self.audio.siblings[next_idx].clone();
        let new_path_str = new_path.to_string_lossy().to_string();

        self.source_paths = vec![new_path_str.clone()];
        self.summaries = vec![super::summary::FileSummary::from_source(&new_path_str)];
        self.preview = Some(super::preview::FilePreview::Audio {
            source_path: new_path.clone(),
        });

        if let Some(handle) = &self.audio.handle {
            let _ = handle.cmd_tx.send(AudioCmd::Load(new_path.clone()));
            self.audio.is_playing = true;
            self.audio.anim_loop_started = false;
            self.audio.frame_metrics = AudioFrameMetrics::default();
        } else {
            let mut handle = spawn_audio_thread(new_path.clone());
            self.audio.event_rx = handle.take_event_rx();
            self.audio.handle = Some(handle);
            self.audio.is_playing = true;
            self.audio.anim_loop_started = false;
            self.audio.frame_metrics = AudioFrameMetrics::default();
        }

        if self.id > 0 {
            let content = FileStickerContent {
                files: self.source_paths.clone(),
            }
            .to_json();
            let store = self.store.clone();
            let sticker_events_tx = self.sticker_events_tx.clone();
            let id = self.id;
            let new_title = super::utils::file_name_for_display(&new_path);
            cx.spawn(async move |entity, cx| {
                let _ = store.update_sticker_content(id, content).await;
                let _ = store.update_sticker_title(id, new_title.clone()).await;
                let _ = sticker_events_tx.send(StickerWindowEvent::TitleChanged {
                    id,
                    title: new_title,
                });
                let _ = entity.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }

        self.spawn_load_audio_metadata(new_path, cx);
    }

    pub(super) fn spawn_load_audio_metadata(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.audio.metadata = None;
        cx.spawn(async move |entity, cx| {
            let metadata = load_audio_metadata(&path);
            let _ = entity.update(cx, |this, cx| {
                this.audio.metadata = Some(metadata);
                cx.notify();
            });
        })
        .detach();
    }

    fn audio_discover_siblings(&mut self) {
        let current_path = match &self.preview {
            Some(super::preview::FilePreview::Audio { source_path }) => source_path.clone(),
            _ => return,
        };
        let parent = match current_path.parent() {
            Some(p) => p.to_path_buf(),
            None => return,
        };
        let mut siblings: Vec<PathBuf> = fs::read_dir(&parent)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().ok().is_some_and(|ft| ft.is_file()))
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| crate::utils::file::is_audio_ext(&ext.to_ascii_lowercase()))
                    .unwrap_or(false)
            })
            .collect();
        siblings.sort();
        let current_idx = siblings
            .iter()
            .position(|p| p == &current_path)
            .unwrap_or(0);
        self.audio.siblings = siblings;
        self.audio.current_idx = current_idx;
        self.audio.siblings_loaded = true;
    }

    pub(super) fn ensure_audio_anim_loop(&mut self, cx: &mut Context<Self>) {
        if self.audio.anim_loop_started || !self.audio.is_playing {
            return;
        }
        self.audio.anim_loop_started = true;
        self.spawn_audio_anim_tick(cx);
    }

    fn spawn_audio_anim_tick(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |entity, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(AUDIO_ANIM_INTERVAL_MS))
                .await;
            let _ = entity.update(cx, |this, cx| {
                this.poll_audio_events(cx);
                if this.audio.is_playing {
                    this.audio.anim_tick = this.audio.anim_tick.wrapping_add(1);
                    cx.notify();
                    this.spawn_audio_anim_tick(cx);
                } else {
                    this.audio.anim_loop_started = false;
                }
            });
        })
        .detach();
    }

    pub(super) fn audio_matrix_bg(&self, window: &Window) -> gpui::AnyElement {
        let tick = self.audio.anim_tick;
        let is_playing = self.audio.is_playing;
        let frame = self.audio.frame_metrics;
        let base_opacity: f32 = if is_playing { 0.045 } else { 0.02 };
        let rows = (window.bounds().size.height.as_f32() / 8.0)
            .floor()
            .max(1.0) as i32;
        let cols = (window.bounds().size.width.as_f32() / 8.0).floor().max(1.0) as i32;
        v_flex()
            .absolute()
            .left_0()
            .top_0()
            .right_0()
            .bottom_0()
            .overflow_hidden()
            .children((0..rows).map(move |row| {
                div()
                    .flex_none()
                    .py(px(2.0))
                    .h(relative(1.0 / rows as f32))
                    .child(h_flex().h_full().children((0..cols).map(move |col| {
                        let x = col as f32 / cols as f32;
                        let y = row as f32 / rows as f32;
                        let band = if y < 0.34 {
                            frame.high
                        } else if y < 0.67 {
                            frame.mid
                        } else {
                            frame.low
                        };

                        let sweep = ((tick as f32 * 0.24) + x * 10.0 - y * 5.5)
                            .sin()
                            .mul_add(0.5, 0.5);
                        let pulse = ((tick as f32 * 0.17) + x * 3.5 + y * 4.0)
                            .cos()
                            .mul_add(0.5, 0.5);

                        let beat = (frame.low * 0.7 + frame.mid * 0.3).powf(0.75);
                        let beat_gate = ((beat - 0.28) / 0.72).clamp(0.0, 1.0).powf(1.6);
                        let driven = (0.10 + band * 0.90) * (0.10 + frame.energy * 0.90);
                        let shimmer = (0.2 + 0.8 * sweep * pulse).powf(1.3);
                        let alpha = (base_opacity
                            + 0.28 * driven * shimmer
                            + 0.44 * beat_gate * (0.45 + 0.55 * sweep))
                            .clamp(0.0, 1.0);
                        div()
                            .px(px(2.0))
                            .flex_none()
                            .w(relative(1.0 / cols as f32))
                            .h_full()
                            .rounded(px(2.0))
                            .bg(Rgba {
                                r: 1.0,
                                g: 1.0,
                                b: 1.0,
                                a: alpha,
                            })
                    })))
            }))
            .into_any_element()
    }

    pub(super) fn audio_player_view(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let file_name = match &self.preview {
            Some(super::preview::FilePreview::Audio { source_path }) => {
                super::utils::file_name_for_display(source_path.as_path())
            }
            _ => "Unknown".to_string(),
        };
        let is_playing = self.audio.is_playing;
        let play_mode = self.audio.play_mode;
        let play_mode_icon = if !play_mode.is_autoplay() {
            IconName::Forward
        } else if play_mode.is_random() {
            IconName::Shuffle
        } else {
            IconName::Loop
        };

        let (display_title, display_artist, display_album, cover) =
            if let Some(meta) = &self.audio.metadata {
                (
                    meta.title
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| file_name.clone()),
                    meta.artist.clone(),
                    meta.album.clone(),
                    meta.cover.clone(),
                )
            } else {
                (file_name, None, None, None)
            };

        let controls = h_flex()
            .gap_2()
            .child(
                Button::new("audio-prev")
                    .icon(IconName::ArrowLeft)
                    .bg(rgba(0x00000000))
                    .border_0()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.audio_navigate(-1, cx);
                    })),
            )
            .child(
                Button::new("audio-play-pause")
                    .icon(if is_playing {
                        IconName::Pause
                    } else {
                        IconName::Play
                    })
                    .bg(rgba(0x00000000))
                    .border_0()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.audio_toggle_play(cx);
                    })),
            )
            .child(
                Button::new("audio-next")
                    .icon(IconName::ArrowRight)
                    .bg(rgba(0x00000000))
                    .border_0()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.audio_navigate(1, cx);
                    })),
            )
            .child(
                Button::new("audio-play-mode")
                    .icon(play_mode_icon)
                    .bg(if play_mode.is_autoplay() {
                        rgba(0xffffff30)
                    } else {
                        rgba(0x00000000)
                    })
                    .border_0()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.audio_cycle_play_mode();
                        cx.notify();
                    })),
            );

        let info_row = h_flex()
            .gap(px(10.0))
            .items_center()
            .when_some(cover, |row, cover_img| {
                row.child(
                    img(ImageSource::Image(cover_img))
                        .w(px(72.0))
                        .h(px(72.0))
                        .rounded(px(6.0))
                        .object_fit(ObjectFit::Cover)
                        .flex_none(),
                )
            })
            .child(
                v_flex()
                    .gap(px(2.0))
                    .max_w(px(200.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::BOLD)
                            .opacity(0.95)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(display_title),
                    )
                    .when_some(display_artist, |v, artist| {
                        v.child(
                            div()
                                .text_xs()
                                .opacity(0.75)
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(artist),
                        )
                    })
                    .when_some(display_album, |v, album| {
                        v.child(
                            div()
                                .text_xs()
                                .opacity(0.55)
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(album),
                        )
                    }),
            );

        div()
            .size_full()
            .relative()
            .child(self.audio_matrix_bg(window))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .items_center()
                    .gap(px(14.0))
                    .child(info_row)
                    .child(controls),
            )
            .into_any_element()
    }
}
