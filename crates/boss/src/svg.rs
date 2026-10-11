//! SVG export: a standalone vector snapshot of the diagram.
//!
//! Rebuilds the canvas drawing with SVG primitives using the light Slate
//! tokens, so exports look like the editor. Small per-type icons (task types,
//! event triggers) are left out; outlines, markers, bands, and labels carry
//! the diagram's structure.

use crate::theme::{self, CanvasTokens, Mode};
use bpmn_core::di::{BpmnShape, DiElement};
use bpmn_core::model::{
    Activity, ActivityKind, ArtifactKind, DataKind, EventPosition, FlowElement, GatewayKind,
    SubProcessKind,
};
use bpmn_core::{Bounds, Document};
use std::fmt::Write;

const MARGIN: f64 = 40.0;
const POOL_HEADER: f64 = 30.0;

fn hex(color: iced::Color) -> String {
    #[expect(clippy::cast_sign_loss, reason = "clamped to 0..=1 first")]
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(color.r),
        channel(color.g),
        channel(color.b)
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Renders the document's first diagram as an SVG string.
pub fn export(document: &Document) -> String {
    let tokens = theme::tokens(Mode::Light).canvas;
    let bounds = document
        .content_bounds()
        .unwrap_or_else(|| Bounds::new(0.0, 0.0, 200.0, 100.0));

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{} {} {} {}\" \
         font-family=\"IBM Plex Sans, sans-serif\">\n",
        bounds.x - MARGIN,
        bounds.y - MARGIN,
        bounds.width + MARGIN * 2.0,
        bounds.height + MARGIN * 2.0,
    );
    let _ = writeln!(
        svg,
        "  <rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\" />",
        bounds.x - MARGIN,
        bounds.y - MARGIN,
        bounds.width + MARGIN * 2.0,
        bounds.height + MARGIN * 2.0,
        hex(tokens.bg),
    );

    let Some(plane) = document.diagram().and_then(|d| d.plane.as_ref()) else {
        svg.push_str("</svg>\n");
        return svg;
    };
    let index = document.flow_element_index();
    let hidden = document.hidden_by_collapse();

    // Containers first, then edges, then nodes — the canvas stacking order.
    let mut pools = Vec::new();
    let mut lanes = Vec::new();
    let mut containers = Vec::new();
    let mut nodes = Vec::new();
    for element in &plane.elements {
        if let DiElement::Shape(shape) = element {
            if hidden.contains(&shape.bpmn_element) {
                continue;
            }
            match index.get(&shape.bpmn_element).copied() {
                Some(FlowElement::Activity(Activity {
                    kind: ActivityKind::SubProcess(_),
                    ..
                })) if shape.is_expanded == Some(true) => containers.push(shape),
                None if document.participant(&shape.bpmn_element).is_some() => pools.push(shape),
                None if document.lane(&shape.bpmn_element).is_some() => lanes.push(shape),
                _ => nodes.push(shape),
            }
        }
    }
    containers.sort_by(|a, b| {
        let area = |s: &BpmnShape| s.bounds.width * s.bounds.height;
        area(b).total_cmp(&area(a))
    });
    lanes.sort_by(|a, b| {
        let area = |s: &BpmnShape| s.bounds.width * s.bounds.height;
        area(b).total_cmp(&area(a))
    });

    for shape in &pools {
        let name = document
            .participant(&shape.bpmn_element)
            .and_then(|p| p.name.as_deref());
        band(&mut svg, shape.bounds, name, tokens, true);
    }
    for shape in &lanes {
        let name = document
            .lane(&shape.bpmn_element)
            .and_then(|l| l.name.as_deref());
        band(&mut svg, shape.bounds, name, tokens, false);
    }
    for shape in &containers {
        if let Some(FlowElement::Activity(activity)) = index.get(&shape.bpmn_element).copied() {
            activity_shape(&mut svg, activity, shape, tokens);
        }
    }

    edges(&mut svg, document, tokens);

    for shape in &nodes {
        node_shape(&mut svg, document, &index, shape, tokens);
    }

    svg.push_str("</svg>\n");
    svg
}

