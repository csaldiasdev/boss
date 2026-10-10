//! Geometry types shared by the DI model and the editor.
//!
//! These mirror the BPMN DI `dc:Point` and `dc:Bounds` elements and use `f64`
//! like the spec. They deliberately do not depend on any UI toolkit.

/// A point in diagram (world) coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    /// Horizontal coordinate.
    pub x: f64,
    /// Vertical coordinate.
    pub y: f64,
}

impl Point {
    /// Creates a point.
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// A size in diagram (world) coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    /// Horizontal extent.
    pub width: f64,
    /// Vertical extent.
    pub height: f64,
}

impl Size {
    /// Creates a size.
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

/// An axis-aligned rectangle in diagram (world) coordinates, as in `dc:Bounds`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Bounds {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Horizontal extent.
    pub width: f64,
    /// Vertical extent.
    pub height: f64,
}

impl Bounds {
    /// Creates a rectangle from its top-left corner and size.
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The center of the rectangle.
    pub fn center(&self) -> Point {
        Point::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    /// Whether `point` lies inside the rectangle (edges inclusive).
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.x
            && point.x <= self.x + self.width
            && point.y >= self.y
            && point.y <= self.y + self.height
    }

    /// Whether this rectangle and `other` overlap at all.
    pub fn intersects(&self, other: &Bounds) -> bool {
        self.x < other.x + other.width
            && other.x < self.x + self.width
            && self.y < other.y + other.height
            && other.y < self.y + self.height
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_and_containment() {
        let b = Bounds::new(10.0, 20.0, 100.0, 50.0);
        assert_eq!(b.center(), Point::new(60.0, 45.0));
        assert!(b.contains(Point::new(10.0, 20.0)));
        assert!(b.contains(Point::new(110.0, 70.0)));
        assert!(!b.contains(Point::new(111.0, 45.0)));
    }

    #[test]
    fn intersection() {
        let a = Bounds::new(0.0, 0.0, 50.0, 50.0);
        assert!(a.intersects(&Bounds::new(40.0, 40.0, 50.0, 50.0)));
        assert!(!a.intersects(&Bounds::new(51.0, 0.0, 10.0, 10.0)));
    }
}
