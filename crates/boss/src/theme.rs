//! The "Slate" design system tokens and widget styles.
//!
//! This module is the sole source of colors in the codebase: nothing outside
//! it hardcodes a color, so restyling never touches editor logic.

use iced::theme::Palette;
use iced::widget::{button, container};
use iced::{Border, Color, Font, Theme, font};

/// IBM Plex Sans Regular, embedded so native and wasm render identically.
pub const FONT_SANS_REGULAR: &[u8] =
    include_bytes!("../../../assets/fonts/IBMPlexSans-Regular.ttf");
/// IBM Plex Sans Medium.
pub const FONT_SANS_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexSans-Medium.ttf");
/// IBM Plex Sans `SemiBold`.
pub const FONT_SANS_SEMIBOLD: &[u8] =
    include_bytes!("../../../assets/fonts/IBMPlexSans-SemiBold.ttf");
/// IBM Plex Mono Regular.
pub const FONT_MONO_REGULAR: &[u8] =
    include_bytes!("../../../assets/fonts/IBMPlexMono-Regular.ttf");

/// The UI typeface.
pub const SANS: Font = Font::with_name("IBM Plex Sans");
/// The medium weight of the UI typeface (section titles).
pub const SANS_MEDIUM: Font = Font {
    weight: font::Weight::Medium,
    ..SANS
};
/// The monospace typeface (ids, XML, zoom readouts).
pub const MONO: Font = Font::with_name("IBM Plex Mono");

/// Light or dark. Both are equal citizens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Light theme.
    #[default]
    Light,
    /// Dark theme.
    Dark,
}

impl Mode {
    /// The other mode.
    pub fn toggled(self) -> Self {
        match self {
            Mode::Light => Mode::Dark,
            Mode::Dark => Mode::Light,
        }
    }
}

/// The fill-and-stroke pair of one BPMN element family. `fill_top` is a
/// slightly lighter fill for the top of the depth gradient.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElementColors {
    /// Shape fill (gradient bottom).
    pub fill: Color,
    /// Gradient top, a touch lighter than `fill`.
    pub fill_top: Color,
    /// Shape outline and inner icon strokes.
    pub stroke: Color,
}

/// Diagram canvas colors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanvasTokens {
    /// Diagram background.
    pub bg: Color,
    /// Minor grid lines (20 px step).
    pub grid: Color,
    /// Major grid lines (every 100 px).
    pub grid_major: Color,
    /// BPMN shape outlines, flows, markers.
    pub stroke: Color,
    /// BPMN shape fills.
    pub fill: Color,
    /// Element names.
    pub label: Color,
    /// Flow labels and event captions.
    pub edge_label: Color,
    /// Selection outlines, marquee, and interaction ghosts (the accent).
    pub selection: Color,
    /// Validation warning badges.
    pub warning: Color,
    /// Validation error badges.
    pub danger: Color,
    /// Drop shadow under elevated shapes.
    pub shadow: Color,
    /// Start events (green).
    pub start: ElementColors,
    /// End events (red).
    pub end: ElementColors,
    /// Intermediate and boundary events (amber).
    pub intermediate: ElementColors,
    /// Tasks (blue).
    pub task: ElementColors,
    /// Gateways (yellow).
    pub gateway: ElementColors,
    /// Data elements and artifacts (slate).
    pub data: ElementColors,
    /// Containers: subprocesses, call activities, pool bands (neutral).
    pub container: ElementColors,
}

/// The full Slate token set for one mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tokens {
    /// App background (behind panels).
    pub bg: Color,
    /// Panels, toolbar, cards, inputs.
    pub surface: Color,
    /// Hairlines, panel borders, input borders.
    pub border: Color,
    /// Primary text.
    pub text: Color,
    /// Secondary text and captions.
    pub muted: Color,
    /// Selection, primary buttons, focus.
    pub accent: Color,
    /// Text on accent fills.
    pub accent_text: Color,
    /// Valid state, confirmations.
    pub success: Color,
    /// Validation warnings, badges.
    pub warning: Color,
    /// Errors, destructive actions.
    pub danger: Color,
    /// Diagram canvas colors.
    pub canvas: CanvasTokens,
}

