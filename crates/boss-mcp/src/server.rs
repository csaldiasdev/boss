//! The MCP server: tool definitions and the streamable-HTTP endpoint.

use crate::bridge::{ApplyOutcome, EditAction, EditorHandle};
use bpmn_core::model::{
    ActivityKind, EventPosition, FlowElement, GatewayKind, SubProcessKind, TaskType,
};
use bpmn_core::{BpmnId, Document, NodeKind};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerConfig};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use std::sync::Arc;

/// The default port of the MCP endpoint.
pub const DEFAULT_PORT: u16 = 4555;

fn refused(message: String) -> ErrorData {
    ErrorData::invalid_params(message, None)
}

fn kind_label(element: &FlowElement) -> &'static str {
    match element {
        FlowElement::Event(event) => match event.position {
            EventPosition::Start { .. } => "start-event",
            EventPosition::End => "end-event",
            EventPosition::IntermediateCatch => "intermediate-catch-event",
            EventPosition::IntermediateThrow => "intermediate-throw-event",
            EventPosition::Boundary { .. } => "boundary-event",
        },
        FlowElement::Activity(activity) => match &activity.kind {
            ActivityKind::Task(task_type) => match task_type {
                TaskType::Generic => "task",
                TaskType::User => "user-task",
                TaskType::Service => "service-task",
                TaskType::Script => "script-task",
                TaskType::Manual => "manual-task",
                TaskType::Send => "send-task",
                TaskType::Receive => "receive-task",
                TaskType::BusinessRule => "business-rule-task",
            },
            ActivityKind::SubProcess(sub) => match sub.kind {
                SubProcessKind::Embedded => "subprocess",
                SubProcessKind::Event => "event-subprocess",
                SubProcessKind::Transaction => "transaction",
            },
            ActivityKind::CallActivity(_) => "call-activity",
        },
        FlowElement::Gateway(gateway) => match gateway.kind {
            GatewayKind::Exclusive => "exclusive-gateway",
            GatewayKind::Parallel => "parallel-gateway",
            GatewayKind::Inclusive => "inclusive-gateway",
            GatewayKind::EventBased => "event-based-gateway",
            GatewayKind::Complex => "complex-gateway",
        },
        FlowElement::SequenceFlow(_) => "sequence-flow",
        FlowElement::Unknown(_) => "unknown",
    }
}

fn parse_kind(kind: &str) -> Option<NodeKind> {
    Some(match kind {
        "start-event" => NodeKind::StartEvent,
        "end-event" => NodeKind::EndEvent,
        "intermediate-catch-event" => NodeKind::IntermediateCatch,
        "intermediate-throw-event" => NodeKind::IntermediateThrow,
        "task" => NodeKind::Task(TaskType::Generic),
        "user-task" => NodeKind::Task(TaskType::User),
        "service-task" => NodeKind::Task(TaskType::Service),
        "script-task" => NodeKind::Task(TaskType::Script),
        "manual-task" => NodeKind::Task(TaskType::Manual),
        "send-task" => NodeKind::Task(TaskType::Send),
        "receive-task" => NodeKind::Task(TaskType::Receive),
        "business-rule-task" => NodeKind::Task(TaskType::BusinessRule),
        "subprocess" => NodeKind::SubProcess(SubProcessKind::Embedded),
        "event-subprocess" => NodeKind::SubProcess(SubProcessKind::Event),
        "transaction" => NodeKind::SubProcess(SubProcessKind::Transaction),
        "call-activity" => NodeKind::CallActivity,
        "exclusive-gateway" => NodeKind::Gateway(GatewayKind::Exclusive),
        "parallel-gateway" => NodeKind::Gateway(GatewayKind::Parallel),
        "inclusive-gateway" => NodeKind::Gateway(GatewayKind::Inclusive),
        "event-based-gateway" => NodeKind::Gateway(GatewayKind::EventBased),
        "complex-gateway" => NodeKind::Gateway(GatewayKind::Complex),
        _ => return None,
    })
}

#[derive(serde::Serialize)]
struct ElementInfo {
    id: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    y: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
}

