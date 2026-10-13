//! The application shell: model, messages, update, and view.

use crate::camera::Camera;
use crate::canvas::{CanvasMessage, DiagramCanvas};
use crate::icons;
use crate::platform::{self, OpenedFile, SavedFile};
use crate::session::EditSession;
use crate::theme::{self, Mode, Tokens};
use bpmn_core::di::DiElement;
use bpmn_core::model::{
    ActivityKind, Condition, EventDefinitionKind, EventPosition, FlowElement, GatewayKind,
    LoopKind, SubProcessKind, TaskType,
};
use bpmn_core::{BpmnId, Document, NodeKind, Scope};
use bpmn_validate::{Issue, Severity, Validator};
use iced::widget::{
    button, column, container, pick_list, row, scrollable, space, text, text_editor, text_input,
};
use iced::{Center, Element, Fill, Size, Subscription, Task, Theme, Vector};
use std::collections::HashSet;

/// Re-brands a document as exported by boss before saving: boss becomes the
/// exporter and `targetNamespace` always becomes boss's own — only the
/// ISO/OMG namespaces carry meaning here, never another tool's. Vendor
/// extension content inside the document is ignored but preserved verbatim:
/// that is user data.
fn branded_for_save(document: &Document) -> Document {
    let mut document = document.clone();
    let def = &mut document.definitions;
    def.exporter = Some(String::from("boss"));
    def.exporter_version = Some(String::from(env!("CARGO_PKG_VERSION")));
    def.target_namespace = String::from(bpmn_core::model::BOSS_TARGET_NAMESPACE);
    document
}

/// Whether the DI shape of `id` carries `isExpanded="true"`.
fn shape_is_expanded(document: &Document, id: &BpmnId) -> bool {
    document
        .diagram()
        .and_then(|d| d.plane.as_ref())
        .into_iter()
        .flat_map(|plane| &plane.elements)
        .any(|element| {
            matches!(element, DiElement::Shape(shape)
                if shape.bpmn_element == *id && shape.is_expanded == Some(true))
        })
}

/// The MCP port shown in the UI (the server itself is native-only).
fn boss_mcp_port() -> u16 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        boss_mcp::DEFAULT_PORT
    }
    #[cfg(target_arch = "wasm32")]
    {
        4555
    }
}

/// What the right-click placement catalog can create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placeable {
    /// A flow node, data element, or artifact.
    Node(NodeKind),
    /// A pool.
    Pool,
}

/// A floating element stacked over the canvas, anchored at a window point.
enum CanvasOverlay {
    /// The right-click placement catalog (empty-space click).
    PlaceMenu { at: iced::Point, world: iced::Point },
    /// The element menu (right-click on an element).
    ElementMenu {
        at: iced::Point,
        world: iced::Point,
        id: BpmnId,
    },
    /// The inline name editor (double-click). The element is the single
    /// selection, as everywhere else in the properties path.
    LabelEdit { at: iced::Point },
}

/// The height of the navigation tab bar above the canvas.
const TAB_BAR_HEIGHT: f32 = 34.0;

/// One drilled-in level of the subprocess navigation path.
struct ScopeLevel {
    /// The subprocess whose plane this level edits.
    id: BpmnId,
    /// The camera to restore when navigating back up to the PARENT view;
    /// `None` (a level entered indirectly) zooms the parent to fit instead.
    parent_camera: Option<Camera>,
}

/// A transient message for the status bar.
struct StatusLine {
    text: String,
    is_error: bool,
}

/// A task type with a human label, for the properties pick list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskTypeChoice(TaskType);

impl std::fmt::Display for TaskTypeChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            TaskType::Generic => "Task",
            TaskType::User => "User task",
            TaskType::Service => "Service task",
            TaskType::Script => "Script task",
            TaskType::Manual => "Manual task",
            TaskType::Send => "Send task",
            TaskType::Receive => "Receive task",
            TaskType::BusinessRule => "Business rule task",
        })
    }
}

const TASK_TYPES: [TaskTypeChoice; 8] = [
    TaskTypeChoice(TaskType::Generic),
    TaskTypeChoice(TaskType::User),
    TaskTypeChoice(TaskType::Service),
    TaskTypeChoice(TaskType::Script),
    TaskTypeChoice(TaskType::Manual),
    TaskTypeChoice(TaskType::Send),
    TaskTypeChoice(TaskType::Receive),
    TaskTypeChoice(TaskType::BusinessRule),
];

/// The "(none)" entry in the default-flow pick list.
const NO_DEFAULT: &str = "(none)";

/// An event trigger with a human label, for the properties pick list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TriggerChoice(Option<EventDefinitionKind>);

impl std::fmt::Display for TriggerChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            None => "None",
            Some(EventDefinitionKind::Message) => "Message",
            Some(EventDefinitionKind::Timer) => "Timer",
            Some(EventDefinitionKind::Signal) => "Signal",
            Some(EventDefinitionKind::Error) => "Error",
            Some(EventDefinitionKind::Escalation) => "Escalation",
            Some(EventDefinitionKind::Conditional) => "Conditional",
            Some(EventDefinitionKind::Compensate) => "Compensate",
            Some(EventDefinitionKind::Link) => "Link",
            Some(EventDefinitionKind::Cancel) => "Cancel",
            Some(EventDefinitionKind::Terminate) => "Terminate",
            Some(EventDefinitionKind::Other) => "(vendor-specific)",
        })
    }
}

/// The triggers that make sense for an event position (BPMN 2.0 table 10.x,
/// abridged to what boss can author).
fn trigger_options(position: &EventPosition) -> Vec<TriggerChoice> {
    use EventDefinitionKind as K;
    let kinds: &[K] = match position {
        EventPosition::Start { .. } => &[K::Message, K::Timer, K::Signal, K::Conditional],
        EventPosition::IntermediateCatch => {
            &[K::Message, K::Timer, K::Signal, K::Conditional, K::Link]
        }
        EventPosition::IntermediateThrow => {
            &[K::Message, K::Signal, K::Escalation, K::Link, K::Compensate]
        }
        EventPosition::Boundary { .. } => &[
            K::Message,
            K::Timer,
            K::Signal,
            K::Error,
            K::Escalation,
            K::Conditional,
            K::Compensate,
            K::Cancel,
        ],
        EventPosition::End => &[
            K::Message,
            K::Signal,
            K::Error,
            K::Escalation,
            K::Compensate,
            K::Cancel,
            K::Terminate,
        ],
    };
    std::iter::once(TriggerChoice(None))
        .chain(kinds.iter().map(|k| TriggerChoice(Some(*k))))
        .collect()
}

/// A loop marker with a human label, for the properties pick list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopChoice(Option<LoopKind>);

impl std::fmt::Display for LoopChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            None => "None",
            Some(LoopKind::Standard) => "Loop",
            Some(LoopKind::MultiInstanceParallel) => "Multi-instance (parallel)",
            Some(LoopKind::MultiInstanceSequential) => "Multi-instance (sequential)",
        })
    }
}

const LOOP_CHOICES: [LoopChoice; 4] = [
    LoopChoice(None),
    LoopChoice(Some(LoopKind::Standard)),
    LoopChoice(Some(LoopKind::MultiInstanceParallel)),
    LoopChoice(Some(LoopKind::MultiInstanceSequential)),
];

/// Top-level application state.
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent UI toggles, not a state machine"
)]
pub struct App {
    camera: Camera,
    canvas: DiagramCanvas,
    /// The OS color scheme (kept fresh by `SystemTheme` messages).
    system_mode: Mode,
    /// A manual Light/Dark choice overriding the OS until restart.
    mode_override: Option<Mode>,
    theme: Theme,
    properties_open: bool,
    issues_open: bool,
    /// The cursor position in diagram units, for the status bar.
    cursor_world: Option<(i32, i32)>,
    /// The main window, for chrome actions like dragging.
    window_id: Option<iced::window::Id>,
    /// The floating canvas overlay (menus or the inline name editor).
    overlay: Option<CanvasOverlay>,
    /// The inline name editor's buffer, (re)loaded when the editor opens.
    inline_name: text_editor::Content,
    /// The drill-down path: one level per subprocess the user navigated into.
    scope_path: Vec<ScopeLevel>,
    session: EditSession,
    file_name: String,
    #[cfg(not(target_arch = "wasm32"))]
    path: Option<std::path::PathBuf>,
    selection: HashSet<BpmnId>,
    status: Option<StatusLine>,
    window_size: Size,
    validator: Validator,
    issues: Vec<Issue>,
    badges: std::collections::HashMap<BpmnId, crate::render::Badge>,
    #[cfg(not(target_arch = "wasm32"))]
    mcp_running: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            camera: Camera::default(),
            canvas: DiagramCanvas::default(),
            system_mode: Mode::default(),
            mode_override: None,
            theme: theme::iced_theme(Mode::default()),
            properties_open: true,
            issues_open: true,
            cursor_world: None,
            window_id: None,
            overlay: None,
            inline_name: text_editor::Content::default(),
            scope_path: Vec::new(),
            session: EditSession::new(Document::new()),
            file_name: String::from("untitled.bpmn"),
            #[cfg(not(target_arch = "wasm32"))]
            path: None,
            selection: HashSet::new(),
            status: None,
            window_size: Size::new(1280.0, 800.0),
            validator: Validator::with_default_rules(),
            issues: Vec::new(),
            badges: std::collections::HashMap::new(),
            #[cfg(not(target_arch = "wasm32"))]
            mcp_running: false,
        }
    }
}

