//! The diagram canvas: grid, document rendering, and (since M2) editing
//! interaction — selection, marquee, moving, placing, and connecting.

use crate::camera::Camera;
use crate::render;
use crate::theme::CanvasTokens;
use bpmn_core::di::DiElement;
use bpmn_core::edit::outline_of;
use bpmn_core::{BpmnId, Document, NodeKind};
use iced::mouse;
use iced::widget::canvas::{self, Cache, Canvas, Event, Frame, Geometry, Path, Stroke};
use iced::{Element, Fill, Point, Rectangle, Renderer, Size, Theme, Vector};
use std::collections::HashSet;

/// Minor grid step in world units.
pub const GRID_STEP: f32 = 20.0;
/// Every n-th grid line is a major line.
const MAJOR_EVERY: i64 = 5;
/// Below this zoom the grid is pure noise, so it is hidden.
const GRID_MIN_SCALING: f32 = 0.3;
/// Placement and movement snap to this step (half the grid).
const SNAP_STEP: f32 = 10.0;

/// The active editing tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    /// Select, move, and marquee.
    #[default]
    Select,
    /// Place a new node on click.
    Place(NodeKind),
    /// Connect two nodes with a sequence flow.
    Connect,
}

/// What canvas interaction asks the app to do.
#[derive(Debug, Clone)]
pub enum CanvasMessage {
    /// The view was panned to a new translation.
    Panned(Vector),
    /// The view was zoomed, optionally re-anchoring the translation.
    Zoomed {
        /// New zoom factor.
        scaling: f32,
        /// New translation keeping the point under the cursor fixed.
        translation: Option<Vector>,
    },
    /// Replace the selection with this node.
    SelectOnly(BpmnId),
    /// Toggle this node in the selection (shift-click).
    SelectToggle(BpmnId),
    /// Clear the selection.
    ClearSelection,
    /// Select every node intersecting this world-space rectangle.
    SelectInRect(Rectangle),
    /// Move the selection by this world-space delta (already snapped).
    MoveSelection {
        /// Horizontal delta.
        dx: f32,
        /// Vertical delta.
        dy: f32,
    },
    /// Place a new node of this kind at this (snapped) world position.
    Place(NodeKind, Point),
    /// Connect source to target with a sequence flow.
    Connect(BpmnId, BpmnId),
    /// Abort the armed tool (clicked empty space while connecting).
    CancelTool,
}

/// Retained canvas state owned by the app (drawing caches).
#[derive(Default)]
pub struct DiagramCanvas {
    grid_cache: Cache,
    content_cache: Cache,
}

impl DiagramCanvas {
    /// Builds the canvas widget for this frame.
    pub fn view<'a>(
        &'a self,
        camera: &'a Camera,
        document: &'a Document,
        selection: &'a HashSet<BpmnId>,
        tool: Tool,
        tokens: CanvasTokens,
    ) -> Element<'a, CanvasMessage> {
        Canvas::new(View {
            grid_cache: &self.grid_cache,
            content_cache: &self.content_cache,
            camera,
            document,
            selection,
            tool,
            tokens,
        })
        .width(Fill)
        .height(Fill)
        .into()
    }

    /// Drops cached geometry; call after the camera, theme, or document
    /// changes.
    pub fn invalidate(&mut self) {
        self.grid_cache.clear();
        self.content_cache.clear();
    }
}

struct View<'a> {
    grid_cache: &'a Cache,
    content_cache: &'a Cache,
    camera: &'a Camera,
    document: &'a Document,
    selection: &'a HashSet<BpmnId>,
    tool: Tool,
    tokens: CanvasTokens,
}

/// Transient pointer interaction, owned by the widget between frames.
#[derive(Debug, Clone, Default)]
pub struct CanvasState {
    interaction: Interaction,
    shift: bool,
}

#[derive(Debug, Clone, Default)]
enum Interaction {
    #[default]
    None,
    Panning {
        translation: Vector,
        start: Point,
    },
    /// Dragging selected shapes; `delta` is in world units, snapped.
    Moving {
        start_world: Point,
        delta: Vector,
        moved: bool,
    },
    /// Rubber-band selection, both corners in world units.
    Marquee {
        start: Point,
        current: Point,
    },
    /// Connection in progress from this node; cursor in world units.
    ConnectFrom {
        source: BpmnId,
        cursor: Point,
    },
}

