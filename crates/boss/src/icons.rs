//! Icons: Lucide font glyphs for UI chrome and canvas-drawn mini BPMN shapes
//! for the palette.
//!
//! The Lucide font (ISC licensed, embedded like the IBM Plex family) covers
//! toolbar, panel, and status-bar icons. Palette entries draw the actual BPMN
//! notation instead, so the palette doubles as a legend.

use crate::render;
use crate::theme::CanvasTokens;
use bpmn_core::NodeKind;
use bpmn_core::model::{GatewayKind, SubProcessKind, TaskType};
use iced::alignment::Vertical;
use iced::widget::canvas::{self, Canvas, Frame, Geometry, Path, Stroke};
use iced::widget::text;
use iced::{Element, Font, Point, Rectangle, Size};

/// The Lucide icon font bytes, registered at startup.
pub const FONT_BYTES: &[u8] = include_bytes!("../../../assets/fonts/lucide.ttf");
/// The Lucide icon font.
pub const LUCIDE: Font = Font::with_name("lucide");

/// `file-plus`.
pub const NEW: char = '\u{e0c9}';
/// `folder-open`.
pub const OPEN: char = '\u{e247}';
/// `save`.
pub const SAVE: char = '\u{e14d}';
/// `copy` (save as; the toolbar entry exists on native only).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub const SAVE_AS: char = '\u{e09e}';
/// `image-down` (SVG export).
pub const EXPORT: char = '\u{e540}';
/// `undo-2`.
pub const UNDO: char = '\u{e2a1}';
/// `redo-2`.
pub const REDO: char = '\u{e2a0}';
/// `zoom-in`.
pub const ZOOM_IN: char = '\u{e1b6}';
/// `zoom-out`.
pub const ZOOM_OUT: char = '\u{e1b7}';
/// `maximize` (zoom to fit).
pub const FIT: char = '\u{e112}';
/// `sun`.
pub const SUN: char = '\u{e178}';
/// `moon`.
pub const MOON: char = '\u{e11e}';
/// `plug-zap` (MCP server; native only).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub const MCP: char = '\u{e460}';
/// `mouse-pointer-2` (select tool).
pub const SELECT: char = '\u{e1c3}';
/// `move-up-right` (connect tool).
pub const CONNECT: char = '\u{e497}';
/// `chevron-left`.
pub const CHEVRON_LEFT: char = '\u{e06e}';
/// `chevron-right`.
pub const CHEVRON_RIGHT: char = '\u{e06f}';
/// `check`.
pub const CHECK: char = '\u{e06c}';
/// `circle-alert`.
pub const ALERT: char = '\u{e077}';
/// `panel-left`.
pub const PANEL_LEFT: char = '\u{e12a}';
/// `panel-right`.
pub const PANEL_RIGHT: char = '\u{e435}';

/// A Lucide glyph as a text widget, vertically centered for row layouts.
pub fn icon<'a>(glyph: char) -> iced::widget::Text<'a> {
    text(glyph.to_string())
        .font(LUCIDE)
        .size(14)
        .align_y(Vertical::Center)
}

/// What a palette mini-shape depicts.
#[derive(Debug, Clone, Copy)]
pub enum ShapeGlyph {
    /// A placeable node kind, drawn with its real notation.
    Node(NodeKind),
    /// A pool band.
    Pool,
}

/// A small canvas drawing the BPMN notation for a palette entry.
pub fn shape_glyph<'a, Message: 'a>(
    glyph: ShapeGlyph,
    tokens: CanvasTokens,
) -> Element<'a, Message> {
    Canvas::new(Glyph { glyph, tokens })
        .width(24)
        .height(20)
        .into()
}

struct Glyph {
    glyph: ShapeGlyph,
    tokens: CanvasTokens,
}

impl<Message> canvas::Program<Message> for Glyph {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let center = Point::new(bounds.width / 2.0, bounds.height / 2.0);
        let stroke = Stroke::default()
            .with_width(1.4)
            .with_color(self.tokens.stroke);

