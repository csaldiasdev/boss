//! The MCP server: tool definitions and the streamable-HTTP endpoint.

use crate::bridge::{ApplyOutcome, EditAction, EditorHandle, NodeSpec, UpdatePatch};
use crate::dto;
use bpmn_core::model::{GatewayKind, SubProcessKind, TaskType};
use bpmn_core::{BpmnId, Document, NodeKind};
use rmcp::handler::server::wrapper::{Json, Parameters};
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
        "data-object" => NodeKind::DataObject,
        "data-store" => NodeKind::DataStore,
        "text-annotation" => NodeKind::TextAnnotation,
        "group" => NodeKind::Group,
        "exclusive-gateway" => NodeKind::Gateway(GatewayKind::Exclusive),
        "parallel-gateway" => NodeKind::Gateway(GatewayKind::Parallel),
        "inclusive-gateway" => NodeKind::Gateway(GatewayKind::Inclusive),
        "event-based-gateway" => NodeKind::Gateway(GatewayKind::EventBased),
        "complex-gateway" => NodeKind::Gateway(GatewayKind::Complex),
        _ => return None,
    })
}

fn parse_task_type(value: &str) -> Option<TaskType> {
    Some(match value {
        "task" => TaskType::Generic,
        "user-task" => TaskType::User,
        "service-task" => TaskType::Service,
        "script-task" => TaskType::Script,
        "manual-task" => TaskType::Manual,
        "send-task" => TaskType::Send,
        "receive-task" => TaskType::Receive,
        "business-rule-task" => TaskType::BusinessRule,
        _ => return None,
    })
}

/// `Ok(None)` clears the trigger; `Err` means an unknown word.
fn parse_trigger(value: &str) -> Result<Option<bpmn_core::model::EventDefinitionKind>, ()> {
    use bpmn_core::model::EventDefinitionKind as K;
    Ok(Some(match value {
        "none" => return Ok(None),
        "message" => K::Message,
        "timer" => K::Timer,
        "signal" => K::Signal,
        "error" => K::Error,
        "escalation" => K::Escalation,
        "conditional" => K::Conditional,
        "compensate" => K::Compensate,
        "link" => K::Link,
        "cancel" => K::Cancel,
        "terminate" => K::Terminate,
        _ => return Err(()),
    }))
}

/// `Ok(None)` clears the marker; `Err` means an unknown word.
fn parse_loop(value: &str) -> Result<Option<bpmn_core::model::LoopKind>, ()> {
    use bpmn_core::model::LoopKind as K;
    Ok(Some(match value {
        "none" => return Ok(None),
        "standard" => K::Standard,
        "multi-instance-parallel" => K::MultiInstanceParallel,
        "multi-instance-sequential" => K::MultiInstanceSequential,
        _ => return Err(()),
    }))
}

