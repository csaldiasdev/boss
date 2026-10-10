//! boss — a truly open-source BPMN editor.

mod app;
mod camera;
mod canvas;
mod platform;
mod render;
mod theme;

use app::App;

/// Entry point on native and wasm: runs the iced application.
pub fn main() -> iced::Result {
    #[cfg(target_arch = "wasm32")]
    console_error_panic_hook::set_once();

    iced::application(App::boot, App::update, App::view)
        .title(App::title)
        .theme(App::theme)
        .antialiasing(true)
        .font(theme::FONT_SANS_REGULAR)
        .font(theme::FONT_SANS_MEDIUM)
        .font(theme::FONT_SANS_SEMIBOLD)
        .font(theme::FONT_MONO_REGULAR)
        .default_font(theme::SANS)
        .window_size((1280.0, 800.0))
        .run()
}
