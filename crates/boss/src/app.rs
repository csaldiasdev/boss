//! The application shell: model, messages, update, and view.

use crate::camera::Camera;
use crate::canvas::{CanvasMessage, DiagramCanvas};
use crate::theme::{self, Mode, Tokens};
use iced::widget::{button, column, container, row, space, text};
use iced::{Center, Element, Fill, Theme};

/// Top-level application state.
pub struct App {
    camera: Camera,
    canvas: DiagramCanvas,
    mode: Mode,
    theme: Theme,
}

impl Default for App {
    fn default() -> Self {
        Self {
            camera: Camera::default(),
            canvas: DiagramCanvas::default(),
            mode: Mode::default(),
            theme: theme::iced_theme(Mode::default()),
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
    /// Reset the camera to 100% centered on the origin.
    ZoomReset,
    /// Switch between light and dark.
    ToggleTheme,
}

impl App {
    /// Window title.
    pub fn title(&self) -> String {
        // The open file name (plus a dirty marker) joins this in M1.
        let _ = self;
        String::from("boss — untitled.bpmn")
    }

    /// The current iced theme (built from the Slate tokens).
    pub fn theme(&self) -> Theme {
        self.theme.clone()
    }

    fn tokens(&self) -> Tokens {
        theme::tokens(self.mode)
    }

    /// Handles a message.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "iced's update signature takes messages by value"
    )]
    pub fn update(&mut self, message: Message) {
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
            Message::ZoomReset => {
                self.camera = Camera::default();
                self.canvas.invalidate();
            }
            Message::ToggleTheme => {
                self.mode = self.mode.toggled();
                self.theme = theme::iced_theme(self.mode);
                self.canvas.invalidate();
            }
        }
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
                        .view(&self.camera, t.canvas)
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

        container(
            row![
                tool("−", Message::ZoomOut),
                container(
                    text(format!("{:>3}%", self.camera.zoom_percent()))
                        .font(theme::MONO)
                        .size(12),
                )
                .padding([4, 6]),
                tool("+", Message::ZoomIn),
                tool("Reset", Message::ZoomReset),
                space::horizontal(),
                tool(
                    match self.mode {
                        Mode::Light => "Dark",
                        Mode::Dark => "Light",
                    },
                    Message::ToggleTheme,
                ),
            ]
            .spacing(8)
            .align_y(Center),
        )
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
        container(
            row![
                text(format!("zoom {}%", self.camera.zoom_percent()))
                    .font(theme::MONO)
                    .size(11),
                space::horizontal(),
                text("drag to pan · scroll to zoom · M0 skeleton")
                    .size(11)
                    .color(t.muted),
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