/// Empty strings clear optional text fields.
fn clearable(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

// ---------------------------------------------------------------------------
// Tool parameter types
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct GetGraphParams {
    /// Limit the graph to the direct contents of this process or subprocess.
    container: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct IdParam {
    /// The element id.
    id: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct CreateNodeParams {
    /// One of: start-event, end-event, intermediate-catch-event,
    /// intermediate-throw-event, task, user-task, service-task, script-task,
    /// manual-task, send-task, receive-task, business-rule-task, subprocess,
    /// event-subprocess, transaction, call-activity, exclusive-gateway,
    /// parallel-gateway, inclusive-gateway, event-based-gateway,
    /// complex-gateway, data-object, data-store, text-annotation, group.
    /// Subprocesses are placed expanded; a node created inside their area
    /// becomes part of the subprocess. Connecting an annotation or data
    /// element to a node makes a dotted association.
    kind: String,
    /// Optional label.
    name: Option<String>,
    /// Diagram x of the center. Omit it (and y) to place automatically.
    x: Option<f64>,
    /// Diagram y of the center.
    y: Option<f64>,
    /// Place right of this node and connect from it automatically.
    after: Option<String>,
    /// Place inside this subprocess (its id).
    container: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct NodeSpecParam {
    /// The element kind (same words as `create_node`).
    kind: String,
    /// Optional label.
    name: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct CreateSequenceParams {
    /// The chain to build, left to right; each node is connected to the next.
    nodes: Vec<NodeSpecParam>,
    /// Start right of this node and connect from it.
    after: Option<String>,
    /// Build inside this subprocess (its id).
    container: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct UpdateElementParams {
    /// The element to patch.
    id: String,
    /// New name; "" clears it.
    name: Option<String>,
    /// New task type: task, user-task, service-task, script-task,
    /// manual-task, send-task, receive-task, business-rule-task.
    task_type: Option<String>,
    /// Event trigger: message, timer, signal, error, escalation, conditional,
    /// compensate, link, cancel, terminate; "none" clears it.
    event_trigger: Option<String>,
    /// The id of the gateway's default flow; "" clears it.
    gateway_default: Option<String>,
    /// A sequence flow's condition expression; "" clears it.
    condition: Option<String>,
    /// Loop marker: standard, multi-instance-parallel,
    /// multi-instance-sequential; "none" clears it.
    #[serde(rename = "loop")]
    loop_marker: Option<String>,
    /// A call activity's target process; "" clears it.
    called_element: Option<String>,
    /// Expand (true) or collapse (false) a subprocess.
    expanded: Option<bool>,
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
struct CreatePoolParams {
    /// Optional pool label (the organization or role it represents).
    name: Option<String>,
    /// Diagram x of the pool center. Pools are 600x250; stack them
    /// vertically ~300 units apart.
    x: f64,
    /// Diagram y of the pool center.
    y: f64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AddLaneParams {
    /// Id of the pool (participant) to extend.
    pool: String,
    /// Optional lane label (a role within the pool).
    name: Option<String>,
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

fn created_id(outcome: &ApplyOutcome) -> Result<String, ErrorData> {
    match outcome {
        ApplyOutcome::Created(id) => Ok(id.as_str().to_string()),
        other => Err(ErrorData::internal_error(
            format!("unexpected outcome: {other:?}"),
            None,
        )),
    }
}

#[tool_router]
impl BossMcp {
    #[tool(
        description = "A bird's-eye view of the open document: processes, pools and lanes, element counts, and validation issue counts.",
        annotations(read_only_hint = true)
    )]
    async fn get_overview(&self) -> Result<Json<dto::Overview>, ErrorData> {
        let document = self.snapshot().await?;
        let issues = bpmn_validate::Validator::with_default_rules().run(&document);
        Ok(Json(dto::overview(&document, &issues)))
    }

    #[tool(
        description = "The document (or one container) as a graph: nodes with their properties and geometry, plus every link (sequence flows, message flows, associations) and lane memberships. Start here to understand the model.",
        annotations(read_only_hint = true)
    )]
    async fn get_graph(
        &self,
        params: Parameters<GetGraphParams>,
    ) -> Result<Json<dto::Graph>, ErrorData> {
        let document = self.snapshot().await?;
        let container = params.0.container.map(|c| BpmnId::new(c.as_str()));
        if let Some(container) = &container
            && !document.flow_element_index().contains_key(container)
            && document.processes().all(|p| p.base.id != *container)
        {
            return Err(refused(format!("no container with id {container}")));
        }
        Ok(Json(dto::graph(&document, container.as_ref())))
    }

    #[tool(
        description = "Everything about one element: properties, container chain, lane, incoming and outgoing flows, boundary events, documentation.",
        annotations(read_only_hint = true)
    )]
    async fn describe_element(
        &self,
        params: Parameters<IdParam>,
    ) -> Result<Json<dto::ElementDetail>, ErrorData> {
        let document = self.snapshot().await?;
        let id = BpmnId::new(params.0.id.as_str());
        dto::describe(&document, &id)
            .map(Json)
            .ok_or_else(|| refused(format!("no element with id {id}")))
    }

    #[tool(
        description = "Run BPMN validation; returns structured issues and counts.",
        annotations(read_only_hint = true)
    )]
    async fn validate(&self) -> Result<Json<dto::ValidationReport>, ErrorData> {
        let document = self.snapshot().await?;
        let issues = bpmn_validate::Validator::with_default_rules().run(&document);
        Ok(Json(dto::validation(issues)))
    }

    #[tool(
        description = "The open document as BPMN 2.0 XML (export and debugging; prefer get_graph for reading the model).",
        annotations(read_only_hint = true)
    )]
    async fn get_xml(&self) -> Result<String, ErrorData> {
        Ok(bpmn_xml::write(&self.snapshot().await?))
    }

    #[tool(
        description = "Replace the open document with this BPMN 2.0 XML (one undoable step; the user sees it immediately)."
    )]
    async fn load_xml(
        &self,
        params: Parameters<LoadXmlParams>,
    ) -> Result<Json<dto::ChangeResult>, ErrorData> {
        let document = bpmn_xml::parse(&params.0.xml)
            .map_err(|e| refused(format!("that XML does not parse as BPMN: {e}")))?;
        let outcome = self
            .apply(EditAction::Replace {
                document: Box::new(document),
            })
            .await?;
        Ok(Json(dto::change_result(&outcome)))
    }

    #[tool(
        description = "Create a flow node; returns its id. Placement, best to worst: `after` a node (lands ~150 units right of it and is connected from it automatically — the result carries the flow id); inside a `container` subprocess; an explicit x/y center; neither (free space right of the content)."
    )]
    async fn create_node(
        &self,
        params: Parameters<CreateNodeParams>,
    ) -> Result<Json<dto::CreateNodeResult>, ErrorData> {
        let p = params.0;
        let kind = parse_kind(&p.kind)
            .ok_or_else(|| refused(format!("unknown element kind {:?}", p.kind)))?;
        let outcome = self
            .apply(EditAction::CreateNode {
                kind,
                x: p.x,
                y: p.y,
                after: p.after.map(|id| BpmnId::new(id.as_str())),
                container: p.container.map(|id| BpmnId::new(id.as_str())),
                name: p.name,
            })
            .await?;
        match outcome {
            ApplyOutcome::Created(id) => Ok(Json(dto::CreateNodeResult {
                id: id.as_str().to_string(),
                flow: None,
            })),
            ApplyOutcome::CreatedMany { nodes, flows } => Ok(Json(dto::CreateNodeResult {
                id: nodes
                    .first()
                    .map(|id| id.as_str().to_string())
                    .unwrap_or_default(),
                flow: flows.first().map(|id| id.as_str().to_string()),
            })),
            other => Err(ErrorData::internal_error(
                format!("unexpected outcome: {other:?}"),
                None,
            )),
        }
    }

    #[tool(
        description = "Create a connected left-to-right chain of nodes in ONE call and one undoable step; returns every node and flow id. The most efficient way to build a process: e.g. start-event, task, exclusive-gateway, task, end-event. With `after`, the chain continues from that node."
    )]
    async fn create_sequence(
        &self,
        params: Parameters<CreateSequenceParams>,
    ) -> Result<Json<dto::CreateSequenceResult>, ErrorData> {
        let p = params.0;
        let mut nodes = Vec::with_capacity(p.nodes.len());
        for spec in p.nodes {
            let kind = parse_kind(&spec.kind)
                .ok_or_else(|| refused(format!("unknown element kind {:?}", spec.kind)))?;
            nodes.push(NodeSpec {
                kind,
                name: spec.name,
            });
        }
        let outcome = self
            .apply(EditAction::CreateSequence {
                nodes,
                after: p.after.map(|id| BpmnId::new(id.as_str())),
                container: p.container.map(|id| BpmnId::new(id.as_str())),
            })
            .await?;
        match outcome {
            ApplyOutcome::CreatedMany { nodes, flows } => Ok(Json(dto::CreateSequenceResult {
                nodes: nodes.iter().map(|id| id.as_str().to_string()).collect(),
                flows: flows.iter().map(|id| id.as_str().to_string()).collect(),
            })),
            ApplyOutcome::Nothing => Ok(Json(dto::CreateSequenceResult {
                nodes: Vec::new(),
                flows: Vec::new(),
            })),
            other => Err(ErrorData::internal_error(
                format!("unexpected outcome: {other:?}"),
                None,
            )),
        }
    }

    #[tool(
        description = "Patch an element's properties; only the fields you pass change, and the result says which applied. Covers renaming, task types, event triggers, gateway default flows, flow conditions, loop markers, call targets, and expanding/collapsing subprocesses.",
        annotations(idempotent_hint = true)
    )]
    async fn update_element(
        &self,
        params: Parameters<UpdateElementParams>,
    ) -> Result<Json<dto::UpdateResult>, ErrorData> {
        let p = params.0;
        let patch = UpdatePatch {
            name: p.name.map(clearable),
            task_type: match p.task_type {
                Some(value) => Some(
                    parse_task_type(&value)
                        .ok_or_else(|| refused(format!("unknown task type {value:?}")))?,
                ),
                None => None,
            },
            event_trigger: match p.event_trigger {
                Some(value) => Some(
                    parse_trigger(&value)
                        .map_err(|()| refused(format!("unknown event trigger {value:?}")))?,
                ),
                None => None,
            },
            gateway_default: p
                .gateway_default
                .map(|value| clearable(value).map(|id| BpmnId::new(id.as_str()))),
            condition: p.condition.map(clearable),
            loop_marker: match p.loop_marker {
                Some(value) => Some(
                    parse_loop(&value)
                        .map_err(|()| refused(format!("unknown loop marker {value:?}")))?,
                ),
                None => None,
            },
            called_element: p.called_element.map(clearable),
            expanded: p.expanded,
        };
        let outcome = self
            .apply(EditAction::Update {
                id: BpmnId::new(p.id.as_str()),
                patch,
            })
            .await?;
        match outcome {
            ApplyOutcome::Updated { applied, failed } => Ok(Json(dto::UpdateResult {
                applied,
                failed: failed
                    .into_iter()
                    .map(|(field, reason)| dto::FieldFailure { field, reason })
                    .collect(),
            })),
            other => Err(ErrorData::internal_error(
                format!("unexpected outcome: {other:?}"),
                None,
            )),
        }
    }

    #[tool(
        description = "Create a pool (participant); returns its id. The first pool wraps the existing process; later pools get their own empty process. Nodes created inside a pool's area join its process."
    )]
    async fn create_pool(
        &self,
        params: Parameters<CreatePoolParams>,
    ) -> Result<Json<dto::CreateResult>, ErrorData> {
        let p = params.0;
        let outcome = self
            .apply(EditAction::CreatePool {
                name: p.name,
                x: p.x,
                y: p.y,
            })
            .await?;
        Ok(Json(dto::CreateResult {
            id: created_id(&outcome)?,
        }))
    }

    #[tool(
        description = "Append a lane to a pool; returns the lane id. The first lane covers the pool; later lanes grow it downward. Nodes belong to the lane under their center."
    )]
    async fn add_lane(
        &self,
        params: Parameters<AddLaneParams>,
    ) -> Result<Json<dto::CreateResult>, ErrorData> {
        let p = params.0;
        let outcome = self
            .apply(EditAction::AddLane {
                pool: BpmnId::new(p.pool.as_str()),
                name: p.name,
            })
            .await?;
        Ok(Json(dto::CreateResult {
            id: created_id(&outcome)?,
        }))
    }

    #[tool(description = "Attach a boundary event to an activity; returns its id.")]
    async fn create_boundary_event(
        &self,
        params: Parameters<CreateBoundaryParams>,
    ) -> Result<Json<dto::CreateResult>, ErrorData> {
        let p = params.0;
        let outcome = self
            .apply(EditAction::CreateBoundary {
                host: BpmnId::new(p.host.as_str()),
                x: p.x,
                y: p.y,
            })
            .await?;
        Ok(Json(dto::CreateResult {
            id: created_id(&outcome)?,
        }))
    }

    #[tool(
        description = "Connect two elements; returns the flow id. Nodes in the same container get a sequence flow; endpoints in different pools (nodes or the pools themselves) get a message flow; annotations and data elements get an association. Nodes in different containers of one process cannot connect."
    )]
    async fn connect(
        &self,
        params: Parameters<ConnectParams>,
    ) -> Result<Json<dto::CreateResult>, ErrorData> {
        let p = params.0;
        let outcome = self
            .apply(EditAction::Connect {
                source: BpmnId::new(p.source.as_str()),
                target: BpmnId::new(p.target.as_str()),
                name: p.name,
            })
            .await?;
        Ok(Json(dto::CreateResult {
            id: created_id(&outcome)?,
        }))
    }

    #[tool(
        description = "Delete elements; sequence flows touching them go too. Unknown ids are reported, not silently accepted.",
        annotations(destructive_hint = true)
    )]
    async fn delete_elements(
        &self,
        params: Parameters<IdsParams>,
    ) -> Result<Json<dto::ChangeResult>, ErrorData> {
        let ids = params
            .0
            .ids
            .iter()
            .map(|s| BpmnId::new(s.as_str()))
            .collect();
        let outcome = self.apply(EditAction::Delete { ids }).await?;
        Ok(Json(dto::change_result(&outcome)))
    }

    #[tool(description = "Move shapes by a delta in diagram units.")]
    async fn move_elements(
        &self,
        params: Parameters<MoveParams>,
    ) -> Result<Json<dto::ChangeResult>, ErrorData> {
        let p = params.0;
        let ids = p.ids.iter().map(|s| BpmnId::new(s.as_str())).collect();
        let outcome = self
            .apply(EditAction::Move {
                ids,
                dx: p.dx,
                dy: p.dy,
            })
            .await?;
        Ok(Json(dto::change_result(&outcome)))
    }

    #[tool(description = "Undo the last change.")]
    async fn undo(&self) -> Result<Json<dto::ChangeResult>, ErrorData> {
        let outcome = self.apply(EditAction::Undo).await?;
        Ok(Json(dto::change_result(&outcome)))
    }

    #[tool(description = "Redo the last undone change.")]
    async fn redo(&self) -> Result<Json<dto::ChangeResult>, ErrorData> {
        let outcome = self.apply(EditAction::Redo).await?;
        Ok(Json(dto::change_result(&outcome)))
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
                "Live control of the boss BPMN editor. Start with get_overview, then \
             get_graph to see the nodes and their links. Build with create_sequence \
             (a whole connected chain in one call) or create_node with `after` \
             (placed right of that node and connected automatically) — you rarely \
             need coordinates. Refine with update_element (triggers, task types, \
             conditions, default flows), branch with connect, then validate. \
             get_xml is for export only. Every write lands on the user's canvas \
             immediately, as one undoable step.",
            )
    }
}

/// Serves the MCP endpoint on `127.0.0.1:{port}` until the process exits.
/// With a `token`, every request must carry `Authorization: Bearer {token}`.
/// The returned future completes only on a bind/serve error.
pub async fn serve(port: u16, handle: EditorHandle, token: Option<String>) -> std::io::Result<()> {
    use axum::response::IntoResponse;

    let service: StreamableHttpService<BossMcp, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(BossMcp::new(handle.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let mut router = axum::Router::new().fallback_service(service);
    if let Some(token) = token {
        let expected = format!("Bearer {token}");
        router = router.layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let expected = expected.clone();
                async move {
                    let authorized = request
                        .headers()
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        == Some(expected.as_str());
                    if authorized {
                        next.run(request).await
                    } else {
                        axum::http::StatusCode::UNAUTHORIZED.into_response()
                    }
                }
            },
        ));
    }
    axum::serve(listener, router).await
}
