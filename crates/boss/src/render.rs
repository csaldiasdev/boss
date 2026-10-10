//! Read-only rendering of a BPMN document onto the canvas (M1).
//!
//! Walks the first diagram's DI elements, resolves each one to its semantic
//! element, and draws it in Slate style. Geometry is pure vector paths; all
//! colors come from [`CanvasTokens`]. DI references outside the modeled subset
//! (subprocesses, pools, ...) render as dashed placeholders.

use crate::theme::CanvasTokens;
use bpmn_core::Document;
use bpmn_core::di::{BpmnLabel, BpmnShape, DiElement};
use bpmn_core::model::{
    ActivityKind, Event as BpmnEvent, EventDefinition, EventPosition, FlowElement, Gateway,
    GatewayKind, TaskType,
};
use iced::alignment::Vertical;
use iced::widget::canvas::path::Builder;
use iced::widget::canvas::{Frame, Path, Stroke, Text};
use iced::{Point, Rectangle, Size, Vector};

use crate::theme;

/// Corner radius of task shapes.
const TASK_RADIUS: f32 = 8.0;

fn pt(p: bpmn_core::Point) -> Point {
    Point::new(p.x as f32, p.y as f32)
}

fn rect(b: bpmn_core::Bounds) -> Rectangle {
    Rectangle {
        x: b.x as f32,
        y: b.y as f32,
        width: b.width as f32,
        height: b.height as f32,
    }
}

/// Draws `document`'s first diagram into an already camera-transformed frame.
pub fn draw(
    frame: &mut Frame,
    document: &Document,
    tokens: CanvasTokens,
    zoom: f32,
    visible: Rectangle,
) {
    let Some(plane) = document.diagram().and_then(|d| d.plane.as_ref()) else {
        return;
    };
    let index = document.flow_element_index();

    // Hairline that never drops under one device pixel.
    let stroke_width = 1.5f32.max(1.0 / zoom);
    let stroke = Stroke::default()
        .with_width(stroke_width)
        .with_color(tokens.stroke);

    // Edges under shapes, labels on top.
    for element in &plane.elements {
        if let DiElement::Edge(edge) = element {
            if edge.waypoints.len() < 2 {
                continue;
            }
            let points: Vec<Point> = edge.waypoints.iter().copied().map(pt).collect();
            if !polyline_box(&points).intersects(&visible) {
                continue;
            }

            let path = Path::new(|builder| {
                builder.move_to(points[0]);
                for point in &points[1..] {
                    builder.line_to(*point);
                }
            });
            frame.stroke(&path, stroke);
            arrowhead(frame, &points, tokens);

            if let Some(FlowElement::SequenceFlow(flow)) = index.get(&edge.bpmn_element).copied()
                && let Some(name) = &flow.name
            {
                let position = edge
                    .label
                    .as_ref()
                    .and_then(|l| l.bounds)
                    .map_or_else(|| midpoint(&points), |b| rect(b).center());
                label(frame, name, position, 11.0, tokens.edge_label);
            }
        }
    }

    for element in &plane.elements {
        if let DiElement::Shape(shape) = element {
            let bounds = rect(shape.bounds);
            if !bounds.intersects(&visible) {
                continue;
            }

            match index.get(&shape.bpmn_element).copied() {
                Some(FlowElement::Event(event)) => {
                    draw_event(frame, event, shape, bounds, stroke, stroke_width, tokens);
                }
                Some(FlowElement::Activity(activity)) => {
                    let path = Path::rounded_rectangle(
                        bounds.position(),
                        bounds.size(),
                        TASK_RADIUS.into(),
                    );
                    frame.fill(&path, tokens.fill);
                    frame.stroke(&path, stroke);

                    let ActivityKind::Task(task_type) = activity.kind;
                    task_icon(frame, task_type, bounds, stroke_width, tokens);

                    if let Some(name) = &activity.node.name {
                        wrapped_label(frame, name, bounds, 12.0, tokens.label);
                    }
                }
                Some(FlowElement::Gateway(gateway)) => {
                    draw_gateway(frame, gateway, shape, bounds, stroke, stroke_width, tokens);
                }
                // Sequence flows have edges, not shapes; anything unmodeled
                // renders as an honest dashed placeholder.
                Some(FlowElement::SequenceFlow(_) | FlowElement::Unknown(_)) | None => {
                    let path = Path::rounded_rectangle(
                        bounds.position(),
                        bounds.size(),
                        TASK_RADIUS.into(),
                    );
                    let mut dashed = stroke;
                    dashed.line_dash.segments = &[6.0, 4.0];
                    frame.stroke(&path, dashed);
                }
            }
        }
    }
}