/// Everything that can happen in the app.
#[derive(Debug, Clone)]
pub enum Message {
    /// Canvas interaction.
    Canvas(CanvasMessage),
    /// Zoom in from the toolbar.
    ZoomIn,
    /// Zoom out from the toolbar.
    ZoomOut,
    /// Zoom to fit the document.
    ZoomFit,
    /// Switch between light and dark (overrides the OS scheme).
    ToggleTheme,
    /// The OS color scheme was reported or changed.
    SystemTheme(iced::theme::Mode),
    /// The window regained focus (re-checks the OS scheme).
    WindowFocused,
    /// A window opened (Windows attaches its native menu here).
    WindowOpened(iced::window::Id),
    /// Collapse or expand the properties panel.
    ToggleProperties,
    /// Show or hide the validation issues panel.
    ToggleIssues,
    /// Close whatever floats over the canvas (menus, inline editor).
    CloseOverlay,
    /// Place from the context menu at the remembered point.
    ContextPlace(Placeable),
    /// Add a boundary event to the element-menu's element, toward the click.
    AddBoundaryHere,
    /// Open the inline name editor from the element menu.
    RenameHere,
    /// Drill into the element-menu's subprocess.
    ViewSubprocess,
    /// Jump to a level of the drill-down path (0 = the root view).
    NavigateToDepth(usize),
    /// Copy the MCP connect command to the clipboard (native only; the chip
    /// is inert on the web).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    CopyMcpCommand,
    /// Undo one step.
    Undo,
    /// Redo one step.
    Redo,
    /// Delete the selection.
    DeleteSelection,
    /// Escape: disarm the tool, else clear the selection.
    Escape,
    /// An unconsumed keyboard event (mapped to shortcuts).
    Keyboard(iced::keyboard::Event),
    /// The window was resized (drives zoom-to-fit).
    WindowResized(Size),
    /// Select every element.
    SelectAll,
    /// Copy the selection as BPMN XML to the clipboard.
    Copy,
    /// Copy, then delete the selection.
    Cut,
    /// Ask the clipboard for BPMN XML to paste.
    Paste,
    /// The clipboard answered a paste request.
    ClipboardPasted(Option<String>),
    /// The name field of the selected element changed.
    NameEdited(String),
    /// The inline (floating, multi-line) name editor did something.
    InlineNameAction(text_editor::Action),
    /// The task type of the selected activity changed.
    TaskTypeChanged(TaskTypeChoice),
    /// The default flow of the selected gateway changed.
    DefaultFlowChanged(String),
    /// The condition of the selected sequence flow changed.
    ConditionEdited(String),
    /// The trigger of the selected event changed.
    TriggerChanged(TriggerChoice),
    /// The loop marker of the selected activity changed.
    LoopChanged(LoopChoice),
    /// Collapse or expand the selected subprocess.
    ToggleExpanded,
    /// Append a lane to the selected pool.
    AddLane,
    /// Focus the name field of the selected element (F2).
    RenameSelection,
    /// The called element of the selected call activity changed.
    CalledElementEdited(String),
    /// An issue row was clicked: select and center its element.
    IssueClicked(Option<BpmnId>),
    /// Start the MCP server (native only; it runs until the app exits).
    #[cfg(not(target_arch = "wasm32"))]
    McpStart,
    /// A request from an MCP tool handler.
    #[cfg(not(target_arch = "wasm32"))]
    Mcp(crate::mcp_bridge::McpRequest),
    /// Start a fresh document.
    NewRequested,
    /// The user asked to open a file.
    OpenRequested,
    /// The open dialog finished.
    FileOpened(Option<OpenedFile>),
    /// Save to the known path, or ask where (web: download).
    SaveRequested,
    /// Always ask where to save (native only; the web's save is always a
    /// download, so this variant goes unused there).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    SaveAsRequested,
    /// Saving finished.
    Saved(Option<SavedFile>),
    /// Export the diagram as SVG (web: download).
    ExportSvg,
    /// The SVG export finished.
    Exported(Option<SavedFile>),
}

impl App {
    /// Initial state. On native, `--mcp` starts the MCP server immediately
    /// and the first non-flag argument opens as a file.
    pub fn boot() -> (Self, Task<Message>) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut app = Self::default();
            let mut task = Task::none();