const LIGHT: Tokens = Tokens {
    bg: Color::from_rgb8(0xf2, 0xf4, 0xf7),
    surface: Color::from_rgb8(0xff, 0xff, 0xff),
    border: Color::from_rgb8(0xd7, 0xdc, 0xe3),
    text: Color::from_rgb8(0x1b, 0x21, 0x29),
    muted: Color::from_rgb8(0x5d, 0x67, 0x73),
    accent: Color::from_rgb8(0x25, 0x63, 0xeb),
    accent_text: Color::from_rgb8(0xff, 0xff, 0xff),
    success: Color::from_rgb8(0x1a, 0x7f, 0x4e),
    warning: Color::from_rgb8(0xb2, 0x6a, 0x0c),
    danger: Color::from_rgb8(0xc2, 0x36, 0x2f),
    canvas: CanvasTokens {
        bg: Color::from_rgb8(0xfa, 0xfb, 0xfc),
        grid: Color::from_rgb8(0xe6, 0xea, 0xef),
        grid_major: Color::from_rgb8(0xdd, 0xe2, 0xe9),
        stroke: Color::from_rgb8(0x39, 0x43, 0x4f),
        fill: Color::from_rgb8(0xff, 0xff, 0xff),
        label: Color::from_rgb8(0x1b, 0x21, 0x29),
        edge_label: Color::from_rgb8(0x5d, 0x67, 0x73),
        selection: Color::from_rgb8(0x25, 0x63, 0xeb),
        warning: Color::from_rgb8(0xb2, 0x6a, 0x0c),
        danger: Color::from_rgb8(0xc2, 0x36, 0x2f),
        shadow: Color::from_rgba8(0x1b, 0x21, 0x29, 0.14),
        start: ElementColors {
            fill: Color::from_rgb8(0xdc, 0xf5, 0xe7),
            fill_top: Color::from_rgb8(0xef, 0xfc, 0xf5),
            stroke: Color::from_rgb8(0x13, 0x7a, 0x4e),
        },
        end: ElementColors {
            fill: Color::from_rgb8(0xfd, 0xe4, 0xe2),
            fill_top: Color::from_rgb8(0xfe, 0xf3, 0xf2),
            stroke: Color::from_rgb8(0xbe, 0x35, 0x2d),
        },
        intermediate: ElementColors {
            fill: Color::from_rgb8(0xfd, 0xf0, 0xd4),
            fill_top: Color::from_rgb8(0xfe, 0xf9, 0xea),
            stroke: Color::from_rgb8(0xb2, 0x6a, 0x0c),
        },
        task: ElementColors {
            fill: Color::from_rgb8(0xe3, 0xed, 0xfd),
            fill_top: Color::from_rgb8(0xf2, 0xf7, 0xfe),
            stroke: Color::from_rgb8(0x2a, 0x5c, 0xc8),
        },
        gateway: ElementColors {
            fill: Color::from_rgb8(0xfc, 0xf4, 0xc7),
            fill_top: Color::from_rgb8(0xfe, 0xfa, 0xe3),
            stroke: Color::from_rgb8(0xa1, 0x7c, 0x0a),
        },
        data: ElementColors {
            fill: Color::from_rgb8(0xee, 0xf1, 0xf5),
            fill_top: Color::from_rgb8(0xf8, 0xfa, 0xfc),
            stroke: Color::from_rgb8(0x54, 0x5f, 0x6d),
        },
        container: ElementColors {
            fill: Color::from_rgb8(0xf5, 0xf7, 0xfa),
            fill_top: Color::from_rgb8(0xfb, 0xfc, 0xfd),
            stroke: Color::from_rgb8(0x45, 0x50, 0x5e),
        },
    },
};