fn polyline_box(points: &[Point]) -> Rectangle {
    let mut min = points[0];
    let mut max = points[0];
    for p in points {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    }
    Rectangle {
        x: min.x - 10.0,
        y: min.y - 10.0,
        width: max.x - min.x + 20.0,
        height: max.y - min.y + 20.0,
    }
}

fn midpoint(points: &[Point]) -> Point {
    let a = points[points.len() / 2 - 1];
    let b = points[points.len() / 2];
    Point::new(f32::midpoint(a.x, b.x), f32::midpoint(a.y, b.y) - 10.0)
}

fn arrowhead(frame: &mut Frame, points: &[Point], tokens: CanvasTokens) {
    let tip = points[points.len() - 1];
    let from = points[points.len() - 2];
    let (dx, dy) = (tip.x - from.x, tip.y - from.y);
    let len = (dx * dx + dy * dy).sqrt().max(0.001);
    let dir = Vector::new(dx / len, dy / len);
    let perp = Vector::new(-dir.y, dir.x);

    let base = Point::new(tip.x - dir.x * 9.0, tip.y - dir.y * 9.0);
    let path = Path::new(|builder| {
        builder.move_to(tip);
        builder.line_to(Point::new(base.x + perp.x * 3.5, base.y + perp.y * 3.5));
        builder.line_to(Point::new(base.x - perp.x * 3.5, base.y - perp.y * 3.5));
        builder.close();
    });
    frame.fill(&path, tokens.stroke);
}

fn label(frame: &mut Frame, content: &str, center: Point, size: f32, color: iced::Color) {
    frame.fill_text(Text {
        content: content.to_string(),
        position: center,
        color,
        size: size.into(),
        font: theme::SANS,
        align_x: iced::widget::text::Alignment::Center,
        align_y: Vertical::Center,
        ..Text::default()
    });
}

fn wrapped_label(
    frame: &mut Frame,
    content: &str,
    bounds: Rectangle,
    size: f32,
    color: iced::Color,
) {
    frame.fill_text(Text {
        content: content.to_string(),
        position: bounds.center(),
        max_width: bounds.width - 14.0,
        color,
        size: size.into(),
        font: theme::SANS,
        align_x: iced::widget::text::Alignment::Center,
        align_y: Vertical::Center,
        ..Text::default()
    });
}

/// Draws the name of an element whose label sits outside its shape (events,
/// gateways): at the DI label bounds when given, below the shape otherwise.
fn outside_label(
    frame: &mut Frame,
    name: &str,
    di_label: Option<&BpmnLabel>,
    bounds: Rectangle,
    tokens: CanvasTokens,
) {
    let position = di_label.and_then(|l| l.bounds).map_or_else(
        || {
            Point::new(
                bounds.x + bounds.width / 2.0,
                bounds.y + bounds.height + 12.0,
            )
        },
        |b| rect(b).center(),
    );
    label(frame, name, position, 11.0, tokens.edge_label);
}

fn draw_event(
    frame: &mut Frame,
    event: &BpmnEvent,
    shape: &BpmnShape,
    bounds: Rectangle,
    stroke: Stroke<'_>,
    stroke_width: f32,
    tokens: CanvasTokens,
) {
    let center = bounds.center();
    let radius = bounds.width.min(bounds.height) / 2.0;

    let circle = Path::circle(center, radius);
    frame.fill(&circle, tokens.fill);

    match event.position {
        EventPosition::Start { .. } => frame.stroke(&circle, stroke),
        EventPosition::End => frame.stroke(&circle, stroke.with_width(stroke_width * 2.4)),
        EventPosition::IntermediateCatch
        | EventPosition::IntermediateThrow
        | EventPosition::Boundary { .. } => {
            frame.stroke(&circle, stroke);
            frame.stroke(&Path::circle(center, radius - 3.5), stroke);
        }
    }

    for definition in &event.definitions {
        match definition {
            EventDefinition::Message { .. } => envelope(frame, center, stroke_width, tokens),
            EventDefinition::Terminate { .. } => {
                frame.fill(&Path::circle(center, radius - 7.0), tokens.stroke);
            }
            EventDefinition::Other(_) => {}
        }
    }

    if let Some(name) = &event.node.name {
        outside_label(frame, name, shape.label.as_ref(), bounds, tokens);
    }
}

