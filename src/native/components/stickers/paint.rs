use gpui::{
    AnyElement, Bounds, Context, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PathBuilder, PathStyle, Pixels, Point, Render, StrokeOptions, Window, WindowControlArea,
    canvas, div, point, prelude::*, px, rgb, rgba, size, transparent_black,
};
use gpui_component::{Sizable, button::Button, h_flex, v_flex, white};
use serde::{
    Deserialize, Serialize,
    ser::{SerializeSeq, SerializeStruct},
};
use std::{
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
    time::{Duration, Instant},
};

use crate::model::sticker::StickerColor;
use crate::native::{components::IconName, windows::StickerWindowEvent};
use crate::storage::ArcStickerStore;

const PAINT_COLORS: [u32; 8] = [
    0x000000ff, // black
    0xffffffff, // white
    0xeb5757ff, // red
    0xf2994aff, // orange
    0xf2c94cff, // yellow
    0x27ae60ff, // green
    0x2d9cdbff, // blue
    0x9b51e0ff, // purple
];

const PAINT_STROKE_WIDTHS: [f32; 5] = [1.0, 2.0, 3.0, 4.0, 6.0];

const PAINT_SAVE_DEBOUNCE: Duration = Duration::from_millis(3000);

const PAINT_NOTIFY_MIN_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct PaintPoint {
    x: i16,
    y: i16,
}

impl From<Point<Pixels>> for PaintPoint {
    fn from(value: Point<Pixels>) -> Self {
        fn clamp_i16(v: f32) -> i16 {
            let rounded = v.round();
            let bounded = rounded.max(i16::MIN as f32).min(i16::MAX as f32);
            bounded as i16
        }

        Self {
            x: clamp_i16(value.x.to_f64() as f32),
            y: clamp_i16(value.y.to_f64() as f32),
        }
    }
}

impl PaintPoint {
    fn from_f32(x: f32, y: f32) -> Self {
        fn clamp_i16(v: f32) -> i16 {
            let rounded = v.round();
            let bounded = rounded.max(i16::MIN as f32).min(i16::MAX as f32);
            bounded as i16
        }

        Self {
            x: clamp_i16(x),
            y: clamp_i16(y),
        }
    }

    fn to_gpui(&self) -> Point<Pixels> {
        point(px(self.x as f32), px(self.y as f32))
    }

    fn distance_sq_to(&self, other: &PaintPoint) -> f32 {
        let dx = (self.x as f32) - (other.x as f32);
        let dy = (self.y as f32) - (other.y as f32);
        dx * dx + dy * dy
    }
}

fn default_stroke_width() -> f32 {
    2.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PaintStroke {
    #[serde(rename = "p")]
    points: Vec<PaintPoint>,

    #[serde(rename = "c")]
    color: u32,

    #[serde(rename = "w")]
    #[serde(default = "default_stroke_width")]
    width: f32,
}

#[derive(Debug, Clone)]
struct PaintStrokeState {
    stroke: PaintStroke,
    bounds: Option<(i16, i16, i16, i16)>,
    path: Option<gpui::Path<Pixels>>,
}

struct StrokesRef<'a>(&'a [PaintStrokeState]);

impl PaintStrokeState {
    fn new(stroke: PaintStroke) -> Self {
        let mut this = Self {
            stroke,
            bounds: None,
            path: None,
        };
        this.recalculate_bounds();
        this.rebuild_path();
        this
    }

    fn rebuild_path(&mut self) {
        self.path = build_spline_path(&self.stroke.points, self.stroke.width);
    }

    fn append_point(&mut self, point: PaintPoint) -> bool {
        let min_distance = min_point_distance_for_width(self.stroke.width);
        let min_distance_sq = min_distance * min_distance;

        if let Some(last) = self.stroke.points.last() {
            if point.distance_sq_to(last) < min_distance_sq {
                return false;
            }
        }

        self.stroke.points.push(point);
        self.expand_bounds_with(point);
        self.rebuild_path();

        true
    }