fn band(svg: &mut String, b: Bounds, name: Option<&str>, tokens: CanvasTokens, is_pool: bool) {
    let fill = if is_pool {
        hex(tokens.fill)
    } else {
        String::from("none")
    };
    let _ = writeln!(
        svg,
        "  <rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{fill}\" stroke=\"{}\" stroke-width=\"1.5\" />",
        b.x,
        b.y,
        b.width,
        b.height,
        hex(tokens.stroke),
    );
    let _ = writeln!(
        svg,
        "  <line x1=\"{x}\" y1=\"{}\" x2=\"{x}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"1.5\" />",
        b.y,
        b.y + b.height,
        hex(tokens.stroke),
        x = b.x + POOL_HEADER,
    );
    if let Some(name) = name {
        let (cx, cy) = (b.x + POOL_HEADER / 2.0, b.y + b.height / 2.0);
        let _ = writeln!(
            svg,
            "  <text x=\"{cx}\" y=\"{cy}\" font-size=\"12\" fill=\"{}\" text-anchor=\"middle\" \
             dominant-baseline=\"central\" transform=\"rotate(-90 {cx} {cy})\">{}</text>",
            hex(tokens.label),
            escape(name),
        );
    }
}

fn label_below(svg: &mut String, b: Bounds, name: &str, tokens: CanvasTokens) {
    let _ = writeln!(
        svg,
        "  <text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{}\" text-anchor=\"middle\">{}</text>",
        b.x + b.width / 2.0,
        b.y + b.height + 14.0,
        hex(tokens.edge_label),
        escape(name),
    );
}

fn label_centered(svg: &mut String, b: Bounds, name: &str, tokens: CanvasTokens) {
    let _ = writeln!(
        svg,
        "  <text x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{}\" text-anchor=\"middle\" \
         dominant-baseline=\"central\">{}</text>",
        b.x + b.width / 2.0,
        b.y + b.height / 2.0,
        hex(tokens.label),
        escape(name),
    );
}

