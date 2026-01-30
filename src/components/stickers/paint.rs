use gpui::{
    AnyElement, Context, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder,
    PathStyle, Pixels, Point, Render, Rgba, StrokeOptions, Window, canvas, div, point, prelude::*,
    px, rgb, rgba, size, transparent_black,
};
use gpui_component::{Sizable, button::Button, h_flex, v_flex, white};
use serde::{Deserialize, Serialize};

use crate::{
    components::IconName, model::sticker::StickerColor, storage::ArcStickerStore,
    windows::StickerWindowEvent,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PaintPoint {
    x: f32,
    y: f32,
}

impl From<Point<Pixels>> for PaintPoint {
    fn from(value: Point<Pixels>) -> Self {
        Self {
            x: value.x.to_f64() as f32,
            y: value.y.to_f64() as f32,
        }
    }
}

impl PaintPoint {
    fn to_gpui(&self) -> Point<Pixels> {
        point(px(self.x), px(self.y))
    }
}

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

fn default_stroke_width() -> f32 {
    2.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PaintStroke {
    points: Vec<PaintPoint>,
    color: u32,

    #[serde(default = "default_stroke_width")]
    width: f32,
}

#[derive(Debug, Clone, Deserialize)]
struct PaintContentV1 {
    #[serde(default)]
    lines: Vec<Vec<PaintPoint>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PaintContent {
    strokes: Vec<PaintStroke>,
    current_color: u32,

    #[serde(default = "default_stroke_width")]
    current_width: f32,
}

impl Default for PaintContent {
    fn default() -> Self {
        Self {
            strokes: Vec::new(),
            current_color: PAINT_COLORS[0],
            current_width: default_stroke_width(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum PaintContentAny {
    V2(PaintContent),
    V1(PaintContentV1),
}

pub struct PaintSticker {
    id: i64,
    color: StickerColor,
    store: ArcStickerStore,
    _sticker_events_tx: std::sync::mpsc::Sender<StickerWindowEvent>,

    strokes: Vec<PaintStroke>,
    current_color: u32,
    current_width: f32,
    painting: bool,

    tool: PaintTool,

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
        let content = serde_json::from_str::<PaintContentAny>(content)
            .map(|x| match x {
                PaintContentAny::V2(v2) => v2,
                PaintContentAny::V1(v1) => PaintContent {
                    strokes: v1
                        .lines
                        .into_iter()
                        .map(|points| PaintStroke {
                            points,
                            color: PAINT_COLORS[0],
                            width: default_stroke_width(),
                        })
                        .collect(),
                    current_color: PAINT_COLORS[0],
                    current_width: default_stroke_width(),
                },
            })
            .unwrap_or_default();
        Self {
            id,
            color,
            store,
            _sticker_events_tx: sticker_events_tx,
            strokes: content.strokes,
            current_color: content.current_color,
            current_width: content.current_width,
            painting: false,
            tool: PaintTool::default(),
            error: None,
        }
    }
    fn build_content(&self) -> PaintContent {
        PaintContent {
            strokes: self.strokes.clone(),
            current_color: self.current_color,
            current_width: self.current_width,
        }
    }

    fn save_state(&mut self, cx: &mut Context<Self>) -> bool {
        let json = match serde_json::to_string(&self.build_content()) {
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

        let mut new_strokes: Vec<PaintStroke> = Vec::with_capacity(self.strokes.len());

        for stroke in self.strokes.drain(..) {
            if stroke.points.len() < 2 {
                continue;
            }

            let mut segment: Vec<PaintPoint> = Vec::new();
            for point in stroke.points {
                let dx = point.x - target.x;
                let dy = point.y - target.y;
                let is_erased = dx * dx + dy * dy <= radius_sq;

                if is_erased {
                    if segment.len() >= 2 {
                        new_strokes.push(PaintStroke {
                            points: std::mem::take(&mut segment),
                            color: stroke.color,
                            width: stroke.width,
                        });
                    } else {
                        segment.clear();
                    }
                } else {
                    segment.push(point);
                }
            }

            if segment.len() >= 2 {
                new_strokes.push(PaintStroke {
                    points: segment,
                    color: stroke.color,
                    width: stroke.width,
                });
            }
        }

        self.strokes = new_strokes;
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
            .child(
                canvas(
                    move |_, _, _| {},
                    move |_, _, window, _| {
                        for stroke in strokes {
                            if stroke.points.len() < 2 {
                                continue;
                            }

                            let points = dedupe_close_points(
                                &stroke.points,
                                min_point_distance_for_width(stroke.width),
                            );
                            if points.len() < 2 {
                                continue;
                            }

                            // Use round caps/joins and a tighter tolerance to reduce jagged edges.
                            // Also paint a subtle wider pass first to visually anti-alias pixel edges.
                            let base_color = rgba(stroke.color);
                            let feather_color = Rgba {
                                a: (base_color.a * 0.25).min(1.0),
                                ..base_color
                            };

                            // Feather pass (slightly wider) + main pass.
                            paint_spline(window, &points, stroke.width + 1.25, feather_color);
                            paint_spline(window, &points, stroke.width, base_color);
                        }
                    },
                )
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, _| {
                    this.painting = true;

                    match this.tool {
                        PaintTool::Pen => {
                            let stroke = PaintStroke {
                                points: vec![PaintPoint::from(ev.position)],
                                color: this.current_color,
                                width: this.current_width,
                            };
                            this.strokes.push(stroke);
                        }
                        PaintTool::Eraser => {
                            this.erase_at(ev.position);
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
                        if let Some(stroke) = this.strokes.last_mut() {
                            let p = PaintPoint::from(ev.position);

                            if let Some(last) = stroke.points.last() {
                                let min_distance = min_point_distance_for_width(stroke.width);
                                let dx = p.x - last.x;
                                let dy = p.y - last.y;
                                if dx * dx + dy * dy < (min_distance * min_distance) {
                                    return;
                                }
                            }

                            stroke.points.push(p);
                        }
                    }
                    PaintTool::Eraser => {
                        this.erase_at(ev.position);
                    }
                }

                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.painting = false;
                    cx.notify();
                    this.save_state(cx);
                }),
            )
            .into_any_element()
    }
}

impl super::Sticker for PaintSticker {
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
            .bg(Rgba {
                a: 0.85,
                ..self.color.bg()
            })
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

fn midpoint(a: Point<Pixels>, b: Point<Pixels>) -> Point<Pixels> {
    let ax = a.x.to_f64() as f32;
    let ay = a.y.to_f64() as f32;
    let bx = b.x.to_f64() as f32;
    let by = b.y.to_f64() as f32;
    point(px((ax + bx) * 0.5), px((ay + by) * 0.5))
}

fn min_point_distance_for_width(width: f32) -> f32 {
    // Skip ultra-close points to reduce jitter and make curves smoother.
    // Tuned to keep thin strokes responsive while stabilizing wider ones.
    (width * 0.25).max(0.75)
}

fn dedupe_close_points(points: &[PaintPoint], min_distance: f32) -> Vec<Point<Pixels>> {
    let min_distance_sq = min_distance * min_distance;
    let mut out: Vec<Point<Pixels>> = Vec::with_capacity(points.len());

    for p in points {
        let p = p.to_gpui();
        if let Some(last) = out.last().copied() {
            let dx = (p.x.to_f64() - last.x.to_f64()) as f32;
            let dy = (p.y.to_f64() - last.y.to_f64()) as f32;
            if dx * dx + dy * dy < min_distance_sq {
                continue;
            }
        }
        out.push(p);
    }

    out
}

fn paint_spline(window: &mut Window, points: &[Point<Pixels>], width: f32, color: Rgba) {
    let options = StrokeOptions::default()
        .with_line_width(width)
        .with_line_cap(lyon::path::LineCap::Round)
        .with_line_join(lyon::path::LineJoin::Round)
        .with_tolerance(0.02);

    let mut builder = PathBuilder::stroke(px(width)).with_style(PathStyle::Stroke(options));
    builder.move_to(points[0]);

    // Quadratic spline through midpoints.
    if points.len() == 2 {
        builder.line_to(points[1]);
    } else {
        for i in 1..points.len() - 1 {
            let ctrl = points[i];
            let to = midpoint(points[i], points[i + 1]);
            builder.curve_to(to, ctrl);
        }
        if let Some(last) = points.last().copied() {
            builder.line_to(last);
        }
    }

    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}