fn element_info(document: &Document, element: &FlowElement) -> Option<ElementInfo> {
    let id = element.id()?;
    let center = document.node_outline(id).map(|(_, bounds)| bounds.center());
    let (source, target) = match element {
        FlowElement::SequenceFlow(flow) => (
            Some(flow.source.as_str().to_string()),
            Some(flow.target.as_str().to_string()),
        ),
        _ => (None, None),
    };
    Some(ElementInfo {
        id: id.as_str().to_string(),
        kind: kind_label(element),
        name: element.name().map(str::to_string),
        x: center.map(|c| c.x),
        y: center.map(|c| c.y),
        source,
        target,
    })
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<String, ErrorData> {
    serde_json::to_string_pretty(value)
        .map_err(|e| ErrorData::internal_error(format!("serialization failed: {e}"), None))
}

// ---------------------------------------------------------------------------
// Tool parameter types
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct CreateNodeParams {
    /// One of: start-event, end-event, intermediate-catch-event,
    /// intermediate-throw-event, task, user-task, service-task, script-task,
    /// manual-task, send-task, receive-task, business-rule-task, subprocess,
    /// event-subprocess, transaction, call-activity, exclusive-gateway,
    /// parallel-gateway, inclusive-gateway, event-based-gateway,
    /// complex-gateway. Subprocesses are placed expanded (350x200); a node
    /// created inside their area becomes part of the subprocess.
    kind: String,
    /// Diagram x of the element center (typical diagrams span 150..1200).
    x: f64,
    /// Diagram y of the element center.
    y: f64,
    /// Optional label.
    name: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct CreateBoundaryParams {
    /// Id of the host activity (a task).
    host: String,
    /// Diagram x the boundary event should sit toward.
    x: f64,
    /// Diagram y the boundary event should sit toward.
    y: f64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ConnectParams {
    /// Id of the source node.
    source: String,
    /// Id of the target node.
    target: String,
    /// Optional flow label (e.g. "yes").
    name: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SetNameParams {
    /// Id of the element to rename.
    id: String,
    /// The new name; omit to clear it.
    name: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct IdsParams {
    /// Element ids.
    ids: Vec<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct MoveParams {
    /// Ids of the shapes to move.
    ids: Vec<String>,
    /// Horizontal delta in diagram units.
    dx: f64,
    /// Vertical delta in diagram units.
    dy: f64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct LoadXmlParams {
    /// A complete BPMN 2.0 XML document.
    xml: String,
}

// ---------------------------------------------------------------------------
// The server
// ---------------------------------------------------------------------------

/// The MCP server backing the `boss` editor.
#[derive(Clone)]
pub struct BossMcp {
    handle: EditorHandle,
}

impl BossMcp {
    /// Creates a server speaking to the editor through `handle`.
    pub fn new(handle: EditorHandle) -> Self {
        Self { handle }
    }

    async fn apply(&self, action: EditAction) -> Result<ApplyOutcome, ErrorData> {
        self.handle.apply(action).await.map_err(refused)
    }

    async fn snapshot(&self) -> Result<Document, ErrorData> {
        self.handle.snapshot().await.map_err(refused)
    }
}

fn outcome_text(outcome: &ApplyOutcome) -> String {
    match outcome {
        ApplyOutcome::Created(id) => format!("created {id}"),
        ApplyOutcome::Changed => String::from("done"),
        ApplyOutcome::Nothing => String::from("nothing to do"),
    }
}

#[tool_router]
impl BossMcp {
    #[tool(
        description = "Overview of the open BPMN document: file metadata, element counts, and validation issues."
    )]
    async fn get_overview(&self) -> Result<String, ErrorData> {
        #[derive(serde::Serialize)]
        struct Overview {
            processes: usize,
            elements: usize,
            issues: Vec<String>,
        }

        let document = self.snapshot().await?;
        let issues = bpmn_validate::Validator::with_default_rules().run(&document);
        to_json(&Overview {
            processes: document.processes().count(),
            elements: document.flow_elements().count(),
            issues: issues
                .iter()
                .map(|i| format!("[{:?}] {} ({})", i.severity, i.message, i.rule))
                .collect(),
        })
    }

    #[tool(description = "The open document as BPMN 2.0 XML.")]
    async fn get_xml(&self) -> Result<String, ErrorData> {
        Ok(bpmn_xml::write(&self.snapshot().await?))
    }

    #[tool(
        description = "Replace the open document with this BPMN 2.0 XML (one undoable step; the user sees it immediately)."
    )]
    async fn load_xml(&self, params: Parameters<LoadXmlParams>) -> Result<String, ErrorData> {
        let document = bpmn_xml::parse(&params.0.xml)
            .map_err(|e| refused(format!("that XML does not parse as BPMN: {e}")))?;
        let outcome = self
            .apply(EditAction::Replace {
                document: Box::new(document),
            })
            .await?;
        Ok(outcome_text(&outcome))
    }

    #[tool(
        description = "List every element: id, kind, name, center position, and flow endpoints."
    )]
    async fn list_elements(&self) -> Result<String, ErrorData> {
        let document = self.snapshot().await?;
        let elements: Vec<ElementInfo> = document
            .flow_elements()
            .filter_map(|element| element_info(&document, element))
            .collect();
        to_json(&elements)
    }

    #[tool(
        description = "Create a flow node at a diagram position; returns its id. Place elements ~150 units apart left-to-right."
    )]
    async fn create_node(&self, params: Parameters<CreateNodeParams>) -> Result<String, ErrorData> {
        let p = params.0;
        let kind = parse_kind(&p.kind)
            .ok_or_else(|| refused(format!("unknown element kind {:?}", p.kind)))?;
        let outcome = self
            .apply(EditAction::CreateNode {
                kind,
                x: p.x,
                y: p.y,
                name: p.name,
            })
            .await?;
        Ok(outcome_text(&outcome))
    }

    #[tool(description = "Attach a boundary event to a task; returns its id.")]
    async fn create_boundary_event(
        &self,
        params: Parameters<CreateBoundaryParams>,
    ) -> Result<String, ErrorData> {
        let p = params.0;
        let outcome = self
            .apply(EditAction::CreateBoundary {
                host: BpmnId::new(p.host.as_str()),
                x: p.x,
                y: p.y,
            })
            .await?;
        Ok(outcome_text(&outcome))
    }

    #[tool(
        description = "Connect two nodes with a sequence flow; returns the flow id. Both nodes must live in the same container (the process, or one subprocess)."
    )]
    async fn connect(&self, params: Parameters<ConnectParams>) -> Result<String, ErrorData> {
        let p = params.0;
        let outcome = self
            .apply(EditAction::Connect {
                source: BpmnId::new(p.source.as_str()),
                target: BpmnId::new(p.target.as_str()),
                name: p.name,
            })
            .await?;
        Ok(outcome_text(&outcome))
    }

    #[tool(description = "Rename an element (omit name to clear it).")]
    async fn set_name(&self, params: Parameters<SetNameParams>) -> Result<String, ErrorData> {
        let p = params.0;
        let outcome = self
            .apply(EditAction::SetName {
                id: BpmnId::new(p.id.as_str()),
                name: p.name,
            })
            .await?;
        Ok(outcome_text(&outcome))
    }

    #[tool(description = "Delete elements; sequence flows touching them go too.")]
    async fn delete_elements(&self, params: Parameters<IdsParams>) -> Result<String, ErrorData> {
        let ids = params
            .0
            .ids
            .iter()
            .map(|s| BpmnId::new(s.as_str()))
            .collect();
        let outcome = self.apply(EditAction::Delete { ids }).await?;
        Ok(outcome_text(&outcome))
    }

    #[tool(description = "Move shapes by a delta in diagram units.")]
    async fn move_elements(&self, params: Parameters<MoveParams>) -> Result<String, ErrorData> {
        let p = params.0;
        let ids = p.ids.iter().map(|s| BpmnId::new(s.as_str())).collect();
        let outcome = self
            .apply(EditAction::Move {
                ids,
                dx: p.dx,
                dy: p.dy,
            })
            .await?;
        Ok(outcome_text(&outcome))
    }

    #[tool(description = "Run BPMN validation; returns the issue list.")]
    async fn validate(&self) -> Result<String, ErrorData> {
        let document = self.snapshot().await?;
        let issues: Vec<String> = bpmn_validate::Validator::with_default_rules()
            .run(&document)
            .iter()
            .map(|i| {
                format!(
                    "[{:?}] {} ({}{})",
                    i.severity,
                    i.message,
                    i.rule,
                    i.element
                        .as_ref()
                        .map(|id| format!(" at {id}"))
                        .unwrap_or_default(),
                )
            })
            .collect();
        if issues.is_empty() {
            Ok(String::from(
                "No issues — the document is structurally sound.",
            ))
        } else {
            to_json(&issues)
        }
    }

    #[tool(description = "Undo the last change.")]
    async fn undo(&self) -> Result<String, ErrorData> {
        Ok(outcome_text(&self.apply(EditAction::Undo).await?))
    }

    #[tool(description = "Redo the last undone change.")]
    async fn redo(&self) -> Result<String, ErrorData> {
        Ok(outcome_text(&self.apply(EditAction::Redo).await?))
    }
}

#[tool_handler]
#[allow(clippy::unused_async_trait_impl)] // the macro generates async fns without awaits
impl ServerHandler for BossMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new(
                "boss",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Live control of the boss BPMN editor. Every write lands on the user's \
             canvas immediately and is undoable with the editor's own undo. Build \
             processes left to right: create nodes ~150 diagram units apart, connect \
             them, then validate. Coordinates are BPMN DI units (events are 36x36, \
             tasks 100x80).",
            )
    }
}

/// Serves the MCP endpoint on `127.0.0.1:{port}` until the process exits.
/// The returned future completes only on a bind/serve error.
pub async fn serve(port: u16, handle: EditorHandle) -> std::io::Result<()> {
    let service: StreamableHttpService<BossMcp, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(BossMcp::new(handle.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let router = axum::Router::new().fallback_service(service);
    axum::serve(listener, router).await
}
