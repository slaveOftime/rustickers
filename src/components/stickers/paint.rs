use gpui::{
    Context, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Point,
    Render, Rgba, Window, WindowControlArea, canvas, div, point, prelude::*, px, rgb, rgba, size,
};
use gpui_component::{Sizable, button::Button, h_flex, scroll::ScrollableElement, v_flex};
use serde::{Deserialize, Serialize};

use crate::{model::sticker::StickerColor, storage::ArcStickerStore, windows::StickerWindowEvent};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PaintStroke {
    points: Vec<PaintPoint>,
    color: u32,
}

#[derive(Debug, Clone, Deserialize)]
struct PaintContentV1 {
    #[serde(default)]
    lines: Vec<Vec<PaintPoint>>,
    #[serde(default)]
    dashed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PaintContent {
    strokes: Vec<PaintStroke>,
    dashed: bool,
    current_color: u32,
}

impl Default for PaintContent {
    fn default() -> Self {
        Self {
            strokes: Vec::new(),
            dashed: false,
            current_color: PAINT_COLORS[0],
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
    dashed: bool,
    current_color: u32,
    painting: bool,

    error: Option<String>,
}

impl PaintSticker {
    pub fn new(
        id: i64,
        color: StickerColor,
        store: ArcStickerStore,
        content: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
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
                        })
                        .collect(),
                    dashed: v1.dashed,
                    current_color: PAINT_COLORS[0],
                },
            })
            .unwrap_or_default();
        Self {
            id,
            color,
            store,
            _sticker_events_tx: sticker_events_tx,
            strokes: content.strokes,
            dashed: content.dashed,
            current_color: content.current_color,
            painting: false,
            error: None,
        }
    }

    fn build_content(&self) -> PaintContent {
        PaintContent {
            strokes: self.strokes.clone(),
            dashed: self.dashed,
            current_color: self.current_color,
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

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.strokes.clear();
        cx.notify();
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strokes = self.strokes.clone();
        let dashed = self.dashed;
        let current_color = self.current_color;

        let mut color_picker = h_flex()
            .gap_1()
            .items_center()
            .flex_shrink()
            .overflow_x_scrollbar();
        for &c in PAINT_COLORS.iter() {
            let is_selected = c == current_color;
            color_picker = color_picker.child(
                div()
                    .w(px(14.0))
                    .h(px(14.0))
                    .bg(rgba(c))
                    .rounded_full()
                    .cursor_pointer()
                    .occlude()
                    .when(is_selected, |v| v.border_2().border_color(rgb(0xffffff)))
                    .when(!is_selected, |v| {
                        v.border_1().border_color(rgba(0x00000000))
                    })
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.current_color = c;
                            cx.notify();
                        }),
                    ),
            );
        }

        let toolbar = h_flex()
            .window_control_area(WindowControlArea::Drag)
            .items_center()
            .gap_2()
            .w_full()
            .child("Draw")
            .child(
                Button::new("paint-dash")
                    .label(if dashed { "Solid" } else { "Dashed" })
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dashed = !this.dashed;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("paint-clear")
                    .label("Clear")
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.clear(cx);
                    })),
            )
            .child(color_picker);

        let canvas_view = div()
            .size_full()
            .child(
                canvas(
                    move |_, _, _| {},
                    move |_, _, window, _| {
                        for stroke in strokes.clone() {
                            if stroke.points.len() < 2 {
                                continue;
                            }

                            let mut builder = PathBuilder::stroke(px(2.));
                            if dashed {
                                builder = builder.dash_array(&[px(6.), px(4.)]);
                            }

                            for (i, p) in stroke.points.iter().enumerate() {
                                let p = p.to_gpui();
                                if i == 0 {
                                    builder.move_to(p);
                                } else {
                                    builder.line_to(p);
                                }
                            }

                            if let Ok(path) = builder.build() {
                                window.paint_path(path, rgba(stroke.color));
                            }
                        }
                    },
                )
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, _| {
                    this.painting = true;
                    let stroke = PaintStroke {
                        points: vec![PaintPoint::from(ev.position)],
                        color: this.current_color,
                    };
                    this.strokes.push(stroke);
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if !this.painting {
                    return;
                }

                if let Some(stroke) = this.strokes.last_mut() {
                    stroke.points.push(PaintPoint::from(ev.position));
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
            );

        v_flex()
            .size_full()
            .gap_2()
            .p_2()
            .bg(Rgba {
                a: 0.85,
                ..self.color.bg()
            })
            .child(toolbar)
            .child(canvas_view)
    }
}