fn draw_gateway(
    frame: &mut Frame,
    gateway: &Gateway,
    shape: &BpmnShape,
    bounds: Rectangle,
    stroke: Stroke<'_>,
    stroke_width: f32,
    tokens: CanvasTokens,
) {
    let center = bounds.center();
    let diamond = Path::new(|builder| {
        builder.move_to(Point::new(center.x, bounds.y));
        builder.line_to(Point::new(bounds.x + bounds.width, center.y));
        builder.line_to(Point::new(center.x, bounds.y + bounds.height));
        builder.line_to(Point::new(bounds.x, center.y));
        builder.close();
    });
    frame.fill(&diamond, tokens.fill);
    frame.stroke(&diamond, stroke);

    let marker_stroke = stroke.with_width(stroke_width * 1.6);
    let r = bounds.width * 0.17;
    match gateway.kind {
        GatewayKind::Exclusive if shape.is_marker_visible == Some(true) => {
            let x = Path::new(|builder| {
                builder.move_to(Point::new(center.x - r, center.y - r));
                builder.line_to(Point::new(center.x + r, center.y + r));
                builder.move_to(Point::new(center.x + r, center.y - r));
                builder.line_to(Point::new(center.x - r, center.y + r));
            });
            frame.stroke(&x, marker_stroke);
        }
        GatewayKind::Parallel => {
            let plus = Path::new(|builder| {
                builder.move_to(Point::new(center.x, center.y - r * 1.3));
                builder.line_to(Point::new(center.x, center.y + r * 1.3));
                builder.move_to(Point::new(center.x - r * 1.3, center.y));
                builder.line_to(Point::new(center.x + r * 1.3, center.y));
            });
            frame.stroke(&plus, marker_stroke);
        }
        _ => {}
    }

    if let Some(name) = &gateway.node.name {
        outside_label(frame, name, shape.label.as_ref(), bounds, tokens);
    }
}

fn envelope(frame: &mut Frame, center: Point, stroke_width: f32, tokens: CanvasTokens) {
    let (half_w, half_h) = (8.0, 5.5);
    let stroke = Stroke::default()
        .with_width(stroke_width * 0.9)
        .with_color(tokens.stroke);

    let body = Path::rectangle(
        Point::new(center.x - half_w, center.y - half_h),
        Size::new(half_w * 2.0, half_h * 2.0),
    );
    let flap = Path::new(|builder| {
        builder.move_to(Point::new(center.x - half_w, center.y - half_h));
        builder.line_to(Point::new(center.x, center.y + 1.0));
        builder.line_to(Point::new(center.x + half_w, center.y - half_h));
    });
    frame.stroke(&body, stroke);
    frame.stroke(&flap, stroke);
}

fn task_icon(
    frame: &mut Frame,
    task_type: TaskType,
    bounds: Rectangle,
    stroke_width: f32,
    tokens: CanvasTokens,
) {
    let stroke = Stroke::default()
        .with_width(stroke_width * 0.9)
        .with_color(tokens.stroke);
    let origin = Point::new(bounds.x + 8.0, bounds.y + 8.0);

    match task_type {
        TaskType::User => {
            // Head + shoulders.
            frame.stroke(
                &Path::circle(Point::new(origin.x + 6.0, origin.y + 4.0), 3.2),
                stroke,
            );
            let shoulders = Path::new(|builder: &mut Builder| {
                builder.move_to(Point::new(origin.x, origin.y + 13.0));
                builder.quadratic_curve_to(
                    Point::new(origin.x + 6.0, origin.y + 6.5),
                    Point::new(origin.x + 12.0, origin.y + 13.0),
                );
            });
            frame.stroke(&shoulders, stroke);
        }
        TaskType::Service => {
            // A plain gear: hub plus radial teeth.
            let hub = Point::new(origin.x + 6.0, origin.y + 6.0);
            frame.stroke(&Path::circle(hub, 3.2), stroke);
            let teeth = Path::new(|builder: &mut Builder| {
                for i in 0..8u8 {
                    let angle = f32::from(i) * std::f32::consts::FRAC_PI_4;
                    let (sin, cos) = angle.sin_cos();
                    builder.move_to(Point::new(hub.x + cos * 4.4, hub.y + sin * 4.4));
                    builder.line_to(Point::new(hub.x + cos * 6.4, hub.y + sin * 6.4));
                }
            });
            frame.stroke(&teeth, stroke);
        }
        TaskType::Send => envelope(
            frame,
            Point::new(origin.x + 7.0, origin.y + 5.5),
            stroke_width,
            tokens,
        ),
        // The remaining icons (script, manual, receive, business rule) land in M4.
        _ => {}
    }
}
