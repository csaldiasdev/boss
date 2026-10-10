//! The application shell: model, messages, update, and view.

use crate::camera::Camera;
use crate::canvas::{CanvasMessage, DiagramCanvas};
use crate::platform::{self, OpenedFile};
use crate::theme::{self, Mode, Tokens};
use bpmn_core::Document;
use iced::widget::{button, column, container, row, space, text};
use iced::{Center, Element, Fill, Size, Task, Theme, Vector};

/// The document currently open in the editor.
struct OpenDocument {
    file_name: String,
    document: Document,
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
    open: Option<OpenDocument>,
    status: Option<StatusLine>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            camera: Camera::default(),
            canvas: DiagramCanvas::default(),
            mode: Mode::default(),
            theme: theme::iced_theme(Mode::default()),
            open: None,
            status: None,
        }
    }
}

/// Everything that can happen in the app.
#[derive(Debug, Clone)]
pub enum Message {
    /// Camera updates coming from canvas interaction.
    Canvas(CanvasMessage),
    /// Zoom in from the toolbar.
    ZoomIn,
    /// Zoom out from the toolbar.
    ZoomOut,
    /// Zoom to fit the open document (or reset the camera).
    ZoomFit,
    /// Switch between light and dark.
    ToggleTheme,
    /// The user asked to open a file.
    OpenRequested,
    /// The open dialog finished.
    FileOpened(Option<OpenedFile>),
    /// The user asked to save the document (native only; the web gets a
    /// download fallback in M2, hence the variants are unused on wasm).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    SaveRequested,
    /// The save dialog finished with this file name.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Saved(Option<String>),
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

    /// Window title.
    pub fn title(&self) -> String {
        match &self.open {
            Some(open) => format!("boss — {}", open.file_name),
            None => String::from("boss"),
        }
    }

    /// The current iced theme (built from the Slate tokens).
    pub fn theme(&self) -> Theme {
        self.theme.clone()
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

    /// Centers the camera on the open document and picks a zoom that shows all
    /// of it. The viewport size is an estimate until M3 tracks the real one.
    fn zoom_to_fit(&mut self) {
        const VIEWPORT: Size = Size::new(860.0, 620.0);
        const MARGIN: f32 = 60.0;

        let Some(bounds) = self.open.as_ref().and_then(|o| o.document.content_bounds()) else {
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

    /// Handles a message.
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Canvas(CanvasMessage::Panned(translation)) => {
                self.camera.translation = translation;
                self.canvas.invalidate();
            }
            Message::Canvas(CanvasMessage::Zoomed {
                scaling,
                translation,
            }) => {
                self.camera.scaling = scaling;
                if let Some(translation) = translation {
                    self.camera.translation = translation;
                }
                self.canvas.invalidate();
            }
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
                    self.open = Some(OpenDocument {
                        file_name: file.name,
                        document,
                    });
                    self.zoom_to_fit();
                    self.canvas.invalidate();
                }
                Err(error) => self.status(format!("Could not open {}: {error}", file.name), true),
            },
            Message::FileOpened(None) | Message::Saved(None) => {}
            Message::SaveRequested =>
            {
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(open) = &self.open {
                    let xml = bpmn_xml::write(&open.document);
                    return Task::perform(
                        platform::save_file(open.file_name.clone(), xml),
                        Message::Saved,
                    );
                }
            }
            Message::Saved(Some(name)) => self.status(format!("Saved {name}"), false),
        }

        Task::none()
    }

    /// Builds the widget tree for this frame.
    pub fn view(&self) -> Element<'_, Message> {
        let t = self.tokens();

        column![
            self.toolbar(t),
            row![
                Self::palette(t),
                container(
                    self.canvas
                        .view(
                            &self.camera,
                            self.open.as_ref().map(|o| &o.document),
                            t.canvas
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
        let tool = |label: &'static str, message: Message| {
            button(text(label).size(13))
                .style(theme::tool_button(t))
                .padding([4, 10])
                .on_press(message)
        };

        let mut bar = row![tool("Open…", Message::OpenRequested),]
            .spacing(8)
            .align_y(Center);

        #[cfg(not(target_arch = "wasm32"))]
        if self.open.is_some() {
            bar = bar.push(tool("Save as…", Message::SaveRequested));
        }

        bar = bar
            .push(space::horizontal())
            .push(tool("−", Message::ZoomOut))
            .push(
                container(
                    text(format!("{:>3}%", self.camera.zoom_percent()))
                        .font(theme::MONO)
                        .size(12),
                )
                .padding([4, 6]),
            )
            .push(tool("+", Message::ZoomIn))
            .push(tool("Fit", Message::ZoomFit))
            .push(space::horizontal())
            .push(tool(
                match self.mode {
                    Mode::Light => "Dark",
                    Mode::Dark => "Light",
                },
                Message::ToggleTheme,
            ));

        container(bar)
            .style(move |_| theme::panel(t))
            .padding(8)
            .width(Fill)
            .into()
    }

    fn palette(t: Tokens) -> Element<'static, Message> {
        let section = |label: &'static str| text(label).font(theme::SANS_MEDIUM).size(14);
        let hint = |label: &'static str| text(label).size(13).color(t.muted);

        container(
            column![
                section("Palette"),
                hint("Events"),
                hint("Tasks"),
                hint("Gateways"),
                hint("Containers"),
                hint("Data"),
                hint("Artifacts"),
                space::vertical(),
                hint("Placement lands in M2"),
            ]
            .spacing(12),
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
        let info: Element<'_, Message> = match &self.status {
            Some(line) => text(line.text.clone())
                .size(11)
                .color(if line.is_error { t.danger } else { t.muted })
                .into(),
            None => text("Open a .bpmn file · drag to pan · scroll to zoom")
                .size(11)
                .color(t.muted)
                .into(),
        };

        container(
            row![
                text(format!("zoom {}%", self.camera.zoom_percent()))
                    .font(theme::MONO)
                    .size(11),
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
