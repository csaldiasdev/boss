//! The application shell: model, messages, update, and view.

use crate::camera::Camera;
use crate::canvas::{CanvasMessage, DiagramCanvas, Tool};
use crate::platform::{self, OpenedFile, SavedFile};
use crate::session::EditSession;
use crate::theme::{self, Mode, Tokens};
use bpmn_core::di::DiElement;
use bpmn_core::model::{GatewayKind, TaskType};
use bpmn_core::{BpmnId, Document, NodeKind};
use iced::widget::{button, column, container, row, space, text};
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

/// A transient message for the status bar.
struct StatusLine {
    text: String,
    is_error: bool,
}

/// Top-level application state.
pub struct App {
    camera: Camera,
    canvas: DiagramCanvas,
    mode: Mode,
    theme: Theme,
    session: EditSession,
    file_name: String,
    #[cfg(not(target_arch = "wasm32"))]
    path: Option<std::path::PathBuf>,
    selection: HashSet<BpmnId>,
    tool: Tool,
    status: Option<StatusLine>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            camera: Camera::default(),
            canvas: DiagramCanvas::default(),
            mode: Mode::default(),
            theme: theme::iced_theme(Mode::default()),
            session: EditSession::new(Document::new()),
            file_name: String::from("untitled.bpmn"),
            #[cfg(not(target_arch = "wasm32"))]
            path: None,
            selection: HashSet::new(),
            tool: Tool::Select,
            status: None,
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
    /// Switch between light and dark.
    ToggleTheme,
    /// A palette/toolbar tool was picked.
    ToolSelected(Tool),
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
}

impl App {
    /// Initial state, plus opening a file passed as a CLI argument on native.
    pub fn boot() -> (Self, Task<Message>) {
        #[cfg(not(target_arch = "wasm32"))]
        let task = match std::env::args().nth(1) {
            Some(path) => Task::perform(
                async move { platform::read_path(&path) },
                Message::FileOpened,
            ),
            None => Task::none(),
        };
        #[cfg(target_arch = "wasm32")]
        let task = Task::none();

        (Self::default(), task)
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

    /// Keyboard shortcuts (events no widget consumed).
    pub fn subscription(&self) -> Subscription<Message> {
        let _ = self;
        iced::keyboard::listen().map(Message::Keyboard)
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
            Key::Character("z") if command && modifiers.shift() => Some(Message::Redo),
            Key::Character("z") if command => Some(Message::Undo),
            Key::Character("y") if command => Some(Message::Redo),
            Key::Character("s") if command => Some(Message::SaveRequested),
            Key::Character("o") if command => Some(Message::OpenRequested),
            _ => None,
        }
    }

    fn tokens(&self) -> Tokens {
        theme::tokens(self.mode)
    }

    fn status(&mut self, text: impl Into<String>, is_error: bool) {
        self.status = Some(StatusLine {
            text: text.into(),
            is_error,
        });
    }

    /// Centers the camera on the document and picks a zoom that shows all of
    /// it. The viewport size is an estimate until M3 tracks the real one.
    fn zoom_to_fit(&mut self) {
        const VIEWPORT: Size = Size::new(860.0, 620.0);
        const MARGIN: f32 = 60.0;

        let Some(bounds) = self.session.document().content_bounds() else {
            self.camera = Camera::default();
            return;
        };

        let (width, height) = (bounds.width as f32 + MARGIN, bounds.height as f32 + MARGIN);
        let scaling = (VIEWPORT.width / width)
            .min(VIEWPORT.height / height)
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

    fn handle_canvas(&mut self, message: CanvasMessage) {
        match message {
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
                let document = self.session.document();
                let index = document.flow_element_index();
                if let Some(plane) = document.diagram().and_then(|d| d.plane.as_ref()) {
                    for element in &plane.elements {
                        if let DiElement::Shape(shape) = element
                            && index.contains_key(&shape.bpmn_element)
                            && world.intersects(&shape.bounds)
                        {
                            self.selection.insert(shape.bpmn_element.clone());
                        }
                    }
                }
            }
            CanvasMessage::MoveSelection { dx, dy } => {
                let selection = self.selection.clone();
                self.session.commit(|doc| {
                    doc.move_shapes(&selection, f64::from(dx), f64::from(dy));
                    true
                });
                self.canvas.invalidate();
            }
            CanvasMessage::Place(kind, at) => {
                let mut created = None;
                self.session.commit(|doc| {
                    created = doc.create_node(
                        kind,
                        bpmn_core::Point::new(f64::from(at.x), f64::from(at.y)),
                    );
                    created.is_some()
                });
                if let Some(id) = created {
                    self.selection.clear();
                    self.selection.insert(id);
                }
                self.tool = Tool::Select;
                self.canvas.invalidate();
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
                self.tool = Tool::Select;
                self.canvas.invalidate();
            }
            CanvasMessage::CancelTool => self.tool = Tool::Select,
        }
    }