#[expect(
    clippy::too_many_lines,
    reason = "one SVG fragment per element kind; a table of small drawings"
)]
fn node_shape(
    svg: &mut String,
    document: &Document,
    index: &std::collections::HashMap<&bpmn_core::BpmnId, &FlowElement>,
    shape: &BpmnShape,
    tokens: CanvasTokens,
) {
    let b = shape.bounds;
    let stroke = hex(tokens.stroke);
    let fill = hex(tokens.fill);
    match index.get(&shape.bpmn_element).copied() {
        Some(FlowElement::Event(event)) => {
            let r = b.width.min(b.height) / 2.0;
            let (cx, cy) = (b.x + b.width / 2.0, b.y + b.height / 2.0);
            let width = if event.position == EventPosition::End {
                3.5
            } else {
                1.5
            };
            let _ = writeln!(
                svg,
                "  <circle cx=\"{cx}\" cy=\"{cy}\" r=\"{r}\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"{width}\" />",
            );
            if matches!(
                event.position,
                EventPosition::IntermediateCatch
                    | EventPosition::IntermediateThrow
                    | EventPosition::Boundary { .. }
            ) {
                let _ = writeln!(
                    svg,
                    "  <circle cx=\"{cx}\" cy=\"{cy}\" r=\"{}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.5\" />",
                    r - 3.5,
                );
            }
            if let Some(name) = &event.node.name {
                label_below(svg, b, name, tokens);
            }
        }
        Some(FlowElement::Gateway(gateway)) => {
            let (cx, cy) = (b.x + b.width / 2.0, b.y + b.height / 2.0);
            let _ = writeln!(
                svg,
                "  <polygon points=\"{cx},{} {},{cy} {cx},{} {},{cy}\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"1.5\" />",
                b.y,
                b.x + b.width,
                b.y + b.height,
                b.x,
            );
            let r = b.width * 0.17;
            match gateway.kind {
                GatewayKind::Exclusive => {
                    let _ = writeln!(
                        svg,
                        "  <path d=\"M {} {} L {} {} M {} {} L {} {}\" stroke=\"{stroke}\" stroke-width=\"2.4\" />",
                        cx - r,
                        cy - r,
                        cx + r,
                        cy + r,
                        cx + r,
                        cy - r,
                        cx - r,
                        cy + r,
                    );
                }
                GatewayKind::Parallel => {
                    let n = r * 1.3;
                    let _ = writeln!(
                        svg,
                        "  <path d=\"M {cx} {} L {cx} {} M {} {cy} L {} {cy}\" stroke=\"{stroke}\" stroke-width=\"2.4\" />",
                        cy - n,
                        cy + n,
                        cx - n,
                        cx + n,
                    );
                }
                GatewayKind::Inclusive => {
                    let _ = writeln!(
                        svg,
                        "  <circle cx=\"{cx}\" cy=\"{cy}\" r=\"{}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"2.4\" />",
                        r * 1.1,
                    );
                }
                GatewayKind::EventBased | GatewayKind::Complex => {
                    let _ = writeln!(
                        svg,
                        "  <circle cx=\"{cx}\" cy=\"{cy}\" r=\"{}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.5\" />",
                        r * 1.4,
                    );
                }
            }
            if let Some(name) = &gateway.node.name {
                label_below(svg, b, name, tokens);
            }
        }
        Some(FlowElement::Activity(activity)) => activity_shape(svg, activity, shape, tokens),
        Some(FlowElement::Data(data)) => {
            match data.kind {
                DataKind::Object | DataKind::ObjectReference { .. } => {
                    let fold = (b.width * 0.3).min(14.0);
                    let _ = writeln!(
                        svg,
                        "  <path d=\"M {} {} L {} {} L {} {} L {} {} L {} {} Z M {} {} L {} {} L {} {}\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"1.5\" />",
                        b.x,
                        b.y,
                        b.x + b.width - fold,
                        b.y,
                        b.x + b.width,
                        b.y + fold,
                        b.x + b.width,
                        b.y + b.height,
                        b.x,
                        b.y + b.height,
                        b.x + b.width - fold,
                        b.y,
                        b.x + b.width - fold,
                        b.y + fold,
                        b.x + b.width,
                        b.y + fold,
                    );
                }
                DataKind::StoreReference { .. } => {
                    let ry = (b.height * 0.12).min(10.0);
                    let _ = writeln!(
                        svg,
                        "  <path d=\"M {x0} {top} A {rx} {ry} 0 0 1 {x1} {top} L {x1} {bottom} A {rx} {ry} 0 0 1 {x0} {bottom} Z\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"1.5\" />",
                        x0 = b.x,
                        x1 = b.x + b.width,
                        top = b.y + ry,
                        bottom = b.y + b.height - ry,
                        rx = b.width / 2.0,
                    );
                    let _ = writeln!(
                        svg,
                        "  <path d=\"M {} {} A {} {ry} 0 0 0 {} {}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.5\" />",
                        b.x,
                        b.y + ry,
                        b.width / 2.0,
                        b.x + b.width,
                        b.y + ry,
                    );
                }
            }
            if let Some(name) = &data.name {
                label_below(svg, b, name, tokens);
            }
        }
        Some(FlowElement::Artifact(artifact)) => match &artifact.kind {
            ArtifactKind::TextAnnotation { text } => {
                let _ = writeln!(
                    svg,
                    "  <path d=\"M {} {} L {} {} L {} {} L {} {}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.5\" />",
                    b.x + 10.0,
                    b.y,
                    b.x,
                    b.y,
                    b.x,
                    b.y + b.height,
                    b.x + 10.0,
                    b.y + b.height,
                );
                if let Some(text) = text {
                    let _ = writeln!(
                        svg,
                        "  <text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{}\" dominant-baseline=\"central\">{}</text>",
                        b.x + 7.0,
                        b.y + b.height / 2.0,
                        hex(tokens.label),
                        escape(text),
                    );
                }
            }
            ArtifactKind::Group { .. } => {
                let _ = writeln!(
                    svg,
                    "  <rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"8\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.5\" stroke-dasharray=\"8 4 2 4\" />",
                    b.x, b.y, b.width, b.height,
                );
            }
            ArtifactKind::Association { .. } => {}
        },
        // Unmodeled references keep the canvas's dashed placeholder.
        _ => {
            let _ = writeln!(
                svg,
                "  <rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"8\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.5\" stroke-dasharray=\"6 4\" />",
                b.x, b.y, b.width, b.height,
            );
        }
    }
    // Participants and lanes were drawn as bands; nothing else to add here.
    let _ = document;
}

