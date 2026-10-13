//! boss — a truly open-source BPMN editor.

mod app;
mod camera;
mod canvas;
mod icons;
#[cfg(not(target_arch = "wasm32"))]
mod mcp_bridge;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod menu;
mod platform;
mod render;
mod session;
mod svg;
mod text_layout;
mod theme;

use app::App;

/// The window settings: on macOS the title bar is transparent and the content
/// extends under it, so the toolbar strip doubles as the (draggable) chrome.
fn window_settings() -> iced::window::Settings {
    iced::window::Settings {
        size: iced::Size::new(1280.0, 800.0),
        #[cfg(target_os = "macos")]
        platform_specific: iced::window::settings::PlatformSpecific {
            title_hidden: true,
            titlebar_transparent: true,
            fullsize_content_view: true,
        },
        ..iced::window::Settings::default()
    }
}

/// Entry point on native and wasm: runs the iced application.
pub fn main() -> iced::Result {
    #[cfg(target_arch = "wasm32")]
    console_error_panic_hook::set_once();

    iced::application(App::boot, App::update, App::view)
        .title(App::title)
        .theme(App::theme)
        .subscription(App::subscription)
        .antialiasing(true)
        .font(theme::FONT_SANS_REGULAR)
        .font(theme::FONT_SANS_MEDIUM)
        .font(theme::FONT_SANS_SEMIBOLD)
        .font(theme::FONT_MONO_REGULAR)
        .font(icons::FONT_BYTES)
        .default_font(theme::SANS)
        .window(window_settings())
        .run()
}