    /// Handles a message.
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Canvas(canvas_message) => self.handle_canvas(canvas_message),
            Message::ZoomIn => {
                self.camera = self.camera.zoomed(1.2);
                self.canvas.invalidate();
            }
            Message::ZoomOut => {
                self.camera = self.camera.zoomed(1.0 / 1.2);
                self.canvas.invalidate();
            }
            Message::ZoomFit => {
                self.zoom_to_fit();
                self.canvas.invalidate();
            }
            Message::ToggleTheme => {
                self.mode = self.mode.toggled();
                self.theme = theme::iced_theme(self.mode);
                self.canvas.invalidate();
            }
            Message::ToolSelected(tool) => self.tool = tool,
            Message::Undo => {
                if self.session.undo() {
                    self.selection.clear();
                    self.canvas.invalidate();
                }
            }
            Message::Redo => {
                if self.session.redo() {
                    self.selection.clear();
                    self.canvas.invalidate();
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
                    self.canvas.invalidate();
                }
            }
            Message::Keyboard(event) => {
                if let Some(message) = Self::shortcut(&event) {
                    return self.update(message);
                }
            }
            Message::Escape => {
                if self.tool == Tool::Select {
                    self.selection.clear();
                } else {
                    self.tool = Tool::Select;
                }
            }
            message => return self.handle_file(message),
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
                    self.camera = Camera::default();
                    self.canvas.invalidate();
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
                    self.tool = Tool::Select;
                    self.zoom_to_fit();
                    self.canvas.invalidate();
                }
                Err(error) => self.status(format!("Could not open {}: {error}", file.name), true),
            },
            Message::SaveRequested => return self.save_task(false),
            Message::SaveAsRequested => return self.save_task(true),
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

    /// Builds the widget tree for this frame.
    pub fn view(&self) -> Element<'_, Message> {
        let t = self.tokens();

        column![
            self.toolbar(t),
            row![
                self.palette(t),
                container(
                    self.canvas
                        .view(
                            &self.camera,
                            self.session.document(),
                            &self.selection,
                            self.tool,
                            t.canvas,
                        )
                        .map(Message::Canvas)
                )
                .width(Fill)
                .height(Fill),
                Self::properties(t),
            ]
            .width(Fill)
            .height(Fill),
            self.status_bar(t),
        ]
        .into()
    }

    fn toolbar(&self, t: Tokens) -> Element<'_, Message> {
        let tool = |label: &'static str, message: Message, enabled: bool| {
            let base = button(text(label).size(13))
                .style(theme::tool_button(t))
                .padding([4, 10]);
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
            tool("New", Message::NewRequested, true),
            tool("Open…", Message::OpenRequested, true),
            tool(save_label, Message::SaveRequested, true),
        ]
        .spacing(8)
        .align_y(Center);

        #[cfg(not(target_arch = "wasm32"))]
        {
            bar = bar.push(tool("Save as…", Message::SaveAsRequested, true));
        }

        bar = bar
            .push(space::horizontal())
            .push(tool("Undo", Message::Undo, self.session.can_undo()))
            .push(tool("Redo", Message::Redo, self.session.can_redo()))
            .push(space::horizontal())
            .push(tool("−", Message::ZoomOut, true))
            .push(
                container(
                    text(format!("{:>3}%", self.camera.zoom_percent()))
                        .font(theme::MONO)
                        .size(12),
                )
                .padding([4, 6]),
            )
            .push(tool("+", Message::ZoomIn, true))
            .push(tool("Fit", Message::ZoomFit, true))
            .push(tool(
                match self.mode {
                    Mode::Light => "Dark",
                    Mode::Dark => "Light",
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

    fn palette(&self, t: Tokens) -> Element<'_, Message> {
        let entry = |label: &'static str, target: Tool| {
            button(text(label).size(13))
                .style(theme::palette_button(t, self.tool == target))
                .padding([5, 10])
                .width(Fill)
                .on_press(Message::ToolSelected(target))
        };
        let section = |label: &'static str| text(label).font(theme::SANS_MEDIUM).size(14);

        container(
            column![
                section("Tools"),
                entry("Select", Tool::Select),
                entry("Connect", Tool::Connect),
                section("Events"),
                entry("Start event", Tool::Place(NodeKind::StartEvent)),
                entry("End event", Tool::Place(NodeKind::EndEvent)),
                section("Tasks"),
                entry("Task", Tool::Place(NodeKind::Task(TaskType::Generic))),
                entry("User task", Tool::Place(NodeKind::Task(TaskType::User))),
                entry(
                    "Service task",
                    Tool::Place(NodeKind::Task(TaskType::Service))
                ),
                section("Gateways"),
                entry(
                    "Exclusive",
                    Tool::Place(NodeKind::Gateway(GatewayKind::Exclusive))
                ),
                entry(
                    "Parallel",
                    Tool::Place(NodeKind::Gateway(GatewayKind::Parallel))
                ),
            ]
            .spacing(6),
        )
        .style(move |_| theme::panel(t))
        .padding(12)
        .width(170)
        .height(Fill)
        .into()
    }

    fn properties(t: Tokens) -> Element<'static, Message> {
        container(
            column![
                text("Properties").font(theme::SANS_MEDIUM).size(14),
                text("No selection").size(13).color(t.muted),
            ]
            .spacing(12),
        )
        .style(move |_| theme::panel(t))
        .padding(12)
        .width(230)
        .height(Fill)
        .into()
    }

    fn status_bar(&self, t: Tokens) -> Element<'_, Message> {
        let hint = match self.tool {
            Tool::Select => "click to select · shift-click to add · drag empty space to marquee",
            Tool::Place(_) => "click to place · Esc to cancel",
            Tool::Connect => "click source, then target · Esc to cancel",
        };
        let info: Element<'_, Message> = match &self.status {
            Some(line) => text(line.text.clone())
                .size(11)
                .color(if line.is_error { t.danger } else { t.muted })
                .into(),
            None => text(hint).size(11).color(t.muted).into(),
        };

        let selection_label = match self.selection.len() {
            0 => String::new(),
            n => format!("· {n} selected"),
        };

        container(
            row![
                text(format!("zoom {}%", self.camera.zoom_percent()))
                    .font(theme::MONO)
                    .size(11),
                text(selection_label)
                    .font(theme::MONO)
                    .size(11)
                    .color(t.muted),
                space::horizontal(),
                info,
            ]
            .spacing(12)
            .align_y(Center),
        )
        .style(move |_| theme::panel(t))
        .padding([4, 12])
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