    fn recalculate_bounds(&mut self) {
        if self.stroke.points.is_empty() {
            self.bounds = None;
            return;
        }

        let mut min_x = self.stroke.points[0].x;
        let mut min_y = self.stroke.points[0].y;
        let mut max_x = self.stroke.points[0].x;
        let mut max_y = self.stroke.points[0].y;

        for p in &self.stroke.points[1..] {
            min_x = min_x.min(p.x);
            min_y = min_y.min(p.y);
            max_x = max_x.max(p.x);
            max_y = max_y.max(p.y);
        }

        self.bounds = Some((min_x, min_y, max_x, max_y));
    }

    fn expand_bounds_with(&mut self, point: PaintPoint) {
        match self.bounds {
            Some((min_x, min_y, max_x, max_y)) => {
                self.bounds = Some((
                    min_x.min(point.x),
                    min_y.min(point.y),
                    max_x.max(point.x),
                    max_y.max(point.y),
                ));
            }
            None => {
                self.bounds = Some((point.x, point.y, point.x, point.y));
            }
        }
    }

    fn intersects_canvas(&self, canvas_bounds: Bounds<Pixels>) -> bool {
        let Some((min_x, min_y, max_x, max_y)) = self.bounds else {
            return false;
        };

        let pad = self.stroke.width + 2.0;
        let left = min_x as f32 - pad;
        let top = min_y as f32 - pad;
        let right = max_x as f32 + pad;
        let bottom = max_y as f32 + pad;

        let canvas_left = canvas_bounds.origin.x.to_f64() as f32;
        let canvas_top = canvas_bounds.origin.y.to_f64() as f32;
        let canvas_right = canvas_left + (canvas_bounds.size.width.to_f64() as f32);
        let canvas_bottom = canvas_top + (canvas_bounds.size.height.to_f64() as f32);

        !(right < canvas_left || left > canvas_right || bottom < canvas_top || top > canvas_bottom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PaintContent {
    strokes: Vec<PaintStroke>,

    #[serde(default = "default_paint_color")]
    current_color: u32,

    #[serde(default = "default_stroke_width")]
    current_width: f32,
}

struct PaintContentBorrowed<'a> {
    strokes: &'a [PaintStrokeState],
    current_color: u32,
    current_width: f32,
}

fn default_paint_color() -> u32 {
    PAINT_COLORS[0]
}

impl Default for PaintContent {
    fn default() -> Self {
        Self {
            strokes: Vec::new(),
            current_color: default_paint_color(),
            current_width: default_stroke_width(),
        }
    }
}

pub struct PaintSticker {
    id: i64,
    color: StickerColor,
    store: ArcStickerStore,
    _sticker_events_tx: std::sync::mpsc::Sender<StickerWindowEvent>,

    strokes: Arc<RwLock<Vec<PaintStrokeState>>>,
    current_color: u32,
    current_width: f32,
    painting: bool,

    last_notify_at: Option<Instant>,

    tool: PaintTool,

    save_debounce_generation: u64,

    error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaintTool {
    Pen,
    Eraser,
}

impl Default for PaintTool {
    fn default() -> Self {
        Self::Pen
    }
}

impl PaintSticker {
    pub fn new(
        id: i64,
        color: StickerColor,
        store: ArcStickerStore,
        content: &str,
        sticker_events_tx: std::sync::mpsc::Sender<StickerWindowEvent>,
    ) -> Self {
        let content = serde_json::from_str::<PaintContent>(content).unwrap_or_default();
        Self {
            id,
            color,
            store,
            _sticker_events_tx: sticker_events_tx,
            strokes: Arc::new(RwLock::new(
                content
                    .strokes
                    .into_iter()
                    .map(PaintStrokeState::new)
                    .collect(),
            )),
            current_color: content.current_color,
            current_width: content.current_width,
            painting: false,
            last_notify_at: None,
            tool: PaintTool::default(),
            save_debounce_generation: 0,
            error: None,
        }
    }

    fn throttled_notify(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let should_notify = match self.last_notify_at {
            None => true,
            Some(last) => now.duration_since(last) >= PAINT_NOTIFY_MIN_INTERVAL,
        };

        if should_notify {
            self.last_notify_at = Some(now);
            cx.notify();
        }
    }

    fn cancel_debounced_save(&mut self) {
        self.save_debounce_generation = self.save_debounce_generation.wrapping_add(1);
    }

    fn save_state_debounced(&mut self, cx: &mut Context<Self>) {
        self.save_debounce_generation = self.save_debounce_generation.wrapping_add(1);
        let generation = self.save_debounce_generation;

        cx.spawn(async move |entity, cx| {
            cx.background_executor().timer(PAINT_SAVE_DEBOUNCE).await;

            let _ = entity.update(cx, |this, cx| {
                if this.save_debounce_generation != generation {
                    return;
                }

                // If we're still actively drawing, wait for a proper mouse-up.
                if this.painting {
                    return;
                }

                let _ = this.save_state(cx);
                tracing::debug!("Paint sticker {} debounced save complete", this.id);
            });
        })
        .detach();
    }

    fn strokes_read(&self) -> RwLockReadGuard<'_, Vec<PaintStrokeState>> {
        match self.strokes.read() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        }
    }

    fn strokes_write(&self) -> RwLockWriteGuard<'_, Vec<PaintStrokeState>> {
        match self.strokes.write() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        }
    }

    fn save_state(&mut self, cx: &mut Context<Self>) -> bool {
        // Avoid cloning the entire strokes vector (and all point data) just to serialize.

        // Keep the read lock in a tight scope so we can update `self.error` on failure.
        let json = {
            let strokes_guard = self.strokes_read();
            let borrowed = PaintContentBorrowed {
                strokes: &strokes_guard,
                current_color: self.current_color,
                current_width: self.current_width,
            };
            serde_json::to_string(&borrowed)
        };

        let json = match json {
            Ok(json) => json,
            Err(err) => {
                self.error = Some(format!("Failed to serialize paint sticker: {err}"));
                return false;
            }
        };

        let store = self.store.clone();
        let id = self.id;

        cx.spawn(async move |entity, cx| {
            if let Err(err) = store.update_sticker_content(id, json).await {
                let _ = entity.update(cx, |this, cx| {
                    this.error = Some(format!("Failed to save paint sticker: {err:#}"));
                    cx.notify();
                });
                return;
            }

            let _ = entity.update(cx, |this, cx| {
                this.error = None;
                cx.notify();
            });
        })
        .detach();

        true
    }

    fn eraser_radius(&self) -> f32 {
        // Reasonable default that still feels usable when stroke width is small.
        (self.current_width * 3.0).max(8.0)
    }

    fn erase_at(&mut self, position: Point<Pixels>) {
        let target = PaintPoint::from(position);
        let radius = self.eraser_radius();
        let radius_sq = radius * radius;

        let mut strokes = self.strokes_write();
        let old_strokes = std::mem::take(&mut *strokes);
        let mut new_strokes: Vec<PaintStrokeState> = Vec::with_capacity(old_strokes.len());

        for mut stroke_state in old_strokes {
            if stroke_state.stroke.points.is_empty() {
                continue;
            }

            let color = stroke_state.stroke.color;
            let width = stroke_state.stroke.width;
            let points = std::mem::take(&mut stroke_state.stroke.points);

            let mut erased_any = false;
            let mut ranges: Vec<(usize, usize)> = Vec::new();
            let mut run_start: Option<usize> = None;

            for (idx, point) in points.iter().enumerate() {
                let is_erased = point.distance_sq_to(&target) <= radius_sq;

                if is_erased {
                    erased_any = true;
                    if let Some(start) = run_start.take() {
                        ranges.push((start, idx));
                    }
                } else if run_start.is_none() {
                    run_start = Some(idx);
                }
            }

            if let Some(start) = run_start {
                ranges.push((start, points.len()));
            }

            if !erased_any {
                stroke_state.stroke.points = points;
                new_strokes.push(stroke_state);
                continue;
            }

            if ranges.is_empty() {
                continue;
            }

            let (first_start, first_end) = ranges[0];
            stroke_state.stroke.points = points[first_start..first_end].to_vec();
            stroke_state.recalculate_bounds();
            stroke_state.rebuild_path();
            new_strokes.push(stroke_state);

            for (start, end) in ranges.into_iter().skip(1) {
                new_strokes.push(PaintStrokeState::new(PaintStroke {
                    points: points[start..end].to_vec(),
                    color,
                    width,
                }));
            }
        }

        *strokes = new_strokes;
    }

    fn toolbar_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let current_color = self.current_color;
        let current_width = self.current_width;

        let eraser = Button::new("eraser")
            .icon(match self.tool {
                PaintTool::Eraser => IconName::Eraser,
                PaintTool::Pen => IconName::Paint,
            })
            .small()
            .border_0()
            .bg(transparent_black())
            .text_color(rgba(current_color))
            .occlude()
            .on_click(cx.listener(|this, _, _, cx| {
                this.tool = if this.tool == PaintTool::Eraser {
                    PaintTool::Pen
                } else {
                    PaintTool::Eraser
                };
                cx.notify();
            }));

        let mut color_picker = h_flex().gap_1().py_1().items_center();
        for &c in PAINT_COLORS.iter() {
            let is_selected = c == current_color;
            color_picker = color_picker.child(
                div()
                    .w(px(14.0))
                    .h(px(14.0))
                    .bg(rgba(c))
                    .rounded_full()
                    .cursor_pointer()
                    .when(is_selected, |v| v.border_1().border_color(rgb(0xffffff)))
                    .when(!is_selected, |v| {
                        v.border_1().border_color(rgba(0x00000000))
                    })
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.current_color = c;
                            cx.stop_propagation();
                            cx.notify();
                            window.prevent_default();
                        }),
                    ),
            );
        }

        let mut stroke_picker = h_flex().gap_1().py_1().items_center();
        for &w in PAINT_STROKE_WIDTHS.iter() {
            let is_selected = (w - current_width).abs() < f32::EPSILON;
            stroke_picker = stroke_picker.child(
                div()
                    .cursor_pointer()
                    .child(make_dot(w, current_color, is_selected))
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.current_width = w;
                            cx.stop_propagation();
                            cx.notify();
                            window.prevent_default();
                        }),
                    ),
            )
        }

        div()
            .w_full()
            .pl_1()
            .pr_3()
            .absolute()
            .left_0()
            .top_0()
            .right_0()
            .occlude()
            .window_control_area(WindowControlArea::Drag)
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .flex_wrap()
                    .child(eraser)
                    .child(div().child("|").opacity(0.2))
                    .child(stroke_picker)
                    .child(div().child("|").opacity(0.2))
                    .child(color_picker),
            )
            .into_any_element()
    }

    fn canvas_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let strokes = self.strokes.clone();

        div()
            .size_full()
            .occlude()
            .child(
                canvas(
                    move |_, _, _| {},
                    move |canvas_bounds, _, window, _| {
                        let strokes = match strokes.read() {
                            Ok(guard) => guard,
                            Err(err) => err.into_inner(),
                        };

                        for stroke in strokes.iter() {
                            if stroke.stroke.points.is_empty() {
                                continue;
                            }

                            if !stroke.intersects_canvas(canvas_bounds) {
                                continue;
                            }

                            let base_color = rgba(stroke.stroke.color);
                            if let Some(path) = &stroke.path {
                                window.paint_path(path.clone(), base_color);
                            }
                        }
                    },
                )
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    // Starting a new stroke should cancel any pending debounced save.
                    this.cancel_debounced_save();
                    this.painting = true;

                    match this.tool {
                        PaintTool::Pen => {
                            let mut points = Vec::with_capacity(64);
                            points.push(PaintPoint::from(ev.position));
                            let stroke = PaintStroke {
                                points,
                                color: this.current_color,
                                width: this.current_width,
                            };
                            this.strokes_write().push(PaintStrokeState::new(stroke));
                            cx.notify();
                        }
                        PaintTool::Eraser => {
                            this.erase_at(ev.position);
                            cx.notify();
                        }
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if !this.painting {
                    return;
                }

                match this.tool {
                    PaintTool::Pen => {
                        let mut strokes = this.strokes_write();

                        if let Some(stroke) = strokes.last_mut() {
                            if !stroke.append_point(PaintPoint::from(ev.position)) {
                                return;
                            }
                        }
                    }
                    PaintTool::Eraser => {
                        this.erase_at(ev.position);
                    }
                }

                this.throttled_notify(cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.painting = false;
                    this.last_notify_at = None;
                    cx.notify();
                    this.save_state_debounced(cx);
                }),
            )
            .into_any_element()
    }
}

