//! The diagram canvas: an `iced` canvas widget with a pannable, zoomable grid.
//!
//! M0 scope: camera + grid only. BPMN shape rendering lands in M1.

use crate::camera::Camera;
use crate::theme::CanvasTokens;
use iced::mouse;
use iced::widget::canvas::{self, Cache, Canvas, Event, Geometry, Path, Stroke};
use iced::{Element, Fill, Point, Rectangle, Renderer, Theme, Vector};

/// Minor grid step in world units.
pub const GRID_STEP: f32 = 20.0;
/// Every n-th grid line is a major line.
const MAJOR_EVERY: i64 = 5;
/// Below this zoom the grid is pure noise, so it is hidden.
const GRID_MIN_SCALING: f32 = 0.3;

/// Camera updates produced by canvas interaction.
#[derive(Debug, Clone)]
pub enum CanvasMessage {
    /// The view was panned to a new translation.
    Panned(Vector),
    /// The view was zoomed, optionally re-anchoring the translation so the
    /// zoom centers on the cursor.
    Zoomed {
        /// New zoom factor.
        scaling: f32,
        /// New translation keeping the point under the cursor fixed.
        translation: Option<Vector>,
    },
}

/// Retained canvas state owned by the app (drawing caches).
#[derive(Default)]
pub struct DiagramCanvas {
    grid_cache: Cache,
}

impl DiagramCanvas {
    /// Builds the canvas widget for this frame.
    pub fn view<'a>(
        &'a self,
        camera: &'a Camera,
        tokens: CanvasTokens,
    ) -> Element<'a, CanvasMessage> {
        Canvas::new(View {
            cache: &self.grid_cache,
            camera,
            tokens,
        })
        .width(Fill)
        .height(Fill)
        .into()
    }

    /// Drops cached geometry; call after the camera or theme changes.
    pub fn invalidate(&mut self) {
        self.grid_cache.clear();
    }
}

struct View<'a> {
    cache: &'a Cache,
    camera: &'a Camera,
    tokens: CanvasTokens,
}

/// Transient pointer interaction, owned by the widget between frames.
#[derive(Debug, Clone, Copy, Default)]
pub enum Interaction {
    /// Nothing in progress.
    #[default]
    None,
    /// Panning: the camera translation and cursor position at press time.
    Panning {
        /// Camera translation when the drag started.
        translation: Vector,
        /// Cursor position (viewport coordinates) when the drag started.
        start: Point,
    },
}

impl canvas::Program<CanvasMessage> for View<'_> {
    type State = Interaction;

    fn update(
        &self,
        interaction: &mut Interaction,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<CanvasMessage>> {
        if let Event::Mouse(mouse::Event::ButtonReleased(_)) = event {
            *interaction = Interaction::None;
        }

        let cursor_position = cursor.position_in(bounds)?;

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(
                mouse::Button::Left | mouse::Button::Middle,
            )) => {
                *interaction = Interaction::Panning {
                    translation: self.camera.translation,
                    start: cursor_position,
                };

                Some(canvas::Action::request_redraw().and_capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => match *interaction {
                Interaction::Panning { translation, start } => Some(
                    canvas::Action::publish(CanvasMessage::Panned(
                        translation + (cursor_position - start) * (1.0 / self.camera.scaling),
                    ))
                    .and_capture(),
                ),
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

                        // Keep the point under the cursor fixed while zooming.
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
        _interaction: &Interaction,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let grid = self.cache.draw(renderer, bounds.size(), |frame| {
            frame.fill_rectangle(Point::ORIGIN, frame.size(), self.tokens.bg);

            if self.camera.scaling < GRID_MIN_SCALING {
                return;
            }

            let size = frame.size();
            let center = Vector::new(size.width / 2.0, size.height / 2.0);

            frame.with_save(|frame| {
                frame.translate(center);
                frame.scale(self.camera.scaling);
                frame.translate(self.camera.translation);

                let region = self.camera.visible_region(size);
                let hairline = 1.0 / self.camera.scaling;

                let line_index = |coordinate: f32| (coordinate / GRID_STEP).round() as i64;

                let lines = |major: bool| {
                    Path::new(|path| {
                        let first_x = (region.x / GRID_STEP).floor() * GRID_STEP;
                        let mut x = first_x;
                        while x <= region.x + region.width {
                            if (line_index(x) % MAJOR_EVERY == 0) == major {
                                path.move_to(Point::new(x, region.y));
                                path.line_to(Point::new(x, region.y + region.height));
                            }
                            x += GRID_STEP;
                        }

                        let first_y = (region.y / GRID_STEP).floor() * GRID_STEP;
                        let mut y = first_y;
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
            });
        });

        vec![grid]
    }

    fn mouse_interaction(
        &self,
        interaction: &Interaction,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        match interaction {
            Interaction::Panning { .. } => mouse::Interaction::Grabbing,
            Interaction::None if cursor.is_over(bounds) => mouse::Interaction::default(),
            Interaction::None => mouse::Interaction::default(),
        }
    }
}