fn activity_shape(svg: &mut String, activity: &Activity, shape: &BpmnShape, tokens: CanvasTokens) {
    let b = shape.bounds;
    let stroke = hex(tokens.stroke);
    let (dash, width) = match &activity.kind {
        ActivityKind::SubProcess(sub) if sub.kind == SubProcessKind::Event => {
            (" stroke-dasharray=\"2.5 3.5\"", 1.5)
        }
        ActivityKind::CallActivity(_) => ("", 3.3),
        _ => ("", 1.5),
    };
    let _ = writeln!(
        svg,
        "  <rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"8\" fill=\"{}\" stroke=\"{stroke}\" stroke-width=\"{width}\"{dash} />",
        b.x,
        b.y,
        b.width,
        b.height,
        hex(tokens.fill),
    );
    if matches!(
        &activity.kind,
        ActivityKind::SubProcess(sub) if sub.kind == SubProcessKind::Transaction
    ) {
        let _ = writeln!(
            svg,
            "  <rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"6\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.5\" />",
            b.x + 3.0,
            b.y + 3.0,
            b.width - 6.0,
            b.height - 6.0,
        );
    }

    let expanded =
        matches!(&activity.kind, ActivityKind::SubProcess(_)) && shape.is_expanded == Some(true);
    if !expanded
        && matches!(
            &activity.kind,
            ActivityKind::SubProcess(_) | ActivityKind::CallActivity(_)
        )
    {
        // The collapsed-container [+] marker.
        let (cx, cy) = (b.x + b.width / 2.0, b.y + b.height - 9.0);
        let _ = writeln!(
            svg,
            "  <rect x=\"{}\" y=\"{}\" width=\"12\" height=\"12\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.3\" />\n  <path d=\"M {cx} {} L {cx} {} M {} {cy} L {} {cy}\" stroke=\"{stroke}\" stroke-width=\"1.3\" />",
            cx - 6.0,
            cy - 6.0,
            cy - 3.5,
            cy + 3.5,
            cx - 3.5,
            cx + 3.5,
        );
    }

    if let Some(name) = &activity.node.name {
        if expanded {
            let _ = writeln!(
                svg,
                "  <text x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{}\" text-anchor=\"middle\">{}</text>",
                b.x + b.width / 2.0,
                b.y + 18.0,
                hex(tokens.label),
                escape(name),
            );
        } else {
            label_centered(svg, b, name, tokens);
        }
    }
}