impl super::Sticker for PaintSticker {
    fn id(&self) -> i64 {
        self.id
    }

    fn save_on_close(&mut self, cx: &mut Context<Self>) -> bool {
        self.save_state(cx)
    }

    fn min_window_size() -> gpui::Size<i32> {
        size(100, 100)
    }

    fn default_window_size() -> gpui::Size<i32> {
        size(400, 300)
    }

    fn set_color(&mut self, color: StickerColor) {
        self.color = color;
    }
}

impl Render for PaintSticker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .gap_2()
            .relative()
            .child(self.canvas_view(cx))
            .when(window.is_window_hovered(), |v| {
                v.child(self.toolbar_view(cx))
            })
    }
}

fn make_dot(w: f32, color: u32, is_selected: bool) -> AnyElement {
    div()
        .w(px(14.0))
        .h(px(14.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .when(is_selected, |v| v.border_1().border_color(white()))
        .child(
            div()
                .w(px((w + 3.0).max(4.0)))
                .h(px((w + 3.0).max(4.0)))
                .bg(rgba(color))
                .rounded_full(),
        )
        .into_any_element()
}

fn min_point_distance_for_width(width: f32) -> f32 {
    // Keep long strokes compact to avoid memory spikes and render slowdown.
    (width * 1.5).max(4.0)
}

fn build_spline_path(points: &[PaintPoint], width: f32) -> Option<gpui::Path<Pixels>> {
    if points.is_empty() {
        return None;
    }

    if points.len() == 1 {
        return build_dot_path(&points[0], width);
    }

    let options = StrokeOptions::default()
        .with_line_width(width)
        .with_line_cap(lyon::path::LineCap::Round)
        .with_line_join(lyon::path::LineJoin::Round)
        .with_tolerance(0.02);

    let mut builder = PathBuilder::stroke(px(width)).with_style(PathStyle::Stroke(options));
    builder.move_to(points[0].to_gpui());

    for p in points.iter().skip(1) {
        builder.line_to(p.to_gpui());
    }

    builder.build().ok()
}

fn build_dot_path(center: &PaintPoint, width: f32) -> Option<gpui::Path<Pixels>> {
    let x = center.x as f32;
    let y = center.y as f32;
    let half_segment = (width * 0.25).max(0.01);
    let points = [
        PaintPoint::from_f32(x - half_segment, y),
        PaintPoint::from_f32(x + half_segment, y),
    ];

    build_spline_path(&points, width)
}

impl Serialize for StrokesRef<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for stroke_state in self.0 {
            seq.serialize_element(&stroke_state.stroke)?;
        }
        seq.end()
    }
}

impl Serialize for PaintContentBorrowed<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("PaintContent", 3)?;
        state.serialize_field("strokes", &StrokesRef(self.strokes))?;
        state.serialize_field("current_color", &self.current_color)?;
        state.serialize_field("current_width", &self.current_width)?;
        state.end()
    }
}