fn snap(value: f32) -> f32 {
    (value / SNAP_STEP).round() * SNAP_STEP
}

fn snap_point(p: Point) -> Point {
    Point::new(snap(p.x), snap(p.y))
}

fn world_rect(a: Point, b: Point) -> Rectangle {
    Rectangle {
        x: a.x.min(b.x),
        y: a.y.min(b.y),
        width: (a.x - b.x).abs(),
        height: (a.y - b.y).abs(),
    }
}

impl View<'_> {
    /// Topmost node under a world-space point, shape-accurate.
    fn hit_node(&self, world: Point) -> Option<BpmnId> {
        let plane = self.document.diagram()?.plane.as_ref()?;
        let index = self.document.flow_element_index();
        let world = bpmn_core::Point::new(f64::from(world.x), f64::from(world.y));

        for element in plane.elements.iter().rev() {
            let DiElement::Shape(shape) = element else {
                continue;
            };
            let Some(outline) = index.get(&shape.bpmn_element).copied().and_then(outline_of) else {
                continue;
            };
            if outline.contains(shape.bounds, world) {
                return Some(shape.bpmn_element.clone());
            }
        }
        None
    }

    fn world(&self, cursor: Point, bounds: Rectangle) -> Point {
        self.camera.project(cursor, bounds.size())
    }
}

