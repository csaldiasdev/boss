//! Zoom-independent text measurement and wrapping for canvas labels.
//!
//! iced's wgpu geometry backend scales a canvas text's font size with the
//! frame transform but not its `max_width`, so text with a finite wrap width
//! re-wraps as the camera zooms — and the tiny-skia backend disagrees, as it
//! scales both. The canvas therefore wraps label text itself, in world units,
//! and draws the result with an infinite wrap width: line breaks become a
//! property of the document, not of the zoom level, on every backend.

use iced::advanced::graphics::text::Paragraph;
use iced::advanced::text::{self, LineHeight, Paragraph as _, Shaping, Wrapping};
use iced::{Pixels, Size};

use crate::theme;

/// The line-height factor of `iced::widget::canvas::Text`; measurement must
/// match what the renderer will lay out.
const LINE_HEIGHT: f32 = 1.2;

/// Measures `content` as the canvas draws it — honoring its explicit
/// newlines, never adding more — in world units.
pub fn measure(content: &str, size: f32) -> Size {
    Paragraph::with_text(text::Text {
        content,
        bounds: Size::INFINITE,
        size: Pixels(size),
        line_height: LineHeight::Relative(LINE_HEIGHT),
        font: theme::SANS,
        align_x: text::Alignment::Default,
        align_y: iced::alignment::Vertical::Top,
        shaping: Shaping::default(),
        wrapping: Wrapping::None,
    })
    .min_bounds()
}

/// Greedily wraps `content` to at most `max_width` world units at `size`.
/// Explicit newlines are kept as hard breaks; a word that alone exceeds the
/// width keeps its own overflowing line. Returns the lines joined with '\n'.
pub fn wrap(content: &str, size: f32, max_width: f32) -> String {
    let mut lines = Vec::new();
    for paragraph in content.split('\n') {
        if measure(paragraph, size).width <= max_width {
            lines.push(paragraph.to_string());
            continue;
        }
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if line.is_empty() {
                line.push_str(word);
                continue;
            }
            let candidate = format!("{line} {word}");
            if measure(&candidate, size).width <= max_width {
                line = candidate;
            } else {
                lines.push(std::mem::take(&mut line));
                line.push_str(word);
            }
        }
        lines.push(line);
    }
    lines.join("\n")
}