const DARK: Tokens = Tokens {
    bg: Color::from_rgb8(0x14, 0x17, 0x1b),
    surface: Color::from_rgb8(0x1d, 0x21, 0x27),
    border: Color::from_rgb8(0x33, 0x3a, 0x44),
    text: Color::from_rgb8(0xe5, 0xe9, 0xee),
    muted: Color::from_rgb8(0x98, 0xa2, 0xae),
    accent: Color::from_rgb8(0x6a, 0x99, 0xf5),
    accent_text: Color::from_rgb8(0x0e, 0x11, 0x16),
    success: Color::from_rgb8(0x4f, 0xbf, 0x84),
    warning: Color::from_rgb8(0xe8, 0xa1, 0x3c),
    danger: Color::from_rgb8(0xe5, 0x73, 0x6b),
    canvas: CanvasTokens {
        bg: Color::from_rgb8(0x18, 0x1c, 0x21),
        grid: Color::from_rgb8(0x24, 0x2a, 0x32),
        grid_major: Color::from_rgb8(0x2b, 0x32, 0x3c),
        stroke: Color::from_rgb8(0xc2, 0xcb, 0xd6),
        fill: Color::from_rgb8(0x22, 0x27, 0x30),
        label: Color::from_rgb8(0xe5, 0xe9, 0xee),
        edge_label: Color::from_rgb8(0x98, 0xa2, 0xae),
        selection: Color::from_rgb8(0x6a, 0x99, 0xf5),
        warning: Color::from_rgb8(0xe8, 0xa1, 0x3c),
        danger: Color::from_rgb8(0xe5, 0x73, 0x6b),
        shadow: Color::from_rgba8(0x00, 0x00, 0x00, 0.35),
        start: ElementColors {
            fill: Color::from_rgb8(0x12, 0x30, 0x24),
            fill_top: Color::from_rgb8(0x18, 0x3b, 0x2c),
            stroke: Color::from_rgb8(0x53, 0xc5, 0x8b),
        },
        end: ElementColors {
            fill: Color::from_rgb8(0x3a, 0x1d, 0x1c),
            fill_top: Color::from_rgb8(0x45, 0x24, 0x23),
            stroke: Color::from_rgb8(0xe5, 0x73, 0x6b),
        },
        intermediate: ElementColors {
            fill: Color::from_rgb8(0x38, 0x29, 0x12),
            fill_top: Color::from_rgb8(0x42, 0x32, 0x18),
            stroke: Color::from_rgb8(0xe8, 0xa1, 0x3c),
        },
        task: ElementColors {
            fill: Color::from_rgb8(0x1a, 0x2a, 0x45),
            fill_top: Color::from_rgb8(0x20, 0x33, 0x52),
            stroke: Color::from_rgb8(0x7d, 0xa5, 0xf0),
        },
        gateway: ElementColors {
            fill: Color::from_rgb8(0x33, 0x2d, 0x10),
            fill_top: Color::from_rgb8(0x3d, 0x36, 0x16),
            stroke: Color::from_rgb8(0xdd, 0xc2, 0x4a),
        },
        data: ElementColors {
            fill: Color::from_rgb8(0x25, 0x2b, 0x34),
            fill_top: Color::from_rgb8(0x2b, 0x32, 0x3c),
            stroke: Color::from_rgb8(0x9d, 0xa9, 0xb7),
        },
        container: ElementColors {
            fill: Color::from_rgb8(0x1e, 0x24, 0x2c),
            fill_top: Color::from_rgb8(0x23, 0x2a, 0x33),
            stroke: Color::from_rgb8(0x8d, 0x99, 0xa8),
        },
    },
};

/// The right-click placement menu: a floating card with a soft shadow.
pub fn context_menu(t: Tokens) -> container::Style {
    container::Style {
        background: Some(t.surface.into()),
        border: Border {
            color: t.border,
            width: 1.0,
            radius: 8.0.into(),
        },
        shadow: iced::Shadow {
            color: Color {
                a: 0.25,
                ..Color::BLACK
            },
            offset: iced::Vector::new(0.0, 4.0),
            blur_radius: 16.0,
        },
        ..container::Style::default()
    }
}

/// The Slate tokens for `mode`.
pub fn tokens(mode: Mode) -> Tokens {
    match mode {
        Mode::Light => LIGHT,
        Mode::Dark => DARK,
    }
}

/// Builds the iced [`Theme`] for `mode` from the Slate tokens.
pub fn iced_theme(mode: Mode) -> Theme {
    let t = tokens(mode);
    let name = match mode {
        Mode::Light => "Slate Light",
        Mode::Dark => "Slate Dark",
    };

    Theme::custom(
        name,
        Palette {
            background: t.bg,
            text: t.text,
            primary: t.accent,
            success: t.success,
            warning: t.warning,
            danger: t.danger,
        },
    )
}

/// Style for side panels and the toolbar: surface fill, hairline border.
pub fn panel(t: Tokens) -> container::Style {
    container::Style {
        text_color: Some(t.text),
        background: Some(t.surface.into()),
        border: Border {
            color: t.border,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

/// Style for toolbar buttons: surface fill, hairline border, 6 px radius.
pub fn tool_button(t: Tokens) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let background = match status {
            button::Status::Hovered | button::Status::Pressed => t.bg,
            _ => t.surface,
        };

        button::Style {
            background: Some(background.into()),
            text_color: t.text,
            border: Border {
                color: t.border,
                width: 1.0,
                radius: 6.0.into(),
            },
            ..button::Style::default()
        }
    }
}

/// Style for palette entries; the active tool gets an accent outline.
pub fn palette_button(t: Tokens, active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let background = match status {
            button::Status::Hovered | button::Status::Pressed => t.bg,
            _ => t.surface,
        };

        button::Style {
            background: Some(background.into()),
            text_color: if active { t.accent } else { t.text },
            border: Border {
                color: if active { t.accent } else { t.border },
                width: 1.0,
                radius: 6.0.into(),
            },
            ..button::Style::default()
        }
    }
}

/// Style for rows in the issues panel: flat, hover tint only.
pub fn issue_row(t: Tokens) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let background = match status {
            button::Status::Hovered | button::Status::Pressed => t.bg,
            _ => t.surface,
        };
        button::Style {
            background: Some(background.into()),
            text_color: t.text,
            border: Border {
                radius: 4.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    }
}
