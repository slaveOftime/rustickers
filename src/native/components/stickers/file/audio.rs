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
use std::time::Duration;

use crate::model::content::FileStickerContent;
use crate::native::components::IconName;
use crate::native::windows::StickerWindowEvent;

const ANALYSIS_INTERVAL_MS: u64 = 40;
const ANALYZER_SEEK_BACK_MS: u64 = 40;
const MATRIX_CELL_SIZE_PX: f32 = 12.0;
const MATRIX_IDLE_ALPHA: f32 = 0.3;

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

impl Default for AudioMetadata {
    fn default() -> Self {
        Self {
            title: None,
            artist: None,
            album: None,
            cover: None,
        }
    }
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
            anim_loop_started: false,
            frame_metrics: AudioFrameMetrics::default(),
            metadata: None,
        }
    }
}

impl AudioState {
    fn reset_visual_state(&mut self) {
        self.anim_loop_started = false;
        self.frame_metrics = AudioFrameMetrics::default();
    }

    fn load_path(&mut self, path: PathBuf) {
        if let Some(handle) = &self.handle {
            let _ = handle.cmd_tx.send(AudioCmd::Load(path));
        } else {
            let mut handle = spawn_thread(path);
            self.event_rx = handle.take_event_rx();
            self.handle = Some(handle);
        }

        self.is_playing = true;
        self.reset_visual_state();
    }
}

struct AudioAnalyzer {
    decoder: Option<Decoder<BufReader<fs::File>>>,
    chunk_len: usize,
}

impl AudioAnalyzer {
    fn from_path(path: &Path) -> Self {
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(e) => {
                tracing::warn!(
                    "Audio: cannot open {} for frame analysis: {e}",
                    path.display()
                );
                return Self::default();
            }
        };
        let decoder = match Decoder::new(BufReader::new(file)) {
            Ok(decoder) => decoder,
            Err(e) => {
                tracing::warn!(
                    "Audio: decode error for frame analysis {}: {e}",
                    path.display()
                );
                return Self::default();
            }
        };

        let channels = decoder.channels().get().max(1) as usize;
        let sample_rate = decoder.sample_rate().get().max(1) as usize;
        let chunk_len = ((sample_rate * ANALYSIS_INTERVAL_MS as usize) / 1000).max(128) * channels;

        Self {
            decoder: Some(decoder),
            chunk_len,
        }
    }

    fn at_pos(&mut self, position: Duration) -> AudioFrameMetrics {
        let Some(decoder) = self.decoder.as_mut() else {
            return AudioFrameMetrics::default();
        };

        let seek_position_ms = position
            .as_millis()
            .saturating_sub(ANALYZER_SEEK_BACK_MS as u128);
        let _ = decoder.try_seek(Duration::from_millis(seek_position_ms as u64));

        let mut chunk = Vec::with_capacity(self.chunk_len);
        for _ in 0..self.chunk_len {
            match decoder.next() {
                Some(sample) => chunk.push(sample),
                None => break,
            }
        }

        if chunk.is_empty() {
            AudioFrameMetrics::default()
        } else {
            chunk_metrics(&chunk)
        }
    }
}