impl canvas::Program<CanvasMessage> for View<'_> {
    type State = CanvasState;

    #[expect(
        clippy::too_many_lines,
        reason = "one exhaustive event match reads better than scattered handlers"
    )]
    fn update(
        &self,
        state: &mut CanvasState,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<CanvasMessage>> {
        if let Event::Keyboard(iced::keyboard::Event::ModifiersChanged(modifiers)) = event {
            state.shift = modifiers.shift();
            return None;
        }

        if let Event::Mouse(mouse::Event::ButtonReleased(button)) = event {
            let finished = std::mem::take(&mut state.interaction);
            if *button == mouse::Button::Left {
                match finished {
                    Interaction::Moving { delta, moved, .. } if moved => {
                        return Some(
                            canvas::Action::publish(CanvasMessage::MoveSelection {
                                dx: delta.x,
                                dy: delta.y,
                            })
                            .and_capture(),
                        );
                    }
                    Interaction::Marquee { start, current } => {
                        let rect = world_rect(start, current);
                        let message = if rect.width < 2.0 && rect.height < 2.0 {
                            CanvasMessage::ClearSelection
                        } else {
                            CanvasMessage::SelectInRect(rect)
                        };
                        return Some(canvas::Action::publish(message).and_capture());
                    }
                    // A connect drag stays armed until the second click.
                    Interaction::ConnectFrom { source, cursor } => {
                        state.interaction = Interaction::ConnectFrom { source, cursor };
                    }
                    _ => {}
                }
            }
            return Some(canvas::Action::request_redraw());
        }

        let cursor_position = cursor.position_in(bounds)?;
        let world = self.world(cursor_position, bounds);

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Middle)) => {
                state.interaction = Interaction::Panning {
                    translation: self.camera.translation,
                    start: cursor_position,
                };
                Some(canvas::Action::request_redraw().and_capture())
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => match self.tool {
                Tool::Place(kind) => Some(
                    canvas::Action::publish(CanvasMessage::Place(kind, snap_point(world)))
                        .and_capture(),
                ),
                Tool::Connect => {
                    let hit = self.hit_node(world);
                    match (&state.interaction, hit) {
                        (Interaction::ConnectFrom { source, .. }, Some(target)) => {
                            let source = source.clone();
                            state.interaction = Interaction::None;
                            Some(
                                canvas::Action::publish(CanvasMessage::Connect(source, target))
                                    .and_capture(),
                            )
                        }
                        (_, Some(source)) => {
                            state.interaction = Interaction::ConnectFrom {
                                source,
                                cursor: world,
                            };
                            Some(canvas::Action::request_redraw().and_capture())
                        }
                        (_, None) => {
                            state.interaction = Interaction::None;
                            Some(canvas::Action::publish(CanvasMessage::CancelTool).and_capture())
                        }
                    }
                }
                Tool::Select => {
                    if let Some(id) = self.hit_node(world) {
                        let message = if state.shift {
                            Some(CanvasMessage::SelectToggle(id.clone()))
                        } else if !self.selection.contains(&id) {
                            Some(CanvasMessage::SelectOnly(id.clone()))
                        } else {
                            None
                        };
                        if !state.shift {
                            state.interaction = Interaction::Moving {
                                start_world: world,
                                delta: Vector::new(0.0, 0.0),
                                moved: false,
                            };
                        }
                        Some(
                            message
                                .map_or_else(
                                    canvas::Action::request_redraw,
                                    canvas::Action::publish,
                                )
                                .and_capture(),
                        )
                    } else {
                        state.interaction = Interaction::Marquee {
                            start: world,
                            current: world,
                        };
                        Some(canvas::Action::request_redraw().and_capture())
                    }
                }
            },
            Event::Mouse(mouse::Event::CursorMoved { .. }) => match &mut state.interaction {
                Interaction::Panning { translation, start } => Some(
                    canvas::Action::publish(CanvasMessage::Panned(
                        *translation + (cursor_position - *start) * (1.0 / self.camera.scaling),
                    ))
                    .and_capture(),
                ),
                Interaction::Moving {
                    start_world,
                    delta,
                    moved,
                } => {
                    *delta =
                        Vector::new(snap(world.x - start_world.x), snap(world.y - start_world.y));
                    *moved = *moved || delta.x != 0.0 || delta.y != 0.0;
                    Some(canvas::Action::request_redraw().and_capture())
                }
                Interaction::Marquee { current, .. } => {
                    *current = world;
                    Some(canvas::Action::request_redraw().and_capture())
                }
                Interaction::ConnectFrom { cursor, .. } => {
                    *cursor = world;
                    Some(canvas::Action::request_redraw())
                }
                Interaction::None => None,
            },
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => match *delta {
                mouse::ScrollDelta::Lines { y, .. } | mouse::ScrollDelta::Pixels { y, .. } => {
                    let can_zoom = (y < 0.0 && self.camera.scaling > Camera::MIN_SCALING)
                        || (y > 0.0 && self.camera.scaling < Camera::MAX_SCALING);
                    if can_zoom {
                        let old_scaling = self.camera.scaling;
                        let scaling = (old_scaling * (1.0 + y / 30.0))
                            .clamp(Camera::MIN_SCALING, Camera::MAX_SCALING);
                        let translation =
                            cursor
                                .position_from(bounds.center())
                                .map(|cursor_to_center| {
                                    let factor = scaling - old_scaling;
                                    self.camera.translation
                                        - Vector::new(
                                            cursor_to_center.x * factor
                                                / (old_scaling * old_scaling),
                                            cursor_to_center.y * factor
                                                / (old_scaling * old_scaling),
                                        )
                                });
                        Some(
                            canvas::Action::publish(CanvasMessage::Zoomed {
                                scaling,
                                translation,
                            })
                            .and_capture(),
                        )
                    } else {
                        Some(canvas::Action::capture())
                    }
                }
            },
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &CanvasState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let size = bounds.size();
        let center = Vector::new(size.width / 2.0, size.height / 2.0);
        let with_camera = |frame: &mut Frame, draw: &mut dyn FnMut(&mut Frame)| {
            frame.with_save(|frame| {
                frame.translate(center);
                frame.scale(self.camera.scaling);
                frame.translate(self.camera.translation);
                draw(frame);
            });
        };

        let grid = self.grid_cache.draw(renderer, size, |frame| {
            frame.fill_rectangle(Point::ORIGIN, frame.size(), self.tokens.bg);
            if self.camera.scaling < GRID_MIN_SCALING {
                return;
            }
            with_camera(frame, &mut |frame| self.draw_grid(frame));
        });

        let content = self.content_cache.draw(renderer, size, |frame| {
            with_camera(frame, &mut |frame| {
                let region = self.camera.visible_region(size);
                render::draw(
                    frame,
                    self.document,
                    self.tokens,
                    self.camera.scaling,
                    region,
                );
            });
        });

        // Overlay: selection, previews, ghosts. Never cached.
        let mut overlay = Frame::new(renderer, size);
        with_camera(&mut overlay, &mut |frame| {
            self.draw_overlay(frame, state, cursor, bounds);
        });

        vec![grid, content, overlay.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &CanvasState,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        match (&state.interaction, self.tool) {
            (Interaction::Panning { .. }, _) => mouse::Interaction::Grabbing,
            (Interaction::Moving { .. }, _) => mouse::Interaction::Move,
            (_, Tool::Place(_) | Tool::Connect) if cursor.is_over(bounds) => {
                mouse::Interaction::Crosshair
            }
            _ => mouse::Interaction::default(),
        }
    }
}