fn edges(svg: &mut String, document: &Document, tokens: CanvasTokens) {
    let Some(plane) = document.diagram().and_then(|d| d.plane.as_ref()) else {
        return;
    };
    let index = document.flow_element_index();
    let hidden = document.hidden_by_collapse();
    let message_flows: std::collections::HashMap<&bpmn_core::BpmnId, Option<&str>> = document
        .message_flows()
        .map(|flow| (&flow.base.id, flow.name.as_deref()))
        .collect();
    let stroke = hex(tokens.stroke);

    for element in &plane.elements {
        let DiElement::Edge(edge) = element else {
            continue;
        };
        if edge.waypoints.len() < 2 || hidden.contains(&edge.bpmn_element) {
            continue;
        }
        let points: String = edge
            .waypoints
            .iter()
            .map(|p| format!("{},{}", p.x, p.y))
            .collect::<Vec<_>>()
            .join(" ");
        let mut name = None;

        if let Some(message_name) = message_flows.get(&edge.bpmn_element) {
            let _ = writeln!(
                svg,
                "  <polyline points=\"{points}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.5\" stroke-dasharray=\"6 4\" />",
            );
            let start = edge.waypoints[0];
            let _ = writeln!(
                svg,
                "  <circle cx=\"{}\" cy=\"{}\" r=\"4\" fill=\"{}\" stroke=\"{stroke}\" stroke-width=\"1.5\" />",
                start.x,
                start.y,
                hex(tokens.bg),
            );
            arrowhead(svg, edge, &hex(tokens.bg), &stroke);
            name = *message_name;
        } else if let Some(FlowElement::SequenceFlow(flow)) = index.get(&edge.bpmn_element).copied()
        {
            let _ = writeln!(
                svg,
                "  <polyline points=\"{points}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.5\" />",
            );
            arrowhead(svg, edge, &stroke, &stroke);
            name = flow.name.as_deref();
        } else {
            let _ = writeln!(
                svg,
                "  <polyline points=\"{points}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.5\" stroke-dasharray=\"2 4\" />",
            );
        }

        if let Some(name) = name {
            let n = edge.waypoints.len();
            let (a, z) = (edge.waypoints[n / 2 - 1], edge.waypoints[n / 2]);
            let _ = writeln!(
                svg,
                "  <text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{}\" text-anchor=\"middle\">{}</text>",
                f64::midpoint(a.x, z.x),
                f64::midpoint(a.y, z.y) - 8.0,
                hex(tokens.edge_label),
                escape(name),
            );
        }
    }
}

fn arrowhead(svg: &mut String, edge: &bpmn_core::di::BpmnEdge, fill: &str, stroke: &str) {
    let n = edge.waypoints.len();
    let tip = edge.waypoints[n - 1];
    let from = edge.waypoints[n - 2];
    let (dx, dy) = (tip.x - from.x, tip.y - from.y);
    let len = dx.hypot(dy).max(0.001);
    let (ux, uy) = (dx / len, dy / len);
    let (px, py) = (-uy, ux);
    let (bx, by) = (tip.x - ux * 10.0, tip.y - uy * 10.0);
    let _ = writeln!(
        svg,
        "  <polygon points=\"{},{} {},{} {},{}\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"1\" />",
        tip.x,
        tip.y,
        bx + px * 4.5,
        by + py * 4.5,
        bx - px * 4.5,
        by - py * 4.5,
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn exports_the_collaboration_fixture() {
        let doc = bpmn_xml::parse(include_str!(
            "../../../tests-fixtures/pizza-collaboration.bpmn"
        ))
        .unwrap();
        let svg = super::export(&doc);

        assert!(svg.starts_with("<?xml"));
        assert!(svg.contains("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        // Pools with rotated names, nodes, flows, and message-flow dashes.
        assert!(svg.contains("rotate(-90"));
        assert!(svg.contains("Pizza shop"));
        assert!(svg.contains("<circle"));
        assert!(svg.contains("<polyline"));
        assert!(svg.contains("stroke-dasharray=\"6 4\""));
        // Text is escaped.
        let doc =
            bpmn_xml::parse(include_str!("../../../tests-fixtures/data-artifacts.bpmn")).unwrap();
        assert!(super::export(&doc).contains("applies &amp; audit"));
    }
}