impl Default for AudioAnalyzer {
    fn default() -> Self {
        Self {
            decoder: None,
            chunk_len: 128,
        }
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

fn pick_cover_image(tag: &lofty::tag::Tag) -> Option<Arc<Image>> {
    tag.pictures()
        .iter()
        .find(|picture| matches!(picture.pic_type(), lofty::picture::PictureType::CoverFront))
        .or_else(|| tag.pictures().first())
        .and_then(|picture| {
            let format = match picture.mime_type() {
                Some(lofty::picture::MimeType::Jpeg) => ImageFormat::Jpeg,
                Some(lofty::picture::MimeType::Png) => ImageFormat::Png,
                _ => return None,
            };
            Some(Arc::new(Image::from_bytes(format, picture.data().to_vec())))
        })
}

fn blend_frequency_bands(frame: AudioFrameMetrics, x: f32) -> f32 {
    if x < 0.5 {
        let t = x / 0.5;
        frame.low * (1.0 - t) + frame.mid * t
    } else {
        let t = (x - 0.5) / 0.5;
        frame.mid * (1.0 - t) + frame.high * t
    }
}

fn matrix_cell_alpha(
    frame: AudioFrameMetrics,
    rows: usize,
    row_from_bottom: f32,
    band: f32,
) -> f32 {
    let fill = (band * (0.35 + frame.energy * 0.65)).clamp(0.0, 1.0);
    let cell_height = (1.0 / rows as f32).max(0.0001);
    let is_active = row_from_bottom <= fill + cell_height;

    if is_active {
        (MATRIX_IDLE_ALPHA + 0.58 * band * (1.0 - row_from_bottom * 0.55)).min(0.7)
    } else {
        MATRIX_IDLE_ALPHA
    }
}

pub(super) fn spawn_thread(initial_path: PathBuf) -> AudioHandle {
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
        load_file(&player, &initial_path);
        let mut analyzer = AudioAnalyzer::from_path(&initial_path);
        let mut was_empty = player.empty();
        loop {
            match rx.recv_timeout(Duration::from_millis(ANALYSIS_INTERVAL_MS)) {
                Ok(cmd) => match cmd {
                    AudioCmd::Load(path) => {
                        load_file(&player, &path);
                        analyzer = AudioAnalyzer::from_path(&path);
                        let pos = player.get_pos();
                        let _ = event_tx.send(AudioEvent::Frame(analyzer.at_pos(pos)));
                        was_empty = player.empty();
                        continue;
                    }
                    AudioCmd::Play => {
                        player.play();
                        let pos = player.get_pos();
                        let _ = event_tx.send(AudioEvent::Frame(analyzer.at_pos(pos)));
                    }
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

            if !is_empty && !player.is_paused() {
                let pos = player.get_pos();
                let _ = event_tx.send(AudioEvent::Frame(analyzer.at_pos(pos)));
            }
            was_empty = is_empty;
        }
    });
    AudioHandle {
        cmd_tx: tx,
        event_rx: Some(event_rx),
    }
}

fn load_file(player: &rodio::Player, path: &Path) {
    match fs::File::open(path) {
        Ok(file) => match Decoder::new(BufReader::new(file)) {
            Ok(source) => {
                player.stop();
                player.append(source);
                player.play();
            }
            Err(e) => tracing::warn!("Audio: decode error for {}: {e}", path.display()),
        },
        Err(e) => tracing::warn!("Audio: cannot open {}: {e}", path.display()),
    }
}

pub(super) fn load_metadata(path: &Path) -> AudioMetadata {
    let Ok(tagged) = Probe::open(path).and_then(|probe| probe.read()) else {
        return AudioMetadata::default();
    };
    let tag = tagged.primary_tag().or_else(|| tagged.tags().first());
    let Some(tag) = tag else {
        return AudioMetadata::default();
    };

    let title = tag.title().map(|s| s.into_owned());
    let artist = tag.artist().map(|s| s.into_owned());
    let album = tag.album().map(|s| s.into_owned());
    let cover = pick_cover_image(tag);

    AudioMetadata {
        title,
        artist,
        album,
        cover,
    }
}

// ── FileSticker audio methods ─────────────────────────────────────────────────

impl super::FileSticker {
    fn state(&self) -> Option<&AudioState> {
        match &self.preview {
            Some(super::preview::FilePreview::Audio { state, .. }) => Some(state),
            _ => None,
        }
    }

    fn state_mut(&mut self) -> Option<&mut AudioState> {
        match &mut self.preview {
            Some(super::preview::FilePreview::Audio { state, .. }) => Some(state),
            _ => None,
        }
    }

    pub(super) fn stop_audio(&mut self) {
        if let Some(state) = self.state_mut() {
            state.handle = None;
            state.event_rx = None;
            state.is_playing = false;
            state.reset_visual_state();
        }
    }

    pub(super) fn toggle_play(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.state_mut() {
            if let Some(handle) = &state.handle {
                if state.is_playing {
                    let _ = handle.cmd_tx.send(AudioCmd::Pause);
                    state.is_playing = false;
                } else {
                    let _ = handle.cmd_tx.send(AudioCmd::Play);
                    state.is_playing = true;
                }
            }
            cx.notify();
        }
    }

    fn cycle_play_mode(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.state_mut() {
            state.play_mode = state.play_mode.cycle();
            cx.notify();

            if state.play_mode.is_autoplay() && !state.is_playing {
                self.navigate_autoplay(cx);
            }
        }
    }

    fn poll_events(&mut self, cx: &mut Context<Self>) {
        let mut playback_ended = false;
        let mut disconnected = false;
        let mut latest_frame = None;
        let mut should_autoplay = false;

        {
            let Some(state) = self.state_mut() else {
                return;
            };

            loop {
                let recv_result = match state.event_rx.as_ref() {
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
                state.event_rx = None;
            }

            if let Some(frame) = latest_frame {
                state.frame_metrics = frame;
            }

            if playback_ended {
                if state.play_mode.is_autoplay() {
                    should_autoplay = true;
                } else {
                    state.is_playing = false;
                    state.frame_metrics = AudioFrameMetrics::default();
                    cx.notify();
                }
            }
        }

        if should_autoplay {
            self.navigate_autoplay(cx);
        }
    }

    pub(super) fn navigate(&mut self, delta: i64, cx: &mut Context<Self>) {
        self.navigate_with_mode(delta, false, cx);
    }

    fn navigate_autoplay(&mut self, cx: &mut Context<Self>) {
        self.navigate_with_mode(1, true, cx);
    }

    fn navigate_with_mode(&mut self, delta: i64, allow_random: bool, cx: &mut Context<Self>) {
        if !self
            .state()
            .map(|state| state.siblings_loaded)
            .unwrap_or(false)
        {
            self.discover_siblings();
        }

        let Some(state) = self.state() else {
            return;
        };
        if state.siblings.is_empty() {
            return;
        }

        let next_idx = if allow_random && state.play_mode.is_random() {
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as usize)
                .unwrap_or(0);
            seed % state.siblings.len()
        } else {
            let len = state.siblings.len() as i64;
            ((state.current_idx as i64 + delta).rem_euclid(len)) as usize
        };

        let new_path = state.siblings[next_idx].clone();
        let new_path_str = new_path.to_string_lossy().to_string();

        self.source_paths = vec![new_path_str.clone()];
        self.summaries = vec![super::summary::FileSummary::from_source(&new_path_str)];

        if let Some(super::preview::FilePreview::Audio { source_path, state }) =
            self.preview.as_mut()
        {
            *source_path = new_path.clone();
            state.current_idx = next_idx;
            state.load_path(new_path.clone());
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

        self.spawn_load_metadata(new_path, cx);
    }

    pub(super) fn spawn_load_metadata(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Some(state) = self.state_mut() {
            state.metadata = None;
        }
        cx.spawn(async move |entity, cx| {
            let metadata = load_metadata(&path);
            let _ = entity.update(cx, |this, cx| {
                if let Some(state) = this.state_mut() {
                    state.metadata = Some(metadata);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn discover_siblings(&mut self) {
        let current_path = match &self.preview {
            Some(super::preview::FilePreview::Audio { source_path, .. }) => source_path.clone(),
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

        if let Some(state) = self.state_mut() {
            state.siblings = siblings;
            state.current_idx = current_idx;
            state.siblings_loaded = true;
        }
    }

    pub(super) fn ensure_anim_loop(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.state_mut() else {
            return;
        };
        if state.anim_loop_started || !state.is_playing {
            return;
        }
        state.anim_loop_started = true;
        self.spawn_anim_tick(cx);
    }

    fn spawn_anim_tick(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |entity, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(ANALYSIS_INTERVAL_MS))
                .await;
            let _ = entity.update(cx, |this, cx| {
                this.poll_events(cx);
                if let Some(state) = this.state_mut() {
                    if state.is_playing {
                        cx.notify();
                        this.spawn_anim_tick(cx);
                    } else {
                        state.anim_loop_started = false;
                    }
                }
            });
        })
        .detach();
    }

    pub(super) fn matrix_bg(&self, window: &Window) -> gpui::AnyElement {
        let default_view = div()
            .absolute()
            .inset_0()
            .bg(Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: MATRIX_IDLE_ALPHA,
            })
            .into_any_element();

        let Some(state) = self.state() else {
            return default_view;
        };

        if !state.is_playing {
            return default_view;
        }

        let frame = state.frame_metrics;
        let rows =
            ((window.bounds().size.height.as_f32() / MATRIX_CELL_SIZE_PX).floor() as usize).max(1);
        let cols =
            ((window.bounds().size.width.as_f32() / MATRIX_CELL_SIZE_PX).floor() as usize).max(1);

        v_flex()
            .absolute()
            .inset_0()
            .overflow_hidden()
            .children((0..rows).map(move |row| {
                div()
                    .flex_none()
                    .w_full()
                    .h(relative(1.0 / rows as f32))
                    .child(h_flex().h_full().children((0..cols).map(move |col| {
                        let x = col as f32 / cols as f32;
                        let row_from_bottom = (rows.saturating_sub(1) - row) as f32 / rows as f32;
                        let band = blend_frequency_bands(frame, x);
                        let alpha = matrix_cell_alpha(frame, rows, row_from_bottom, band);

                        div()
                            .flex_none()
                            .w(relative(1.0 / cols as f32))
                            .h_full()
                            .bg(Rgba {
                                r: 0.0,
                                g: 0.0,
                                b: 0.0,
                                a: alpha,
                            })
                    })))
            }))
            .into_any_element()
    }

    pub(super) fn player_view(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let (file_name, is_playing, play_mode, metadata) = match &self.preview {
            Some(super::preview::FilePreview::Audio { source_path, state }) => (
                super::utils::file_name_for_display(source_path.as_path()),
                state.is_playing,
                state.play_mode,
                state.metadata.as_ref(),
            ),
            _ => ("Unknown".to_string(), false, AudioPlayMode::Manual, None),
        };
        let play_mode_icon = if !play_mode.is_autoplay() {
            IconName::Forward
        } else if play_mode.is_random() {
            IconName::Shuffle
        } else {
            IconName::Loop
        };

        let (display_title, display_artist, display_album, cover) = if let Some(meta) = metadata {
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
                        this.navigate(-1, cx);
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
                        this.toggle_play(cx);
                    })),
            )
            .child(
                Button::new("audio-next")
                    .icon(IconName::ArrowRight)
                    .bg(rgba(0x00000000))
                    .border_0()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.navigate(1, cx);
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
                        this.cycle_play_mode(cx);
                    })),
            );

        let info_row = h_flex().gap(px(10.0)).items_center().child(
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
            .when_some(cover, |row, cover_img| {
                row.child(
                    img(ImageSource::Image(cover_img))
                        .absolute()
                        .inset_0()
                        .object_fit(ObjectFit::Cover),
                )
            })
            .child(self.matrix_bg(window))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .items_center()
                    .p_2()
                    .gap_3()
                    .child(info_row)
                    .child(controls),
            )
            .into_any_element()
    }
}