        match self.glyph {
            ShapeGlyph::Node(kind) => {
                draw_node_glyph(&mut frame, kind, center, stroke, self.tokens);
            }
            ShapeGlyph::Pool => {
                let colors = self.tokens.container;
                let stroke = stroke.with_color(colors.stroke);
                let rect = Rectangle {
                    x: center.x - 11.0,
                    y: center.y - 8.0,
                    width: 22.0,
                    height: 16.0,
                };
                frame.fill(&Path::rectangle(rect.position(), rect.size()), colors.fill);
                frame.stroke(&Path::rectangle(rect.position(), rect.size()), stroke);
                frame.stroke(
                    &Path::line(
                        Point::new(rect.x + 5.0, rect.y),
                        Point::new(rect.x + 5.0, rect.y + rect.height),
                    ),
                    stroke,
                );
            }
        }
        vec![frame.into_geometry()]
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one tiny drawing per palette entry; a table of glyphs"
)]
fn draw_node_glyph(
    frame: &mut Frame,
    kind: NodeKind,
    c: Point,
    stroke: Stroke<'_>,
    tokens: CanvasTokens,
) {
    // The palette doubles as a color legend: each mini-shape carries its
    // family's fill and stroke.
    let colors = match kind {
        NodeKind::StartEvent => tokens.start,
        NodeKind::EndEvent => tokens.end,
        NodeKind::IntermediateCatch | NodeKind::IntermediateThrow | NodeKind::Boundary => {
            tokens.intermediate
        }
        NodeKind::Task(_) => tokens.task,
        NodeKind::Gateway(_) => tokens.gateway,
        NodeKind::SubProcess(_) | NodeKind::CallActivity => tokens.container,
        NodeKind::DataObject | NodeKind::DataStore | NodeKind::TextAnnotation | NodeKind::Group => {
            tokens.data
        }
    };
    let stroke = stroke.with_color(colors.stroke);

    let rounded = |frame: &mut Frame, half_w: f32, half_h: f32, stroke: Stroke<'_>| {
        frame.fill(
            &Path::rounded_rectangle(
                Point::new(c.x - half_w, c.y - half_h),
                Size::new(half_w * 2.0, half_h * 2.0),
                3.0.into(),
            ),
            colors.fill,
        );
        frame.stroke(
            &Path::rounded_rectangle(
                Point::new(c.x - half_w, c.y - half_h),
                Size::new(half_w * 2.0, half_h * 2.0),
                3.0.into(),
            ),
            stroke,
        );
    };

    match kind {
        NodeKind::StartEvent => {
            frame.fill(&Path::circle(c, 7.0), colors.fill);
            frame.stroke(&Path::circle(c, 7.0), stroke);
        }
        NodeKind::EndEvent => {
            frame.fill(&Path::circle(c, 7.0), colors.fill);
            frame.stroke(&Path::circle(c, 7.0), stroke.with_width(2.6));
        }
        NodeKind::IntermediateCatch => {
            frame.fill(&Path::circle(c, 7.0), colors.fill);
            frame.stroke(&Path::circle(c, 7.0), stroke);
            frame.stroke(&Path::circle(c, 4.6), stroke);
        }
        NodeKind::IntermediateThrow => {
            frame.fill(&Path::circle(c, 7.0), colors.fill);
            frame.stroke(&Path::circle(c, 7.0), stroke);
            frame.stroke(&Path::circle(c, 4.6), stroke);
            frame.fill(&Path::circle(c, 2.0), colors.stroke);
        }
        NodeKind::Boundary => {
            let mut dashed = stroke;
            dashed.line_dash.segments = &[2.0, 2.0];
            frame.fill(&Path::circle(c, 7.0), colors.fill);
            frame.stroke(&Path::circle(c, 7.0), dashed);
            frame.stroke(&Path::circle(c, 4.6), dashed);
        }
        NodeKind::Task(TaskType::Generic) => rounded(frame, 10.0, 7.0, stroke),
        NodeKind::Task(task_type) => {
            // The type icon is the distinguishing mark; render it centered
            // using the canvas primitives (they draw at bounds + (8, 8)).
            let bounds = Rectangle {
                x: c.x - 15.0,
                y: c.y - 14.5,
                width: 30.0,
                height: 29.0,
            };
            render::task_icon(frame, task_type, bounds, 1.4, tokens.task);
        }
        NodeKind::Gateway(gateway) => {
            let half = 9.0;
            let diamond = Path::new(|builder| {
                builder.move_to(Point::new(c.x, c.y - half));
                builder.line_to(Point::new(c.x + half, c.y));
                builder.line_to(Point::new(c.x, c.y + half));
                builder.line_to(Point::new(c.x - half, c.y));
                builder.close();
            });
            frame.fill(&diamond, colors.fill);
            frame.stroke(&diamond, stroke);
            render::gateway_marker(
                frame,
                gateway,
                gateway == GatewayKind::Exclusive,
                c,
                3.0,
                stroke,
                1.4,
            );
        }
        NodeKind::SubProcess(sub) => {
            match sub {
                SubProcessKind::Embedded => rounded(frame, 10.0, 7.0, stroke),
                SubProcessKind::Event => {
                    let mut dotted = stroke;
                    dotted.line_dash.segments = &[1.6, 2.2];
                    rounded(frame, 10.0, 7.0, dotted);
                }
                SubProcessKind::Transaction => {
                    rounded(frame, 10.0, 7.0, stroke);
                    rounded(frame, 8.0, 5.0, stroke);
                }
            }
            if sub != SubProcessKind::Transaction {
                let plus = Path::new(|builder| {
                    builder.move_to(Point::new(c.x, c.y - 3.0));
                    builder.line_to(Point::new(c.x, c.y + 3.0));
                    builder.move_to(Point::new(c.x - 3.0, c.y));
                    builder.line_to(Point::new(c.x + 3.0, c.y));
                });
                frame.stroke(&plus, stroke);
            }
        }
        NodeKind::CallActivity => rounded(frame, 10.0, 7.0, stroke.with_width(2.4)),
        NodeKind::DataObject => {
            let (half_w, half_h, fold) = (6.0, 8.0, 4.0);
            let page = Path::new(|builder| {
                builder.move_to(Point::new(c.x - half_w, c.y - half_h));
                builder.line_to(Point::new(c.x + half_w - fold, c.y - half_h));
                builder.line_to(Point::new(c.x + half_w, c.y - half_h + fold));
                builder.line_to(Point::new(c.x + half_w, c.y + half_h));
                builder.line_to(Point::new(c.x - half_w, c.y + half_h));
                builder.close();
            });
            frame.fill(&page, colors.fill);
            frame.stroke(&page, stroke);
        }
        NodeKind::DataStore => {
            let (half_w, half_h, ry) = (8.0, 7.0, 2.4);
            let body = Path::new(|builder| {
                builder.move_to(Point::new(c.x - half_w, c.y - half_h + ry));
                builder.quadratic_curve_to(
                    Point::new(c.x, c.y - half_h - ry),
                    Point::new(c.x + half_w, c.y - half_h + ry),
                );
                builder.line_to(Point::new(c.x + half_w, c.y + half_h - ry));
                builder.quadratic_curve_to(
                    Point::new(c.x, c.y + half_h + ry),
                    Point::new(c.x - half_w, c.y + half_h - ry),
                );
                builder.close();
            });
            frame.fill(&body, colors.fill);
            frame.stroke(&body, stroke);
            let lid = Path::new(|builder| {
                builder.move_to(Point::new(c.x - half_w, c.y - half_h + ry));
                builder.quadratic_curve_to(
                    Point::new(c.x, c.y - half_h + ry * 2.4),
                    Point::new(c.x + half_w, c.y - half_h + ry),
                );
            });
            frame.stroke(&lid, stroke);
        }
        NodeKind::TextAnnotation => {
            let bracket = Path::new(|builder| {
                builder.move_to(Point::new(c.x - 2.0, c.y - 7.0));
                builder.line_to(Point::new(c.x - 7.0, c.y - 7.0));
                builder.line_to(Point::new(c.x - 7.0, c.y + 7.0));
                builder.line_to(Point::new(c.x - 2.0, c.y + 7.0));
            });
            frame.stroke(&bracket, stroke);
            let lines = Path::new(|builder| {
                for i in 0..3u8 {
                    let y = c.y - 4.0 + f32::from(i) * 4.0;
                    builder.move_to(Point::new(c.x - 3.0, y));
                    builder.line_to(Point::new(c.x + 7.0, y));
                }
            });
            frame.stroke(&lines, stroke.with_width(1.0));
        }
        NodeKind::Group => {
            let mut dashed = stroke;
            dashed.line_dash.segments = &[4.0, 2.0, 1.0, 2.0];
            rounded(frame, 10.0, 7.0, dashed);
        }
    }
}