            let arguments: Vec<String> = std::env::args().skip(1).collect();
            let mut start_mcp = false;
            let mut token = None;
            let mut index = 0;
            while index < arguments.len() {
                match arguments[index].as_str() {
                    "--mcp" => start_mcp = true,
                    // A token implies the server; agents must send
                    // `Authorization: Bearer <token>`.
                    "--mcp-token" => {
                        index += 1;
                        if let Some(value) = arguments.get(index) {
                            token = Some(value.clone());
                            start_mcp = true;
                        }
                    }
                    argument if !argument.starts_with('-') => {
                        let path = argument.to_string();
                        task = Task::perform(
                            async move { platform::read_path(&path) },
                            Message::FileOpened,
                        );
                    }
                    _ => {}
                }
                index += 1;
            }
            crate::mcp_bridge::set_token(token);
            if start_mcp {
                let _ = app.update(Message::McpStart);
            }
            // The native menu bar (macOS attaches immediately; Windows waits
            // for the window handle via `WindowOpened`).
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            crate::menu::install();
            let task = Task::batch([task, iced::system::theme().map(Message::SystemTheme)]);
            (app, task)
        }
        #[cfg(target_arch = "wasm32")]
        {
            (
                Self::default(),
                iced::system::theme().map(Message::SystemTheme),
            )
        }
    }

    /// Window title: file name plus a dirty marker.
    pub fn title(&self) -> String {
        let dirty = if self.session.is_dirty() { " •" } else { "" };
        format!("boss — {}{dirty}", self.file_name)
    }

    /// The current iced theme (built from the Slate tokens).
    pub fn theme(&self) -> Theme {
        self.theme.clone()
    }

    /// Keyboard shortcuts (events no widget consumed), window resizes, and —
    /// once started — the MCP server.
    pub fn subscription(&self) -> Subscription<Message> {
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let mut subscriptions = vec![
            iced::keyboard::listen().map(Message::Keyboard),
            iced::window::resize_events().map(|(_id, size)| Message::WindowResized(size)),
            iced::system::theme_changes().map(Message::SystemTheme),
            iced::event::listen_with(|event, _status, _id| match event {
                iced::Event::Window(iced::window::Event::Focused) => Some(Message::WindowFocused),
                _ => None,
            }),
            iced::window::open_events().map(Message::WindowOpened),
        ];
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        subscriptions.push(crate::menu::subscription());
        #[cfg(not(target_arch = "wasm32"))]
        if self.mcp_running {
            subscriptions.push(crate::mcp_bridge::subscription().map(Message::Mcp));
        }
        #[cfg(target_arch = "wasm32")]
        let _ = self;
        Subscription::batch(subscriptions)
    }

    fn shortcut(event: &iced::keyboard::Event) -> Option<Message> {
        use iced::keyboard::key::{Key, Named};

        let iced::keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
            return None;
        };
        let command = modifiers.command();
        match key.as_ref() {
            Key::Named(Named::Delete | Named::Backspace) => Some(Message::DeleteSelection),
            Key::Named(Named::Escape) => Some(Message::Escape),
            Key::Named(Named::F2) => Some(Message::RenameSelection),
            Key::Character("z") if command && modifiers.shift() => Some(Message::Redo),
            Key::Character("z") if command => Some(Message::Undo),
            Key::Character("y") if command => Some(Message::Redo),
            Key::Character("s") if command => Some(Message::SaveRequested),
            Key::Character("o") if command => Some(Message::OpenRequested),
            Key::Character("a") if command => Some(Message::SelectAll),
            Key::Character("c") if command => Some(Message::Copy),
            Key::Character("x") if command => Some(Message::Cut),
            Key::Character("v") if command => Some(Message::Paste),
            _ => None,
        }
    }

    /// The mode in effect: the manual override, else the OS scheme.
    fn effective_mode(&self) -> Mode {
        self.mode_override.unwrap_or(self.system_mode)
    }

    fn tokens(&self) -> Tokens {
        theme::tokens(self.effective_mode())
    }

    fn apply_mode(&mut self) {
        self.theme = theme::iced_theme(self.effective_mode());
        self.canvas.invalidate();
    }

    /// Re-runs validation and refreshes the canvas; call after any document
    /// mutation (documents are small — a full pass is microseconds).
    fn document_changed(&mut self) {
        // Undo, delete, or a wholesale replace can invalidate the drill-down
        // path; back out to the deepest level that still exists.
        let document = self.session.document();
        let valid = self
            .scope_path
            .iter()
            .take_while(|level| {
                document
                    .plane_index(&Scope::Subprocess(level.id.clone()))
                    .is_some()
            })
            .count();
        if valid < self.scope_path.len() {
            let restore = self.scope_path[valid].parent_camera;
            self.scope_path.truncate(valid);
            match restore {
                Some(camera) => self.camera = camera,
                None => self.zoom_to_fit(),
            }
        }

        let was_clean = self.issues.is_empty();
        self.issues = self.validator.run(self.session.document());
        if was_clean && !self.issues.is_empty() {
            self.issues_open = true;
        }
        self.badges.clear();
        for issue in &self.issues {
            if let Some(id) = &issue.element {
                let badge = match issue.severity {
                    Severity::Error => crate::render::Badge::Error,
                    Severity::Warning | Severity::Info => crate::render::Badge::Warning,
                };
                let entry = self
                    .badges
                    .entry(id.clone())
                    .or_insert(crate::render::Badge::Warning);
                if badge == crate::render::Badge::Error {
                    *entry = crate::render::Badge::Error;
                }
            }
        }
        self.canvas.invalidate();
    }

    fn status(&mut self, text: impl Into<String>, is_error: bool) {
        self.status = Some(StatusLine {
            text: text.into(),
            is_error,
        });
    }

    /// The width the side panels occupy, honoring the collapsed state.
    fn side_panels_width(&self) -> f32 {
        let properties = if self.properties_open { 230.0 } else { 36.0 };
        properties + 30.0
    }

    /// Centers the camera on the document and picks a zoom that shows all of
    /// it, based on the window size minus the surrounding panels.
    fn zoom_to_fit(&mut self) {
        const MARGIN: f32 = 60.0;

        let Some(bounds) = self.session.document().content_bounds_in(&self.scope()) else {
            self.camera = Camera::default();
            return;
        };

        let panels = self.side_panels_width();
        let viewport = Size::new(
            (self.window_size.width - panels).max(200.0),
            (self.window_size.height - 110.0 - TAB_BAR_HEIGHT).max(200.0),
        );
        let (width, height) = (bounds.width as f32 + MARGIN, bounds.height as f32 + MARGIN);
        let scaling = (viewport.width / width)
            .min(viewport.height / height)
            .clamp(Camera::MIN_SCALING, 1.5);
        let center = bounds.center();

        self.camera = Camera {
            translation: Vector::new(-(center.x as f32), -(center.y as f32)),
            scaling,
        };
    }

    fn save_task(&mut self, always_ask: bool) -> Task<Message> {
        let xml = bpmn_xml::write(&branded_for_save(self.session.document()));

        #[cfg(not(target_arch = "wasm32"))]
        {
            if !always_ask && let Some(path) = self.path.clone() {
                return Task::perform(
                    async move { platform::write_path(&path, &xml) },
                    Message::Saved,
                );
            }
            Task::perform(
                platform::save_file(self.file_name.clone(), xml),
                Message::Saved,
            )
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = always_ask;
            let file_name = self.file_name.clone();
            Task::perform(
                async move { platform::save_file(&file_name, &xml) },
                Message::Saved,
            )
        }
    }

    /// The id of the properties-panel name input, so double-click and F2 can
    /// focus it.
    fn name_input_id() -> iced::advanced::widget::Id {
        iced::advanced::widget::Id::new("element-name")
    }

    /// The id of the inline (floating) name editor.
    fn inline_input_id() -> iced::advanced::widget::Id {
        iced::advanced::widget::Id::new("inline-name")
    }

    /// The display name of any nameable element (flow element, participant,
    /// lane, or message flow).
    fn element_name(&self, id: &BpmnId) -> String {
        let document = self.session.document();
        document
            .flow_element_index()
            .get(id)
            .and_then(|element| element.name())
            .map(str::to_string)
            .or_else(|| {
                document
                    .participant(id)
                    .and_then(|p| p.name.clone())
                    .or_else(|| document.lane(id).and_then(|l| l.name.clone()))
                    .or_else(|| {
                        document
                            .message_flows()
                            .find(|m| m.base.id == *id)
                            .and_then(|m| m.name.clone())
                    })
            })
            .unwrap_or_default()
    }

    /// The scope the canvas currently edits.
    fn scope(&self) -> Scope {
        self.scope_path
            .last()
            .map_or(Scope::Root, |level| Scope::Subprocess(level.id.clone()))
    }

    /// Drills into `id` (a subprocess), making sure it owns a DI plane first.
    fn enter_subprocess(&mut self, id: BpmnId, parent_camera: Option<Camera>) {
        let mut entered = false;
        self.session.commit(|doc| {
            doc.ensure_subprocess_plane(&id)
                .inspect(|_| entered = true)
                .is_some_and(|(_, changed)| changed)
        });
        if !entered {
            self.status("Only subprocesses can be opened", true);
            return;
        }
        self.scope_path.push(ScopeLevel { id, parent_camera });
        self.selection.clear();
        self.overlay = None;
        self.document_changed();
        self.zoom_to_fit();
    }

    /// Jumps to `depth` levels of the drill-down path (0 = root).
    fn navigate_to_depth(&mut self, depth: usize) {
        if depth >= self.scope_path.len() {
            return;
        }
        let restore = self.scope_path[depth].parent_camera;
        self.scope_path.truncate(depth);
        self.selection.clear();
        self.overlay = None;
        match restore {
            Some(camera) => self.camera = camera,
            None => self.zoom_to_fit(),
        }
        self.canvas.invalidate();
    }

    /// Selects `id` and centers the camera on it, first navigating the
    /// drill-down path to wherever the element is actually drawn (collapsed
    /// subprocess ancestors keep their children on their own planes).
    fn reveal(&mut self, id: BpmnId) {
        // The collapsed-subprocess ancestors hiding `id`, outermost first.
        let chain: Vec<BpmnId> = {
            let document = self.session.document();
            let index = document.flow_element_index();
            let mut chain = Vec::new();
            let mut cursor = id.clone();
            while let Some(container) = document.container_of(&cursor) {
                let is_sub = matches!(
                    index.get(&container),
                    Some(FlowElement::Activity(activity))
                        if matches!(activity.kind, ActivityKind::SubProcess(_))
                );
                if !is_sub {
                    break;
                }
                if !shape_is_expanded(document, &container) {
                    chain.push(container.clone());
                }
                cursor = container;
            }
            chain.reverse();
            chain
        };

        let current: Vec<&BpmnId> = self.scope_path.iter().map(|level| &level.id).collect();
        if current != chain.iter().collect::<Vec<_>>() {
            // The camera the root view had, to restore when navigating back.
            let root_camera = self
                .scope_path
                .first()
                .map_or(Some(self.camera), |level| level.parent_camera);
            self.scope_path.clear();
            let mut parent_camera = root_camera;
            for sub in chain {
                self.enter_subprocess(sub, parent_camera);
                parent_camera = None;
            }
        }

        if let Some((_, bounds)) = self.session.document().node_outline(&id) {
            let center = bounds.center();
            self.camera.translation = Vector::new(-(center.x as f32), -(center.y as f32));
        }
        self.selection.clear();
        self.selection.insert(id);
        self.canvas.invalidate();
    }

    /// Opens the floating name editor over the element at `at`.
    fn edit_label(&mut self, id: &BpmnId, at: iced::Point) -> Task<Message> {
        self.selection.clear();
        self.selection.insert(id.clone());
        self.inline_name = text_editor::Content::with_text(&self.element_name(id));
        self.overlay = Some(CanvasOverlay::LabelEdit { at });
        iced::widget::operation::focus(Self::inline_input_id())
    }

    /// Selects an element and moves focus into the properties name field.
    fn focus_properties_name(&mut self, id: BpmnId) -> Task<Message> {
        self.selection.clear();
        self.selection.insert(id);
        iced::widget::operation::focus(Self::name_input_id())
    }

    fn handle_canvas(&mut self, message: CanvasMessage) -> Task<Message> {
        match message {
            CanvasMessage::EditLabel { id, screen } => return self.edit_label(&id, screen),
            CanvasMessage::CursorAt(x, y) => self.cursor_world = Some((x, y)),
            CanvasMessage::ContextMenu { screen, world } => {
                self.overlay = Some(CanvasOverlay::PlaceMenu { at: screen, world });
            }
            CanvasMessage::ElementMenu { id, screen, world } => {
                self.selection.clear();
                self.selection.insert(id.clone());
                self.overlay = Some(CanvasOverlay::ElementMenu {
                    at: screen,
                    world,
                    id,
                });
            }
            CanvasMessage::DragWindow => {
                if let Some(id) = self.window_id {
                    return iced::window::drag(id);
                }
            }
            CanvasMessage::Panned(translation) => {
                self.camera.translation = translation;
                self.canvas.invalidate();
            }
            CanvasMessage::Zoomed {
                scaling,
                translation,
            } => {
                self.camera.scaling = scaling;
                if let Some(translation) = translation {
                    self.camera.translation = translation;
                }
                self.canvas.invalidate();
            }
            CanvasMessage::SelectOnly(_)
            | CanvasMessage::SelectToggle(_)
            | CanvasMessage::ClearSelection
            | CanvasMessage::SelectInRect(_) => self.handle_selection(message),
            CanvasMessage::MoveSelection { dx, dy } => {
                let selection = self.selection.clone();
                self.session.commit(|doc| {
                    doc.move_shapes(&selection, f64::from(dx), f64::from(dy));
                    true
                });
                self.document_changed();
            }

            CanvasMessage::Connect(source, target) => {
                let mut connected = false;
                self.session.commit(|doc| {
                    connected = doc.connect(&source, &target).is_some();
                    connected
                });
                if !connected {
                    self.status("Cannot connect those elements", true);
                }
                self.document_changed();
            }
            CanvasMessage::SetEdgeWaypoints { id, points } => {
                let points: Vec<bpmn_core::Point> = points
                    .iter()
                    .map(|p| bpmn_core::Point::new(f64::from(p.x), f64::from(p.y)))
                    .collect();
                self.session
                    .commit(|doc| doc.set_edge_waypoints(&id, &points));
                self.document_changed();
            }
        }
        Task::none()
    }

    /// Places a node (`Some(kind)`) or a pool (`None`) at a canvas position.
    fn handle_place(&mut self, kind: Option<NodeKind>, at: iced::Point) {
        let center = bpmn_core::Point::new(f64::from(at.x), f64::from(at.y));
        let scope = self.scope();
        if kind.is_none() && scope != Scope::Root {
            self.status("Pools live on the root canvas", true);
            return;
        }
        let mut created = None;
        self.session.commit(|doc| {
            created = match kind {
                Some(kind) => doc.create_node_in(&scope, kind, center),
                None => doc.create_pool(None, center),
            };
            created.is_some()
        });
        match created {
            Some(id) => {
                self.selection.clear();
                self.selection.insert(id);
            }
            None => self.status("In a collaboration, nodes go inside a pool", true),
        }
        self.document_changed();
    }

    fn handle_selection(&mut self, message: CanvasMessage) {
        match message {
            CanvasMessage::SelectOnly(id) => {
                self.selection.clear();
                self.selection.insert(id);
            }
            CanvasMessage::SelectToggle(id) => {
                if !self.selection.remove(&id) {
                    self.selection.insert(id);
                }
            }
            CanvasMessage::ClearSelection => self.selection.clear(),
            CanvasMessage::SelectInRect(rect) => {
                self.selection.clear();
                let world = bpmn_core::Bounds::new(
                    f64::from(rect.x),
                    f64::from(rect.y),
                    f64::from(rect.width),
                    f64::from(rect.height),
                );
                let scope = self.scope();
                let document = self.session.document();
                let index = document.flow_element_index();
                let hidden = document.hidden_by_collapse_in(&scope);
                if let Some(plane) = document.plane_in(&scope) {
                    for element in &plane.elements {
                        if let DiElement::Shape(shape) = element
                            && index.contains_key(&shape.bpmn_element)
                            && !hidden.contains(&shape.bpmn_element)
                            && world.intersects(&shape.bounds)
                        {
                            self.selection.insert(shape.bpmn_element.clone());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The selected element, when the selection is exactly one.
    fn single_selection(&self) -> Option<BpmnId> {
        (self.selection.len() == 1)
            .then(|| self.selection.iter().next().cloned())
            .flatten()
    }

    fn handle_property(&mut self, message: Message) {
        let Some(id) = self.single_selection() else {
            return;
        };
        match message {
            Message::NameEdited(name) => {
                let name = (!name.is_empty()).then_some(name);
                self.session
                    .commit_merged(format!("name:{id}"), |doc| doc.set_name(&id, name));
            }
            Message::TaskTypeChanged(choice) => {
                self.session.commit(|doc| doc.set_task_type(&id, choice.0));
            }
            Message::DefaultFlowChanged(flow) => {
                let default = (flow != NO_DEFAULT).then(|| BpmnId::new(flow.as_str()));
                self.session
                    .commit(|doc| doc.set_gateway_default(&id, default));
            }
            Message::ConditionEdited(text) => {
                let text = (!text.is_empty()).then_some(text);
                self.session
                    .commit_merged(format!("condition:{id}"), |doc| {
                        doc.set_condition(&id, text)
                    });
            }
            Message::TriggerChanged(choice) => {
                self.session
                    .commit(|doc| doc.set_event_definition(&id, choice.0));
            }
            Message::LoopChanged(choice) => {
                self.session
                    .commit(|doc| doc.set_loop_marker(&id, choice.0));
            }
            Message::ToggleExpanded => {
                let expanded = shape_is_expanded(self.session.document(), &id);
                self.session
                    .commit(|doc| doc.set_subprocess_expanded(&id, !expanded));
            }
            Message::CalledElementEdited(called) => {
                let called = (!called.is_empty()).then_some(called);
                self.session.commit_merged(format!("called:{id}"), |doc| {
                    doc.set_called_element(&id, called)
                });
            }
            Message::AddLane => {
                self.session.commit(|doc| doc.add_lane(&id, None).is_some());
            }
            _ => {}
        }
        self.document_changed();
    }

    /// Window chrome and appearance: theme, panels, and native-shell glue.
    fn handle_chrome(&mut self, message: &Message) -> Task<Message> {
        match *message {
            Message::ToggleTheme => {
                self.mode_override = Some(self.effective_mode().toggled());
                self.apply_mode();
            }
            Message::SystemTheme(mode) => {
                self.system_mode = if mode == iced::theme::Mode::Dark {
                    Mode::Dark
                } else {
                    Mode::Light
                };
                if self.mode_override.is_none() {
                    self.apply_mode();
                }
            }
            // macOS can suppress live theme events once a custom theme forces
            // the window appearance; re-query whenever the window regains
            // focus so an OS theme change still lands.
            Message::WindowFocused => {
                return iced::system::theme().map(Message::SystemTheme);
            }
            Message::WindowOpened(id) => {
                self.window_id = Some(id);
                // The native menu attaches once the app is fully launched:
                // earlier, winit's own default macOS menu would replace it.
                #[cfg(target_os = "macos")]
                crate::menu::attach();
                #[cfg(target_os = "windows")]
                return iced::window::run(id, |window| crate::menu::attach_to_window(window))
                    .discard();
                #[cfg(not(target_os = "windows"))]
                let _ = id;
            }
            Message::WindowResized(size) => self.window_size = size,
            Message::ToggleProperties => self.properties_open = !self.properties_open,
            Message::ToggleIssues => self.issues_open = !self.issues_open,
            Message::CopyMcpCommand => {
                self.status("Copied the MCP connect command", false);
                return iced::clipboard::write(format!(
                    "claude mcp add --transport http boss http://127.0.0.1:{}/",
                    boss_mcp_port()
                ));
            }
            _ => {}
        }
        Task::none()
    }

    /// Handles a message.
    #[expect(
        clippy::too_many_lines,
        reason = "one exhaustive dispatch; the bodies live in handle_* helpers"
    )]
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Canvas(canvas_message) => return self.handle_canvas(canvas_message),
            message @ (Message::ZoomIn | Message::ZoomOut) => {
                let factor = if matches!(message, Message::ZoomIn) {
                    1.2
                } else {
                    1.0 / 1.2
                };
                self.camera = self.camera.zoomed(factor);
                self.canvas.invalidate();
            }
            Message::ZoomFit => {
                self.zoom_to_fit();
                self.canvas.invalidate();
            }
            ref message @ (Message::ToggleTheme
            | Message::SystemTheme(_)
            | Message::WindowFocused
            | Message::WindowOpened(_)
            | Message::WindowResized(_)
            | Message::ToggleProperties
            | Message::ToggleIssues
            | Message::CopyMcpCommand) => return self.handle_chrome(message),
            message @ (Message::Undo | Message::Redo) => {
                let stepped = if matches!(message, Message::Undo) {
                    self.session.undo()
                } else {
                    self.session.redo()
                };
                if stepped {
                    self.selection.clear();
                    self.document_changed();
                }
            }
            Message::DeleteSelection => {
                if !self.selection.is_empty() {
                    let doomed = self.selection.clone();
                    self.session.commit(|doc| {
                        doc.delete_elements(&doomed);
                        true
                    });
                    self.selection.clear();
                    self.document_changed();
                }
            }
            Message::Keyboard(event) => {
                if let Some(message) = Self::shortcut(&event) {
                    return self.update(message);
                }
            }
            Message::CloseOverlay => self.overlay = None,
            Message::ContextPlace(placeable) => {
                if let Some(CanvasOverlay::PlaceMenu { world, .. }) = self.overlay.take() {
                    match placeable {
                        Placeable::Node(kind) => self.handle_place(Some(kind), world),
                        Placeable::Pool => self.handle_place(None, world),
                    }
                }
            }
            Message::AddBoundaryHere => {
                if let Some(CanvasOverlay::ElementMenu { world, id, .. }) = self.overlay.take() {
                    let mut created = None;
                    self.session.commit(|doc| {
                        created = doc.create_boundary_event(
                            &id,
                            bpmn_core::Point::new(f64::from(world.x), f64::from(world.y)),
                        );
                        created.is_some()
                    });
                    match created {
                        Some(boundary) => {
                            self.selection.clear();
                            self.selection.insert(boundary);
                        }
                        None => self.status("Boundary events attach to activities", true),
                    }
                    self.document_changed();
                }
            }
            Message::RenameHere => {
                if let Some(CanvasOverlay::ElementMenu { at, id, .. }) = self.overlay.take() {
                    return self.edit_label(&id, at);
                }
            }
            Message::ViewSubprocess => {
                if let Some(CanvasOverlay::ElementMenu { id, .. }) = self.overlay.take() {
                    let camera = self.camera;
                    self.enter_subprocess(id, Some(camera));
                }
            }
            Message::NavigateToDepth(depth) => self.navigate_to_depth(depth),
            Message::Escape => {
                if self.overlay.take().is_some() {
                    return Task::none();
                }
                self.selection.clear();
            }
            Message::RenameSelection => {
                if let Some(id) = self.single_selection() {
                    return self.focus_properties_name(id);
                }
            }
            Message::SelectAll => {
                let scope = self.scope();
                let document = self.session.document();
                let hidden = document.hidden_by_collapse_in(&scope);
                let index = document.flow_element_index();
                self.selection = document
                    .plane_in(&scope)
                    .map(|plane| {
                        plane
                            .elements
                            .iter()
                            .filter_map(DiElement::bpmn_element)
                            .filter(|id| index.contains_key(*id) && !hidden.contains(*id))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
            }
            message @ (Message::Copy
            | Message::Cut
            | Message::Paste
            | Message::ClipboardPasted(_)) => return self.handle_clipboard(message),
            message @ (Message::NameEdited(_)
            | Message::TaskTypeChanged(_)
            | Message::DefaultFlowChanged(_)
            | Message::ConditionEdited(_)
            | Message::TriggerChanged(_)
            | Message::LoopChanged(_)
            | Message::ToggleExpanded
            | Message::AddLane
            | Message::CalledElementEdited(_)) => self.handle_property(message),
            Message::InlineNameAction(action) => {
                let edited = action.is_edit();
                self.inline_name.perform(action);
                // Only edits commit; cursor motion must not touch the session.
                if edited {
                    let name = self.inline_name.text().replace("\r\n", "\n");
                    self.handle_property(Message::NameEdited(name));
                }
            }
            Message::IssueClicked(Some(id)) => self.reveal(id),
            Message::IssueClicked(None) => {}
            #[cfg(not(target_arch = "wasm32"))]
            message @ (Message::McpStart | Message::Mcp(_)) => self.handle_mcp_message(message),
            message => return self.handle_file(message),
        }

        Task::none()
    }

    /// Handles the MCP message family.
    #[cfg(not(target_arch = "wasm32"))]
    fn handle_mcp_message(&mut self, message: Message) {
        match message {
            Message::McpStart => {
                self.mcp_running = true;
                let auth = if crate::mcp_bridge::token_required() {
                    " (Authorization: Bearer token required)"
                } else {
                    ""
                };
                self.status(
                    format!(
                        "MCP server listening on http://127.0.0.1:{port}{auth} — connect with: \
                         claude mcp add --transport http boss http://127.0.0.1:{port}/",
                        port = boss_mcp::DEFAULT_PORT
                    ),
                    false,
                );
            }
            Message::Mcp(request) => {
                if let Some(request) = request.take() {
                    self.handle_mcp(request);
                }
            }
            _ => {}
        }
    }

    /// Services one request from an MCP tool handler. Reads answer with a
    /// snapshot; writes go through the same edit session as user input.
    #[cfg(not(target_arch = "wasm32"))]
    fn handle_mcp(&mut self, request: boss_mcp::EditorRequest) {
        use boss_mcp::EditorRequest;
        match request {
            EditorRequest::Snapshot { reply } => {
                let _ = reply.send(self.session.document().clone());
            }
            EditorRequest::Apply { action, reply } => {
                let summary = format!("{action:?}");
                let outcome = self.apply_mcp_action(action);
                if let Ok(outcome) = &outcome {
                    if !matches!(outcome, boss_mcp::ApplyOutcome::Nothing) {
                        self.document_changed();
                        let summary: String = summary.chars().take(80).collect();
                        self.status(format!("MCP agent: {summary}"), false);
                    }
                }
                let _ = reply.send(outcome);
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    /// Applies one MCP action: history steps go to the edit session, every
    /// other action runs through the shared `boss_mcp::apply` inside one
    /// undoable commit. UI side effects (selection, camera) stay here.
    #[cfg(not(target_arch = "wasm32"))]
    fn apply_mcp_action(
        &mut self,
        action: boss_mcp::EditAction,
    ) -> Result<boss_mcp::ApplyOutcome, String> {
        use boss_mcp::{ApplyOutcome, EditAction};

        if action.is_history() {
            let stepped = match action {
                EditAction::Undo => self.session.undo(),
                _ => self.session.redo(),
            };
            if stepped {
                self.selection.clear();
                return Ok(ApplyOutcome::Changed);
            }
            return Ok(ApplyOutcome::Nothing);
        }

        let is_replace = matches!(action, EditAction::Replace { .. });
        let is_delete = matches!(action, EditAction::Delete { .. });
        let mut result = Err(String::from("not applied"));
        self.session.commit(|doc| {
            result = boss_mcp::apply(action, doc);
            result.as_ref().is_ok_and(ApplyOutcome::changed_document)
        });
        if result.as_ref().is_ok_and(ApplyOutcome::changed_document) {
            if is_delete || is_replace {
                self.selection.clear();
            }
            if is_replace {
                self.zoom_to_fit();
            }
        }
        result
    }

    /// Handles the clipboard message family (BPMN XML as the payload).
    fn handle_clipboard(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Copy => {
                if !self.selection.is_empty() {
                    let xml = bpmn_xml::write(&self.session.document().extract(&self.selection));
                    return iced::clipboard::write(xml);
                }
            }
            Message::Cut => {
                if !self.selection.is_empty() {
                    let xml = bpmn_xml::write(&self.session.document().extract(&self.selection));
                    let doomed = self.selection.clone();
                    self.session.commit(|doc| {
                        doc.delete_elements(&doomed);
                        true
                    });
                    self.selection.clear();
                    self.document_changed();
                    return iced::clipboard::write(xml);
                }
            }
            Message::Paste => {
                return iced::clipboard::read().map(Message::ClipboardPasted);
            }
            Message::ClipboardPasted(Some(contents)) => {
                if let Ok(source) = bpmn_xml::parse(&contents) {
                    let scope = self.scope();
                    let mut pasted = Vec::new();
                    self.session.commit(|doc| {
                        pasted = doc.paste_from_in(&scope, &source, 20.0, 20.0);
                        !pasted.is_empty()
                    });
                    if pasted.is_empty() {
                        self.status("Nothing pastable on the clipboard", true);
                    } else {
                        self.selection = pasted.into_iter().collect();
                        self.document_changed();
                    }
                } else {
                    self.status("Clipboard does not hold BPMN XML", true);
                }
            }
            _ => {}
        }
        Task::none()
    }

    /// Handles the new/open/save message family.
    fn handle_file(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::NewRequested => {
                if self.session.is_dirty() {
                    self.status("Unsaved changes — save (or undo them) first", true);
                } else {
                    self.session = EditSession::new(Document::new());
                    self.file_name = String::from("untitled.bpmn");
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        self.path = None;
                    }
                    self.selection.clear();
                    self.scope_path.clear();
                    self.camera = Camera::default();
                    self.document_changed();
                }
            }
            Message::OpenRequested => {
                return Task::perform(platform::open_file(), Message::FileOpened);
            }
            Message::FileOpened(Some(file)) => match bpmn_xml::parse(&file.contents) {
                Ok(document) => {
                    let elements = document.flow_elements().count();
                    self.status(
                        format!("Opened {} — {} flow elements", file.name, elements),
                        false,
                    );
                    self.session = EditSession::new(document);
                    self.file_name = file.name;
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        self.path = file.path;
                    }
                    self.selection.clear();
                    self.scope_path.clear();
                    self.zoom_to_fit();
                    self.document_changed();
                }
                Err(error) => self.status(format!("Could not open {}: {error}", file.name), true),
            },
            Message::SaveRequested => return self.save_task(false),
            Message::SaveAsRequested => return self.save_task(true),
            Message::ExportSvg => {
                let svg = crate::svg::export(self.session.document());
                let name = format!(
                    "{}.svg",
                    self.file_name
                        .trim_end_matches(".bpmn")
                        .trim_end_matches(".xml")
                );
                #[cfg(not(target_arch = "wasm32"))]
                {
                    return Task::perform(platform::save_file(name, svg), Message::Exported);
                }
                #[cfg(target_arch = "wasm32")]
                {
                    return Task::perform(
                        async move { platform::save_file(&name, &svg) },
                        Message::Exported,
                    );
                }
            }
            Message::Exported(Some(saved)) => {
                self.status(format!("Exported {}", saved.name), false);
            }
            Message::Saved(Some(saved)) => {
                self.session.mark_saved();
                self.file_name.clone_from(&saved.name);
                #[cfg(not(target_arch = "wasm32"))]
                {
                    self.path = saved.path;
                }
                self.status(format!("Saved {}", saved.name), false);
            }
            _ => {}
        }

        Task::none()
    }

    /// The navigation tab bar: the file at the root, then one tab per
    /// drilled subprocess. On macOS it starts after the traffic lights and
    /// its empty space drags the window (commands live in the native menu).
    fn tab_bar(&self, t: Tokens) -> Element<'_, Message> {
        let depth = self.scope_path.len();
        let mut bar = row![].spacing(4).align_y(Center);
        if cfg!(target_os = "macos") {
            // Clearance for the traffic lights floating over the bar.
            bar = bar.push(space::horizontal().width(70.0));
        }
        bar = bar.push(Self::tab(t, self.file_name.clone(), 0, depth));
        for (i, level) in self.scope_path.iter().enumerate() {
            bar = bar.push(text("›").size(12).color(t.muted));
            let mut name = self.element_name(&level.id);
            if name.is_empty() {
                name = level.id.as_str().to_string();
            }
            bar = bar.push(Self::tab(t, name, i + 1, depth));
        }

        let filler: Element<'_, Message> = if cfg!(target_os = "macos") {
            iced::widget::mouse_area(space::horizontal().width(Fill).height(Fill))
                .on_press(Message::Canvas(CanvasMessage::DragWindow))
                .into()
        } else {
            space::horizontal().into()
        };
        bar = bar.push(filler);

        // Transparent: the bar floats over the canvas, which stays visible
        // (and reaches the window top on macOS) underneath it.
        container(bar.width(Fill).height(Fill))
            .width(Fill)
            .height(TAB_BAR_HEIGHT)
            .padding([2, 8])
            .into()
    }

    /// One navigation tab; the active (deepest) one is highlighted and inert.
    fn tab(t: Tokens, label: String, depth: usize, current: usize) -> Element<'static, Message> {
        let active = depth == current;
        button(text(label).size(12).font(theme::SANS_MEDIUM))
            .style(theme::palette_button(t, active))
            .padding([2, 10])
            .on_press_maybe((!active).then_some(Message::NavigateToDepth(depth)))
            .into()
    }

    /// Builds the widget tree for this frame.
    pub fn view(&self) -> Element<'_, Message> {
        let t = self.tokens();

        // On macOS there is no toolbar: commands live in the native menu bar
        // and the tab bar doubles as the window-drag strip under the traffic
        // lights.
        let mut layout = column![];
        if !cfg!(target_os = "macos") {
            layout = layout.push(self.toolbar(t));
        }

        // The tab bar floats transparently over the canvas top, so the
        // workspace stays visible underneath (up to the window top on macOS).
        let canvas = iced::widget::stack![
            container(
                self.canvas
                    .view(
                        &self.camera,
                        self.session.document(),
                        self.scope(),
                        &self.selection,
                        &self.badges,
                        t.canvas,
                    )
                    .map(Message::Canvas)
            )
            .width(Fill)
            .height(Fill),
            self.tab_bar(t),
        ];

        layout = layout.push(
            row![
                container(canvas).width(Fill).height(Fill),
                self.properties(t),
            ]
            .width(Fill)
            .height(Fill),
        );
        let base: Element<'_, Message> = layout
            .push(self.issues_panel(t))
            .push(self.status_bar(t))
            .into();

        match &self.overlay {
            Some(CanvasOverlay::PlaceMenu { at, .. }) => {
                self.overlay_at(base, *at, Self::place_menu(t))
            }
            Some(CanvasOverlay::ElementMenu { at, id, .. }) => {
                self.overlay_at(base, *at, self.element_menu(t, id))
            }
            Some(CanvasOverlay::LabelEdit { at }) => {
                self.overlay_at(base, *at, self.inline_editor(t))
            }
            None => base,
        }
    }

    fn issues_panel(&self, t: Tokens) -> Element<'_, Message> {
        if self.issues.is_empty() || !self.issues_open {
            return space::vertical().height(0).into();
        }

        let rows = self.issues.iter().map(|issue| {
            let dot_color = match issue.severity {
                Severity::Error => t.danger,
                Severity::Warning | Severity::Info => t.warning,
            };
            button(
                row![
                    text("\u{25cf}").size(11).color(dot_color),
                    text(issue.rule).font(theme::MONO).size(11).color(t.muted),
                    text(issue.message.clone()).size(12),
                ]
                .spacing(8)
                .align_y(Center),
            )
            .style(theme::issue_row(t))
            .padding([2, 8])
            .width(Fill)
            .on_press(Message::IssueClicked(issue.element.clone()))
            .into()
        });

        container(
            scrollable(column(rows).spacing(2))
                .height(f32::min(94.0, 22.0 * self.issues.len() as f32 + 8.0)),
        )
        .style(move |_| theme::panel(t))
        .padding([4, 8])
        .width(Fill)
        .into()
    }

    fn toolbar(&self, t: Tokens) -> Element<'_, Message> {
        let tool = |glyph: char, label: &'static str, message: Message, enabled: bool| {
            let base = button(
                row![icons::icon(glyph), text(label).size(13)]
                    .spacing(5)
                    .align_y(Center),
            )
            .style(theme::tool_button(t))
            .padding([4, 10]);
            if enabled {
                base.on_press(message)
            } else {
                base
            }
        };
        let icon_tool = |glyph: char, message: Message, enabled: bool| {
            let base = button(icons::icon(glyph))
                .style(theme::tool_button(t))
                .padding([4, 8]);
            if enabled {
                base.on_press(message)
            } else {
                base
            }
        };

        let save_label = if cfg!(target_arch = "wasm32") {
            "Download"
        } else {
            "Save"
        };

        let mut bar = row![
            tool(icons::NEW, "New", Message::NewRequested, true),
            tool(icons::OPEN, "Open…", Message::OpenRequested, true),
            tool(icons::SAVE, save_label, Message::SaveRequested, true),
        ]
        .spacing(8)
        .align_y(Center);

        bar = bar.push(tool(icons::EXPORT, "SVG", Message::ExportSvg, true));
        #[cfg(not(target_arch = "wasm32"))]
        {
            bar = bar.push(tool(
                icons::SAVE_AS,
                "Save as…",
                Message::SaveAsRequested,
                true,
            ));
            bar = bar.push(tool(
                icons::MCP,
                if self.mcp_running { "MCP on" } else { "MCP" },
                Message::McpStart,
                !self.mcp_running,
            ));
        }

        bar = bar
            .push(space::horizontal())
            .push(icon_tool(
                icons::UNDO,
                Message::Undo,
                self.session.can_undo(),
            ))
            .push(icon_tool(
                icons::REDO,
                Message::Redo,
                self.session.can_redo(),
            ))
            .push(space::horizontal())
            .push(icon_tool(icons::ZOOM_OUT, Message::ZoomOut, true))
            .push(
                container(
                    text(format!("{:>3}%", self.camera.zoom_percent()))
                        .font(theme::MONO)
                        .size(12),
                )
                .padding([4, 6]),
            )
            .push(icon_tool(icons::ZOOM_IN, Message::ZoomIn, true))
            .push(icon_tool(icons::FIT, Message::ZoomFit, true))
            .push(icon_tool(
                match self.effective_mode() {
                    Mode::Light => icons::MOON,
                    Mode::Dark => icons::SUN,
                },
                Message::ToggleTheme,
                true,
            ));

        container(bar)
            .style(move |_| theme::panel(t))
            .padding(8)
            .width(Fill)
            .into()
    }

    /// Stacks `content` over the layout at a clamped window point, with a
    /// transparent click-catcher that closes the overlay.
    fn overlay_at<'a>(
        &'a self,
        base: Element<'a, Message>,
        at: iced::Point,
        content: Element<'a, Message>,
    ) -> Element<'a, Message> {
        const WIDTH: f32 = 190.0;
        const MAX_HEIGHT: f32 = 420.0;

        let x = at.x.min(self.window_size.width - WIDTH - 12.0).max(0.0);
        let y = at
            .y
            .min(self.window_size.height - MAX_HEIGHT.min(self.window_size.height * 0.6) - 40.0)
            .max(0.0);

        iced::widget::stack![
            base,
            iced::widget::mouse_area(space::vertical().width(Fill).height(Fill))
                .on_press(Message::CloseOverlay)
                .on_right_press(Message::CloseOverlay),
            container(content).padding(iced::Padding {
                top: y,
                left: x,
                right: 0.0,
                bottom: 0.0,
            }),
        ]
        .into()
    }

    /// One slim overlay-menu entry: a mini glyph (when given) plus a label.
    fn overlay_entry<'a>(
        t: Tokens,
        glyph: Option<Element<'a, Message>>,
        label: &'a str,
        message: Message,
    ) -> Element<'a, Message> {
        let mut content = row![].spacing(4).align_y(Center);
        if let Some(glyph) = glyph {
            content = content.push(container(glyph).width(26));
        }
        content = content.push(text(label).size(12));
        button(content)
            .style(theme::palette_button(t, false))
            .padding([1, 6])
            .width(Fill)
            .on_press(message)
            .into()
    }

    /// The placement catalog shown on empty-space right-click.
    fn place_menu(t: Tokens) -> Element<'static, Message> {
        let mut list = column![].spacing(2);
        for (section, entries) in Self::PALETTE {
            list = list.push(
                text(*section)
                    .font(theme::SANS_MEDIUM)
                    .size(11)
                    .color(t.muted),
            );
            for (label, placeable) in *entries {
                let glyph = match placeable {
                    Placeable::Node(kind) => {
                        icons::shape_glyph(icons::ShapeGlyph::Node(*kind), t.canvas)
                    }
                    Placeable::Pool => icons::shape_glyph(icons::ShapeGlyph::Pool, t.canvas),
                };
                list = list.push(Self::overlay_entry(
                    t,
                    Some(glyph),
                    label,
                    Message::ContextPlace(*placeable),
                ));
            }
        }

        container(scrollable(list).height(iced::Shrink))
            .style(move |_| theme::context_menu(t))
            .padding(8)
            .width(190)
            .max_height(420)
            .into()
    }

    /// The small menu shown on element right-click.
    fn element_menu(&self, t: Tokens, id: &BpmnId) -> Element<'_, Message> {
        let mut list = column![].spacing(2);
        let document = self.session.document();
        let index = document.flow_element_index();
        let is_activity = matches!(index.get(id), Some(FlowElement::Activity(_)));
        let is_subprocess = matches!(
            index.get(id),
            Some(FlowElement::Activity(activity))
                if matches!(activity.kind, ActivityKind::SubProcess(_))
        );
        if is_subprocess {
            list = list.push(Self::overlay_entry(
                t,
                None,
                "View subprocess",
                Message::ViewSubprocess,
            ));
        }
        if is_activity {
            list = list.push(Self::overlay_entry(
                t,
                Some(icons::shape_glyph(
                    icons::ShapeGlyph::Node(NodeKind::Boundary),
                    t.canvas,
                )),
                "Add boundary event",
                Message::AddBoundaryHere,
            ));
        }
        list = list.push(Self::overlay_entry(t, None, "Rename", Message::RenameHere));
        list = list.push(Self::overlay_entry(
            t,
            None,
            "Delete",
            Message::DeleteSelection,
        ));

        container(list)
            .style(move |_| theme::context_menu(t))
            .padding(8)
            .width(180)
            .into()
    }

    /// The floating name editor opened by a double-click. Multi-line:
    /// Shift+Enter breaks the line, Enter and Escape close the editor.
    fn inline_editor(&self, t: Tokens) -> Element<'_, Message> {
        use iced::keyboard::key::{Key, Named};

        container(
            text_editor(&self.inline_name)
                .id(Self::inline_input_id())
                .placeholder("Name")
                .on_action(Message::InlineNameAction)
                .key_binding(|press| match press.key.as_ref() {
                    Key::Named(Named::Enter) if !press.modifiers.shift() => {
                        Some(text_editor::Binding::Custom(Message::CloseOverlay))
                    }
                    Key::Named(Named::Escape) => {
                        Some(text_editor::Binding::Custom(Message::CloseOverlay))
                    }
                    _ => text_editor::Binding::from_key_press(press),
                })
                .size(13)
                .width(170),
        )
        .style(move |_| theme::context_menu(t))
        .padding(6)
        .into()
    }

    /// The width the side panels occupy, honoring the collapsed state.
    /// The placement catalog: section headers with placeable elements.
    /// (Boundary events live in the element menu — they need a host.)
    const PALETTE: &'static [(&'static str, &'static [(&'static str, Placeable)])] = &[
        (
            "Events",
            &[
                ("Start", Placeable::Node(NodeKind::StartEvent)),
                ("Catch", Placeable::Node(NodeKind::IntermediateCatch)),
                ("Throw", Placeable::Node(NodeKind::IntermediateThrow)),
                ("End", Placeable::Node(NodeKind::EndEvent)),
            ],
        ),
        (
            "Tasks",
            &[
                ("Task", Placeable::Node(NodeKind::Task(TaskType::Generic))),
                ("User", Placeable::Node(NodeKind::Task(TaskType::User))),
                (
                    "Service",
                    Placeable::Node(NodeKind::Task(TaskType::Service)),
                ),
                ("Script", Placeable::Node(NodeKind::Task(TaskType::Script))),
                ("Manual", Placeable::Node(NodeKind::Task(TaskType::Manual))),
                ("Send", Placeable::Node(NodeKind::Task(TaskType::Send))),
                (
                    "Receive",
                    Placeable::Node(NodeKind::Task(TaskType::Receive)),
                ),
                (
                    "Business rule",
                    Placeable::Node(NodeKind::Task(TaskType::BusinessRule)),
                ),
            ],
        ),
        (
            "Containers",
            &[
                (
                    "Subprocess",
                    Placeable::Node(NodeKind::SubProcess(SubProcessKind::Embedded)),
                ),
                (
                    "Event subproc.",
                    Placeable::Node(NodeKind::SubProcess(SubProcessKind::Event)),
                ),
                (
                    "Transaction",
                    Placeable::Node(NodeKind::SubProcess(SubProcessKind::Transaction)),
                ),
                ("Call activity", Placeable::Node(NodeKind::CallActivity)),
            ],
        ),
        ("Collaboration", &[("Pool", Placeable::Pool)]),
        (
            "Data & artifacts",
            &[
                ("Data object", Placeable::Node(NodeKind::DataObject)),
                ("Data store", Placeable::Node(NodeKind::DataStore)),
                ("Annotation", Placeable::Node(NodeKind::TextAnnotation)),
                ("Group", Placeable::Node(NodeKind::Group)),
            ],
        ),
        (
            "Gateways",
            &[
                (
                    "Exclusive",
                    Placeable::Node(NodeKind::Gateway(GatewayKind::Exclusive)),
                ),
                (
                    "Parallel",
                    Placeable::Node(NodeKind::Gateway(GatewayKind::Parallel)),
                ),
                (
                    "Inclusive",
                    Placeable::Node(NodeKind::Gateway(GatewayKind::Inclusive)),
                ),
                (
                    "Event-based",
                    Placeable::Node(NodeKind::Gateway(GatewayKind::EventBased)),
                ),
            ],
        ),
    ];

    /// A collapsed side panel: a slim strip holding only the reopen button.
    fn collapsed_panel(t: Tokens, glyph: char, message: Message) -> Element<'static, Message> {
        let padding = if cfg!(target_os = "macos") {
            iced::Padding::from(4).top(34)
        } else {
            iced::Padding::from(4)
        };
        container(
            button(icons::icon(glyph))
                .style(theme::tool_button(t))
                .padding([4, 4])
                .on_press(message),
        )
        .style(move |_| theme::panel(t))
        .padding(padding)
        .width(36)
        .height(Fill)
        .into()
    }

    fn properties(&self, t: Tokens) -> Element<'_, Message> {
        let body: Element<'_, Message> = if let Some(id) = self.single_selection() {
            let document = self.session.document();
            let index = document.flow_element_index();
            match index.get(&id) {
                Some(element) => Self::element_form(t, document, &id, element),
                None => Self::collaboration_form(t, document, &id)
                    .unwrap_or_else(|| text("No selection").size(13).color(t.muted).into()),
            }
        } else if self.selection.len() > 1 {
            text(format!("{} elements selected", self.selection.len()))
                .size(13)
                .color(t.muted)
                .into()
        } else {
            text("No selection").size(13).color(t.muted).into()
        };

        if !self.properties_open {
            return Self::collapsed_panel(t, icons::PANEL_RIGHT, Message::ToggleProperties);
        }
        let header = row![
            text("Properties").font(theme::SANS_MEDIUM).size(14),
            space::horizontal(),
            button(icons::icon(icons::CHEVRON_RIGHT))
                .style(theme::tool_button(t))
                .padding([2, 6])
                .on_press(Message::ToggleProperties),
        ]
        .align_y(Center);

        container(column![header, body].spacing(12))
            .style(move |_| theme::panel(t))
            .padding(10)
            .width(230)
            .height(Fill)
            .into()
    }

    fn element_form<'a>(
        t: Tokens,
        document: &Document,
        id: &BpmnId,
        element: &FlowElement,
    ) -> Element<'a, Message> {
        let caption = |label: &'static str| text(label).size(11).color(t.muted);

        let mut form = column![
            caption("Id"),
            text(id.as_str().to_string()).font(theme::MONO).size(12),
            caption("Name"),
            text_input("Unnamed", element.name().unwrap_or(""))
                .id(Self::name_input_id())
                .on_input(Message::NameEdited)
                .size(13),
        ]
        .spacing(6);

        match element {
            FlowElement::Activity(activity) => {
                form = Self::activity_form(form, t, document, id, activity);
            }

            FlowElement::Gateway(gateway) => {
                let mut options = vec![String::from(NO_DEFAULT)];
                options.extend(
                    document
                        .outgoing_flows(id)
                        .iter()
                        .map(|flow| flow.as_str().to_string()),
                );
                let selected = gateway
                    .default_flow
                    .as_ref()
                    .map_or_else(|| String::from(NO_DEFAULT), |d| d.as_str().to_string());
                form = form.push(caption("Default flow")).push(
                    pick_list(options, Some(selected), Message::DefaultFlowChanged)
                        .text_size(13)
                        .width(Fill),
                );
            }
            FlowElement::SequenceFlow(flow) => match &flow.condition {
                Some(Condition::Raw(_)) => {
                    form = form.push(caption("Condition")).push(
                        text("(complex expression, preserved as-is)")
                            .size(12)
                            .color(t.muted),
                    );
                }
                condition => {
                    let value = match condition {
                        Some(Condition::Expression { text, .. }) => text.as_str(),
                        _ => "",
                    };
                    form = form.push(caption("Condition")).push(
                        text_input("${expression}", value)
                            .on_input(Message::ConditionEdited)
                            .font(theme::MONO)
                            .size(13),
                    );
                }
            },
            FlowElement::Event(event) => {
                let current = TriggerChoice(event.definitions.first().map(|d| d.kind));
                form = form.push(caption("Trigger")).push(
                    pick_list(
                        trigger_options(&event.position),
                        Some(current),
                        Message::TriggerChanged,
                    )
                    .text_size(13)
                    .width(Fill),
                );
            }
            // Data elements and artifacts only expose their name (an
            // annotation's name is its text); unknown elements nothing.
            FlowElement::Data(_) | FlowElement::Artifact(_) | FlowElement::Unknown(_) => {}
        }

        form.into()
    }

    /// The properties form for collaboration elements (pools, lanes, message
    /// flows), which live outside the flow-element index.
    fn collaboration_form<'a>(
        t: Tokens,
        document: &Document,
        id: &BpmnId,
    ) -> Option<Element<'a, Message>> {
        let caption = |label: &'static str| text(label).size(11).color(t.muted);
        let (kind, name): (&str, Option<&str>) = if let Some(participant) = document.participant(id)
        {
            ("Pool", participant.name.as_deref())
        } else if let Some(lane) = document.lane(id) {
            ("Lane", lane.name.as_deref())
        } else {
            let flow = document.message_flows().find(|m| m.base.id == *id)?;
            ("Message flow", flow.name.as_deref())
        };

        let mut form = column![
            caption("Kind"),
            text(kind).size(13),
            caption("Id"),
            text(id.as_str().to_string()).font(theme::MONO).size(12),
            caption("Name"),
            text_input("Unnamed", name.unwrap_or(""))
                .id(Self::name_input_id())
                .on_input(Message::NameEdited)
                .size(13),
        ]
        .spacing(6);

        if kind == "Pool" {
            form = form.push(
                button(text("Add lane").size(13))
                    .style(theme::tool_button(t))
                    .padding([4, 10])
                    .on_press(Message::AddLane),
            );
        }
        Some(form.into())
    }

    /// The kind-specific rows of the properties form for an activity.
    fn activity_form<'a>(
        mut form: iced::widget::Column<'a, Message>,
        t: Tokens,
        document: &Document,
        id: &BpmnId,
        activity: &bpmn_core::model::Activity,
    ) -> iced::widget::Column<'a, Message> {
        let caption = |label: &'static str| text(label).size(11).color(t.muted);

        match &activity.kind {
            ActivityKind::Task(task_type) => {
                form = form.push(caption("Task type")).push(
                    pick_list(
                        TASK_TYPES,
                        Some(TaskTypeChoice(*task_type)),
                        Message::TaskTypeChanged,
                    )
                    .text_size(13)
                    .width(Fill),
                );
            }
            ActivityKind::SubProcess(sub) => {
                let kind_label = match sub.kind {
                    SubProcessKind::Embedded => "Subprocess",
                    SubProcessKind::Event => "Event subprocess",
                    SubProcessKind::Transaction => "Transaction",
                };
                let expanded = shape_is_expanded(document, id);
                form = form
                    .push(caption("Kind"))
                    .push(text(kind_label).size(13))
                    .push(caption("Contents"))
                    .push(
                        text(format!("{} elements", sub.flow_elements.len()))
                            .size(12)
                            .color(t.muted),
                    )
                    .push(
                        button(text(if expanded { "Collapse" } else { "Expand" }).size(13))
                            .style(theme::tool_button(t))
                            .padding([4, 10])
                            .on_press(Message::ToggleExpanded),
                    );
            }
            ActivityKind::CallActivity(call) => {
                form = form.push(caption("Called element")).push(
                    text_input("Process id", call.called_element.as_deref().unwrap_or(""))
                        .on_input(Message::CalledElementEdited)
                        .font(theme::MONO)
                        .size(13),
                );
            }
        }
        form.push(caption("Marker")).push(
            pick_list(
                LOOP_CHOICES,
                Some(LoopChoice(activity.loop_marker.as_ref().map(|m| m.kind))),
                Message::LoopChanged,
            )
            .text_size(13)
            .width(Fill),
        )
    }

    /// Validation chip: a check when clean, the count otherwise; clicking it
    /// shows or hides the issues panel.
    fn issue_chip<'a>(
        &self,
        t: Tokens,
        chip: &impl Fn(Element<'a, Message>, Option<Message>) -> iced::widget::Button<'a, Message>,
    ) -> Element<'a, Message> {
        if self.issues.is_empty() {
            chip(
                row![
                    icons::icon(icons::CHECK).size(11).color(t.muted),
                    text("no issues").size(11).color(t.muted),
                ]
                .spacing(4)
                .align_y(Center)
                .into(),
                None,
            )
            .into()
        } else {
            let worst_is_error = self.issues.iter().any(|i| i.severity == Severity::Error);
            let color = if worst_is_error { t.danger } else { t.warning };
            chip(
                row![
                    icons::icon(icons::ALERT).size(11).color(color),
                    text(format!("{} issues", self.issues.len()))
                        .size(11)
                        .color(color),
                ]
                .spacing(4)
                .align_y(Center)
                .into(),
                Some(Message::ToggleIssues),
            )
            .into()
        }
    }

    /// MCP chip: clicking it copies the connect command.
    fn mcp_chip<'a>(
        &self,
        t: Tokens,
        chip: &impl Fn(Element<'a, Message>, Option<Message>) -> iced::widget::Button<'a, Message>,
    ) -> Element<'a, Message> {
        #[cfg(not(target_arch = "wasm32"))]
        if self.mcp_running {
            return chip(
                text(format!("MCP :{}", boss_mcp_port()))
                    .font(theme::MONO)
                    .size(11)
                    .color(t.accent)
                    .into(),
                Some(Message::CopyMcpCommand),
            )
            .into();
        }
        let _ = (self, t, chip);
        text("").size(11).into()
    }

    fn status_bar(&self, t: Tokens) -> Element<'_, Message> {
        let hint =
            "right-click to add · double-click to rename · drag to pan · shift-drag to select";
        let info: Element<'_, Message> = match &self.status {
            Some(line) => text(line.text.clone())
                .size(11)
                .color(if line.is_error { t.danger } else { t.muted })
                .into(),
            None => text(hint).size(11).color(t.muted).into(),
        };

        let chip = |content: Element<'static, Message>, message: Option<Message>| {
            let base = button(content).style(theme::tool_button(t)).padding([1, 6]);
            match message {
                Some(message) => base.on_press(message),
                None => base,
            }
        };

        // File name with an unsaved-changes dot.
        let file_chip: Element<'_, Message> = row![
            text(if self.session.is_dirty() {
                "\u{25cf} "
            } else {
                ""
            })
            .size(11)
            .color(t.accent),
            text(self.file_name.clone()).font(theme::MONO).size(11),
        ]
        .align_y(Center)
        .into();

        let selection_label = match self.selection.len() {
            0 => String::new(),
            n => format!("· {n} selected"),
        };

        let cursor_label = self
            .cursor_world
            .map_or(String::new(), |(x, y)| format!("x {x}  y {y}"));

        let issue_chip = self.issue_chip(t, &chip);
        let mcp_chip = self.mcp_chip(t, &chip);

        container(
            row![
                file_chip,
                text(format!("zoom {}%", self.camera.zoom_percent()))
                    .font(theme::MONO)
                    .size(11),
                text(selection_label)
                    .font(theme::MONO)
                    .size(11)
                    .color(t.muted),
                text(cursor_label).font(theme::MONO).size(11).color(t.muted),
                issue_chip,
                mcp_chip,
                space::horizontal(),
                info,
            ]
            .spacing(12)
            .align_y(Center),
        )
        .style(move |_| theme::panel(t))
        .padding([3, 12])
        .width(Fill)
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branding_replaces_exporter_and_target_namespace() {
        let mut doc = bpmn_xml::parse(include_str!(
            "../../../tests-fixtures/order-fulfillment.bpmn"
        ))
        .unwrap();
        doc.definitions.exporter = Some(String::from("Some Other Modeler"));
        let branded = branded_for_save(&doc);

        let def = &branded.definitions;
        assert_eq!(def.exporter.as_deref(), Some("boss"));
        assert_eq!(
            def.exporter_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(
            def.target_namespace,
            bpmn_core::model::BOSS_TARGET_NAMESPACE
        );

        let out = bpmn_xml::write(&branded);
        assert!(!out.contains("Some Other Modeler"));
        // The fixture's foreign targetNamespace is gone...
        assert!(!out.contains("targetNamespace=\"https://example.com/bpmn\""));
        // ...but vendor extension content is user data and stays untouched.
        assert!(out.contains("acme:assignee=\"demo\""));
    }

    #[test]
    fn a_fresh_document_writes_valid_bpmn() {
        let out = bpmn_xml::write(&branded_for_save(&Document::new()));
        let reparsed = bpmn_xml::parse(&out).unwrap();
        assert!(reparsed.processes().next().is_some());
        assert!(out.contains("xmlns:bpmn=\"http://www.omg.org/spec/BPMN/20100524/MODEL\""));
    }
}
