//! The diagram canvas: grid, document rendering, and (since M2) editing
//! interaction — selection, marquee, moving, placing, and connecting.

use crate::camera::Camera;
use crate::render;
use crate::theme::CanvasTokens;
use bpmn_core::di::DiElement;
use bpmn_core::edit::outline_of;
use bpmn_core::{BpmnId, Document, NodeKind};
use iced::advanced::mouse;
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
    /// Place a new pool on click.
    PlacePool,
    /// Connect two nodes: a sequence flow inside a container, a message flow
    /// across pools.
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
    /// Place a new pool centered at this (snapped) world position.
    PlacePool(Point),
    /// Place a boundary event on this host activity, toward this point.
    PlaceBoundary(BpmnId, Point),
    /// Connect source to target with a sequence flow.
    Connect(BpmnId, BpmnId),
    /// Replace an edge's waypoints after a handle drag.
    SetEdgeWaypoints {
        /// The sequence flow whose edge changed.
        id: BpmnId,
        /// The full new waypoint list, in world units.
        points: Vec<Point>,
    },
    /// Double-click on an element: select it and focus its name field.
    EditLabel(BpmnId),
    /// The cursor moved to this diagram position (status-bar readout).
    CursorAt(i32, i32),
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
        badges: &'a std::collections::HashMap<BpmnId, crate::render::Badge>,
        tool: Tool,
        tokens: CanvasTokens,
    ) -> Element<'a, CanvasMessage> {
        Canvas::new(View {
            grid_cache: &self.grid_cache,
            content_cache: &self.content_cache,
            camera,
            document,
            selection,
            badges,
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
    badges: &'a std::collections::HashMap<BpmnId, crate::render::Badge>,
    tool: Tool,
    tokens: CanvasTokens,
}

/// Transient pointer interaction, owned by the widget between frames.
#[derive(Debug, Clone, Default)]
pub struct CanvasState {
    interaction: Interaction,
    shift: bool,
    /// The previous left click, for double-click detection.
    last_click: Option<mouse::Click>,
    /// The last cursor position reported to the status bar.
    last_cursor: Option<(i32, i32)>,
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
    /// Dragging one waypoint of the selected edge; `points` is the working
    /// copy previewed by the overlay.
    DraggingWaypoint {
        flow: BpmnId,
        points: Vec<Point>,
        index: usize,
        moved: bool,
    },
}

fn snap(value: f32) -> f32 {
    (value / SNAP_STEP).round() * SNAP_STEP
}

fn snap_point(p: Point) -> Point {
    Point::new(snap(p.x), snap(p.y))
}

fn distance(a: Point, b: Point) -> f32 {
    (a.x - b.x).hypot(a.y - b.y)
}

/// Distance from `p` to the segment `a`..`b`.
fn dist_to_segment(p: Point, a: Point, b: Point) -> f32 {
    let (abx, aby) = (b.x - a.x, b.y - a.y);
    let len_sq = abx * abx + aby * aby;
    if len_sq == 0.0 {
        return distance(p, a);
    }
    let t = (((p.x - a.x) * abx + (p.y - a.y) * aby) / len_sq).clamp(0.0, 1.0);
    distance(p, Point::new(a.x + abx * t, a.y + aby * t))
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
    /// The node under a world-space point, shape-accurate. When shapes nest
    /// (subprocess children), the smallest hit wins so children stay
    /// selectable inside their container. Pools and lanes only respond on
    /// their 30px name band, keeping their body free for the nodes inside.
    fn hit_node(&self, world: Point) -> Option<BpmnId> {
        const HEADER: f64 = 30.0;

        let plane = self.document.diagram()?.plane.as_ref()?;
        let index = self.document.flow_element_index();
        let hidden = self.document.hidden_by_collapse();
        let world = bpmn_core::Point::new(f64::from(world.x), f64::from(world.y));

        let mut best: Option<(BpmnId, f64)> = None;
        for element in plane.elements.iter().rev() {
            let DiElement::Shape(shape) = element else {
                continue;
            };
            let id = &shape.bpmn_element;
            if hidden.contains(id) {
                continue;
            }
            // Groups are transparent: they respond only near their border so
            // the diagram inside stays clickable.
            if let Some(bpmn_core::model::FlowElement::Artifact(artifact)) = index.get(id).copied()
                && matches!(artifact.kind, bpmn_core::model::ArtifactKind::Group { .. })
            {
                let b = shape.bounds;
                let margin = f64::from(6.0 / self.camera.scaling);
                let outer = bpmn_core::Bounds::new(
                    b.x - margin,
                    b.y - margin,
                    b.width + margin * 2.0,
                    b.height + margin * 2.0,
                );
                let inner = bpmn_core::Bounds::new(
                    b.x + margin,
                    b.y + margin,
                    (b.width - margin * 2.0).max(0.0),
                    (b.height - margin * 2.0).max(0.0),
                );
                if outer.contains(world) && !inner.contains(world) {
                    // A border hit is thin; let it win like a band would.
                    let area = b.width * margin * 4.0;
                    if best.as_ref().is_none_or(|(_, a)| area < *a) {
                        best = Some((id.clone(), area));
                    }
                }
                continue;
            }
            let (hit, area) = match index.get(id).copied().and_then(outline_of) {
                Some(outline) => (
                    outline.contains(shape.bounds, world),
                    shape.bounds.width * shape.bounds.height,
                ),
                None if self.document.participant(id).is_some()
                    || self.document.lane(id).is_some() =>
                {
                    let band = bpmn_core::Bounds::new(
                        shape.bounds.x,
                        shape.bounds.y,
                        HEADER,
                        shape.bounds.height,
                    );
                    // The band is small, so a band hit should not lose to a
                    // node by area; treat it as its own footprint.
                    (band.contains(world), band.width * band.height)
                }
                None => continue,
            };
            if hit && best.as_ref().is_none_or(|(_, a)| area < *a) {
                best = Some((id.clone(), area));
            }
        }
        best.map(|(id, _)| id)
    }

    /// Topmost edge (sequence or message flow) whose polyline passes near a
    /// world-space point.
    fn hit_edge(&self, world: Point) -> Option<BpmnId> {
        let plane = self.document.diagram()?.plane.as_ref()?;
        let index = self.document.flow_element_index();
        let hidden = self.document.hidden_by_collapse();
        let message_flows: std::collections::HashSet<&BpmnId> = self
            .document
            .message_flows()
            .map(|flow| &flow.base.id)
            .collect();
        let tolerance = 6.0 / self.camera.scaling;

        for element in plane.elements.iter().rev() {
            let DiElement::Edge(edge) = element else {
                continue;
            };
            let known = index.contains_key(&edge.bpmn_element)
                || message_flows.contains(&edge.bpmn_element);
            if !known || hidden.contains(&edge.bpmn_element) {
                continue;
            }
            let points: Vec<Point> = edge
                .waypoints
                .iter()
                .map(|p| Point::new(p.x as f32, p.y as f32))
                .collect();
            for pair in points.windows(2) {
                if dist_to_segment(world, pair[0], pair[1]) <= tolerance {
                    return Some(edge.bpmn_element.clone());
                }
            }
        }
        None
    }

    /// The waypoints of a flow's edge, in world units.
    fn edge_points(&self, flow: &BpmnId) -> Option<Vec<Point>> {
        let plane = self.document.diagram()?.plane.as_ref()?;
        plane.elements.iter().find_map(|element| match element {
            DiElement::Edge(edge) if edge.bpmn_element == *flow => Some(
                edge.waypoints
                    .iter()
                    .map(|p| Point::new(p.x as f32, p.y as f32))
                    .collect(),
            ),
            _ => None,
        })
    }

    /// The selected edge, when the selection is exactly one sequence flow or
    /// message flow.
    fn single_selected_edge(&self) -> Option<BpmnId> {
        if self.selection.len() != 1 {
            return None;
        }
        let id = self.selection.iter().next()?;
        let index = self.document.flow_element_index();
        let is_edge = matches!(
            index.get(id),
            Some(bpmn_core::model::FlowElement::SequenceFlow(_))
        ) || self.document.message_flows().any(|m| m.base.id == *id);
        is_edge.then(|| id.clone())
    }

    /// When the press lands on a waypoint or midpoint handle of the selected
    /// edge, starts a waypoint drag and reports it.
    fn grab_waypoint(&self, state: &mut CanvasState, world: Point) -> bool {
        let Some(flow) = self.single_selected_edge() else {
            return false;
        };
        let Some(points) = self.edge_points(&flow) else {
            return false;
        };
        let grab = 7.0 / self.camera.scaling;

        // Interior waypoints first (endpoints stay auto-anchored).
        for (i, point) in points
            .iter()
            .enumerate()
            .skip(1)
            .take(points.len().saturating_sub(2))
        {
            if distance(world, *point) <= grab {
                state.interaction = Interaction::DraggingWaypoint {
                    flow,
                    points,
                    index: i,
                    moved: false,
                };
                return true;
            }
        }
        // Then segment midpoints: grabbing one inserts a new waypoint there.
        for i in 0..points.len() - 1 {
            let mid = Point::new(
                f32::midpoint(points[i].x, points[i + 1].x),
                f32::midpoint(points[i].y, points[i + 1].y),
            );
            if distance(world, mid) <= grab {
                let mut points = points;
                points.insert(i + 1, mid);
                state.interaction = Interaction::DraggingWaypoint {
                    flow,
                    points,
                    index: i + 1,
                    moved: false,
                };
                return true;
            }
        }
        false
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
                    Interaction::DraggingWaypoint {
                        flow,
                        points,
                        moved,
                        ..
                    } if moved => {
                        return Some(
                            canvas::Action::publish(CanvasMessage::SetEdgeWaypoints {
                                id: flow,
                                points,
                            })
                            .and_capture(),
                        );
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
                Tool::Place(NodeKind::Boundary) => {
                    // Boundary events need a host activity under the cursor.
                    self.hit_node(world).map_or_else(
                        || Some(canvas::Action::request_redraw().and_capture()),
                        |host| {
                            Some(
                                canvas::Action::publish(CanvasMessage::PlaceBoundary(host, world))
                                    .and_capture(),
                            )
                        },
                    )
                }
                Tool::Place(kind) => Some(
                    canvas::Action::publish(CanvasMessage::Place(kind, snap_point(world)))
                        .and_capture(),
                ),
                Tool::PlacePool => Some(
                    canvas::Action::publish(CanvasMessage::PlacePool(snap_point(world)))
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
                    let click =
                        mouse::Click::new(cursor_position, mouse::Button::Left, state.last_click);
                    state.last_click = Some(click);
                    if click.kind() == mouse::click::Kind::Double
                        && let Some(id) = self.hit_node(world).or_else(|| self.hit_edge(world))
                    {
                        state.interaction = Interaction::None;
                        return Some(
                            canvas::Action::publish(CanvasMessage::EditLabel(id)).and_capture(),
                        );
                    }
                    if self.grab_waypoint(state, world) {
                        return Some(canvas::Action::request_redraw().and_capture());
                    }
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
                    } else if let Some(id) = self.hit_edge(world) {
                        let message = if state.shift {
                            CanvasMessage::SelectToggle(id)
                        } else {
                            CanvasMessage::SelectOnly(id)
                        };
                        Some(canvas::Action::publish(message).and_capture())
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
                Interaction::DraggingWaypoint {
                    points,
                    index,
                    moved,
                    ..
                } => {
                    points[*index] = snap_point(world);
                    *moved = true;
                    Some(canvas::Action::request_redraw().and_capture())
                }
                Interaction::None => {
                    // Report the diagram position for the status bar, but only
                    // when the rounded coordinate actually changed.
                    let at = (world.x.round() as i32, world.y.round() as i32);
                    if state.last_cursor == Some(at) {
                        None
                    } else {
                        state.last_cursor = Some(at);
                        Some(canvas::Action::publish(CanvasMessage::CursorAt(at.0, at.1)))
                    }
                }
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
                    self.badges,
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
            (_, Tool::Place(_) | Tool::PlacePool | Tool::Connect) if cursor.is_over(bounds) => {
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

    fn stroke_polyline(&self, frame: &mut Frame, points: &[Point], width: f32) {
        let path = Path::new(|builder| {
            builder.move_to(points[0]);
            for point in &points[1..] {
                builder.line_to(*point);
            }
        });
        frame.stroke(
            &path,
            Stroke::default()
                .with_width(width.max(1.0 / self.camera.scaling))
                .with_color(self.tokens.selection),
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

        self.draw_edge_overlay(frame, state);

        // Marquee rectangle, connection ghost, and placement ghost follow.
        self.draw_tool_overlay(frame, state, cursor, bounds, zoom);
    }

    /// Selected-edge highlights plus waypoint/midpoint handles.
    fn draw_edge_overlay(&self, frame: &mut Frame, state: &CanvasState) {
        let zoom = self.camera.scaling;
        // Selected edges: accent polyline; a waypoint drag previews instead.
        let dragging_edge = match &state.interaction {
            Interaction::DraggingWaypoint { flow, .. } => Some(flow.clone()),
            _ => None,
        };
        if let Some(plane) = self.document.diagram().and_then(|d| d.plane.as_ref()) {
            for element in &plane.elements {
                let DiElement::Edge(edge) = element else {
                    continue;
                };
                if !self.selection.contains(&edge.bpmn_element)
                    || dragging_edge.as_ref() == Some(&edge.bpmn_element)
                    || edge.waypoints.len() < 2
                {
                    continue;
                }
                let points: Vec<Point> = edge
                    .waypoints
                    .iter()
                    .map(|p| Point::new(p.x as f32, p.y as f32))
                    .collect();
                self.stroke_polyline(frame, &points, 2.0);
            }
        }

        // Waypoint handles on the single selected edge (live points while
        // dragging), plus midpoint grips for inserting new waypoints.
        let handle_points = match &state.interaction {
            Interaction::DraggingWaypoint { points, .. } => Some(points.clone()),
            _ => self
                .single_selected_edge()
                .and_then(|flow| self.edge_points(&flow)),
        };
        if let Some(points) = handle_points {
            if dragging_edge.is_some() {
                self.stroke_polyline(frame, &points, 2.0);
            }
            let r = 3.5 / zoom.max(0.4);
            for point in points.iter().skip(1).take(points.len().saturating_sub(2)) {
                frame.fill_rectangle(
                    Point::new(point.x - r, point.y - r),
                    Size::new(r * 2.0, r * 2.0),
                    self.tokens.selection,
                );
            }
            for pair in points.windows(2) {
                let mid = Point::new(
                    f32::midpoint(pair[0].x, pair[1].x),
                    f32::midpoint(pair[0].y, pair[1].y),
                );
                frame.stroke(
                    &Path::circle(mid, r * 0.85),
                    Stroke::default()
                        .with_width((1.2 / zoom).max(1.0 / zoom))
                        .with_color(self.tokens.selection),
                );
            }
        }
    }

    /// Marquee rectangle, connection ghost line, and placement ghost.
    fn draw_tool_overlay(
        &self,
        frame: &mut Frame,
        state: &CanvasState,
        cursor: mouse::Cursor,
        bounds: Rectangle,
        zoom: f32,
    ) {
        let accent = |width: f32| {
            let mut stroke = Stroke::default()
                .with_width(width.max(1.0 / zoom))
                .with_color(self.tokens.selection);
            stroke.line_dash.segments = &[5.0, 4.0];
            stroke
        };

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
        let ghost_size = match self.tool {
            Tool::Place(kind) => {
                let size = kind.default_size();
                Some((size.width as f32, size.height as f32))
            }
            Tool::PlacePool => Some((600.0, 250.0)),
            _ => None,
        };
        if let Some((w, h)) = ghost_size
            && let Some(position) = cursor.position_in(bounds)
        {
            let world = snap_point(self.camera.project(position, bounds.size()));
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