impl View<'_> {
    fn draw_grid(&self, frame: &mut Frame) {
        let region = self.camera.visible_region(frame.size());
        let hairline = 1.0 / self.camera.scaling;
        let line_index = |coordinate: f32| (coordinate / GRID_STEP).round() as i64;

        let lines = |major: bool| {
            Path::new(|path| {
                let mut x = (region.x / GRID_STEP).floor() * GRID_STEP;
                while x <= region.x + region.width {
                    if (line_index(x) % MAJOR_EVERY == 0) == major {
                        path.move_to(Point::new(x, region.y));
                        path.line_to(Point::new(x, region.y + region.height));
                    }
                    x += GRID_STEP;
                }
                let mut y = (region.y / GRID_STEP).floor() * GRID_STEP;
                while y <= region.y + region.height {
                    if (line_index(y) % MAJOR_EVERY == 0) == major {
                        path.move_to(Point::new(region.x, y));
                        path.line_to(Point::new(region.x + region.width, y));
                    }
                    y += GRID_STEP;
                }
            })
        };

        frame.stroke(
            &lines(false),
            Stroke::default()
                .with_width(hairline)
                .with_color(self.tokens.grid),
        );
        frame.stroke(
            &lines(true),
            Stroke::default()
                .with_width(hairline)
                .with_color(self.tokens.grid_major),
        );
    }

    fn draw_overlay(
        &self,
        frame: &mut Frame,
        state: &CanvasState,
        cursor: mouse::Cursor,
        bounds: Rectangle,
    ) {
        let zoom = self.camera.scaling;
        let accent = |width: f32| {
            let mut stroke = Stroke::default()
                .with_width(width.max(1.0 / zoom))
                .with_color(self.tokens.selection);
            stroke.line_dash.segments = &[5.0, 4.0];
            stroke
        };

        let move_delta = match &state.interaction {
            Interaction::Moving { delta, moved, .. } if *moved => Some(*delta),
            _ => None,
        };

        // Selection outlines (at the preview position while moving).
        if let Some(plane) = self.document.diagram().and_then(|d| d.plane.as_ref()) {
            for element in &plane.elements {
                let DiElement::Shape(shape) = element else {
                    continue;
                };
                if !self.selection.contains(&shape.bpmn_element) {
                    continue;
                }
                let offset = move_delta.unwrap_or(Vector::new(0.0, 0.0));
                let b = shape.bounds;
                frame.stroke(
                    &Path::rectangle(
                        Point::new(b.x as f32 + offset.x - 4.0, b.y as f32 + offset.y - 4.0),
                        Size::new(b.width as f32 + 8.0, b.height as f32 + 8.0),
                    ),
                    accent(1.4),
                );
            }
        }

        // Marquee rectangle.
        if let Interaction::Marquee { start, current } = &state.interaction {
            let rect = world_rect(*start, *current);
            frame.fill_rectangle(
                rect.position(),
                rect.size(),
                iced::Color {
                    a: 0.08,
                    ..self.tokens.selection
                },
            );
            frame.stroke(&Path::rectangle(rect.position(), rect.size()), accent(1.0));
        }

        // Connection ghost line.
        if let Interaction::ConnectFrom { source, cursor } = &state.interaction
            && let Some((outline, source_bounds)) = self.document.node_outline(source)
        {
            let from = outline.anchor(
                source_bounds,
                bpmn_core::Point::new(f64::from(cursor.x), f64::from(cursor.y)),
            );
            frame.stroke(
                &Path::line(Point::new(from.x as f32, from.y as f32), *cursor),
                accent(1.4),
            );
        }

        // Placement ghost.
        if let Tool::Place(kind) = self.tool
            && let Some(position) = cursor.position_in(bounds)
        {
            let world = snap_point(self.camera.project(position, bounds.size()));
            let size = kind.default_size();
            let (w, h) = (size.width as f32, size.height as f32);
            frame.stroke(
                &Path::rectangle(
                    Point::new(world.x - w / 2.0, world.y - h / 2.0),
                    Size::new(w, h),
                ),
                accent(1.2),
            );
        }
    }
}
