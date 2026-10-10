//! The world/screen transform of the diagram canvas.
//!
//! Model geometry lives in world coordinates (the same space as BPMN DI). The
//! camera pans and zooms the view: screen = (world + translation) * scaling,
//! centered on the viewport.

use iced::{Point, Rectangle, Size, Vector};

/// Pan + zoom state of the canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// World-space offset of the view center.
    pub translation: Vector,
    /// Zoom factor (1.0 = 100%).
    pub scaling: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            translation: Vector::new(0.0, 0.0),
            scaling: 1.0,
        }
    }
}

impl Camera {
    /// Lowest zoom factor (10%).
    pub const MIN_SCALING: f32 = 0.1;
    /// Highest zoom factor (400%).
    pub const MAX_SCALING: f32 = 4.0;

    /// The world-space rectangle currently visible in a viewport of `size`.
    pub fn visible_region(&self, size: Size) -> Rectangle {
        let width = size.width / self.scaling;
        let height = size.height / self.scaling;

        Rectangle {
            x: -self.translation.x - width / 2.0,
            y: -self.translation.y - height / 2.0,
            width,
            height,
        }
    }

    /// Converts a position in viewport coordinates to world coordinates.
    // Unused in the binary until M1 hit-testing, but already exercised by tests.
    #[allow(dead_code)]
    pub fn project(&self, position: Point, size: Size) -> Point {
        let region = self.visible_region(size);

        Point::new(
            position.x / self.scaling + region.x,
            position.y / self.scaling + region.y,
        )
    }

    /// Multiplies the zoom by `factor`, clamped to the allowed range.
    pub fn zoomed(self, factor: f32) -> Self {
        Self {
            scaling: (self.scaling * factor).clamp(Self::MIN_SCALING, Self::MAX_SCALING),
            ..self
        }
    }

    /// The zoom factor as a whole percentage.
    #[expect(clippy::cast_sign_loss, reason = "scaling is clamped positive")]
    pub fn zoom_percent(&self) -> u32 {
        (self.scaling * 100.0).round() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_round_trips_the_center() {
        let camera = Camera {
            translation: Vector::new(40.0, -25.0),
            scaling: 2.0,
        };
        let size = Size::new(800.0, 600.0);

        let world = camera.project(Point::new(400.0, 300.0), size);
        assert!((world.x - -40.0).abs() < f32::EPSILON * 100.0);
        assert!((world.y - 25.0).abs() < f32::EPSILON * 100.0);
    }

    #[test]
    fn zoom_is_clamped() {
        let camera = Camera::default();
        assert_eq!(camera.zoomed(100.0).scaling, Camera::MAX_SCALING);
        assert_eq!(camera.zoomed(0.001).scaling, Camera::MIN_SCALING);
    }
}
