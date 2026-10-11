//! Native menu bar (macOS and Windows) via `muda`.
//!
//! The menu mirrors the toolbar and keyboard shortcuts one to one: every item
//! maps onto an existing [`Message`]. Events flow from muda's global handler
//! into a channel drained by an iced subscription — the update loop stays the
//! single writer. On macOS the menu attaches to the application; on Windows it
//! attaches to the window once its handle exists. Linux is not supported
//! (muda needs GTK there, which iced/winit does not use), so the toolbar
//! remains the only chrome on Linux and the web.

use crate::app::Message;
use iced::Subscription;
use iced::futures::{SinkExt, Stream};
use muda::accelerator::Accelerator;
use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use std::sync::{Mutex, OnceLock};
use tokio::sync::mpsc;

/// The subscription side of the menu-event channel; created on install,
/// taken exactly once by the subscription worker.
static EVENTS: OnceLock<Mutex<Option<mpsc::UnboundedReceiver<String>>>> = OnceLock::new();

fn accelerator(keys: &str) -> Option<Accelerator> {
    keys.parse().ok()
}

fn item(id: &str, label: &str, keys: Option<&str>) -> MenuItem {
    MenuItem::with_id(id, label, true, keys.and_then(accelerator))
}

/// Builds the menu bar. Keyboard accelerators mirror the in-app shortcuts;
/// on macOS the menu intercepts them, firing one menu event instead of a key
/// event, so nothing dispatches twice.
fn build() -> muda::Result<Menu> {
    let menu = Menu::new();

    #[cfg(target_os = "macos")]
    {
        let app = Submenu::new("boss", true);
        app.append_items(&[
            &PredefinedMenuItem::about(None, None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::hide(None),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::quit(None),
        ])?;
        menu.append(&app)?;
    }

    let file = Submenu::new("&File", true);
    file.append_items(&[
        &item("file.new", "New", Some("CmdOrCtrl+N")),
        &item("file.open", "Open…", Some("CmdOrCtrl+O")),
        &PredefinedMenuItem::separator(),
        &item("file.save", "Save", Some("CmdOrCtrl+S")),
        &item("file.save-as", "Save As…", Some("CmdOrCtrl+Shift+S")),
        &PredefinedMenuItem::separator(),
        &item("file.export-svg", "Export SVG…", None),
    ])?;

    let edit = Submenu::new("&Edit", true);
    edit.append_items(&[
        &item("edit.undo", "Undo", Some("CmdOrCtrl+Z")),
        &item("edit.redo", "Redo", Some("CmdOrCtrl+Shift+Z")),
        &PredefinedMenuItem::separator(),
        &item("edit.cut", "Cut", Some("CmdOrCtrl+X")),
        &item("edit.copy", "Copy", Some("CmdOrCtrl+C")),
        &item("edit.paste", "Paste", Some("CmdOrCtrl+V")),
        &item("edit.delete", "Delete", None),
        &PredefinedMenuItem::separator(),
        &item("edit.select-all", "Select All", Some("CmdOrCtrl+A")),
        &item("edit.rename", "Rename", None),
    ])?;

    let view = Submenu::new("&View", true);
    view.append_items(&[
        &item("view.zoom-in", "Zoom In", Some("CmdOrCtrl+=")),
        &item("view.zoom-out", "Zoom Out", Some("CmdOrCtrl+-")),
        &item("view.zoom-fit", "Zoom to Fit", Some("CmdOrCtrl+0")),
        &PredefinedMenuItem::separator(),
        &item("view.toggle-theme", "Toggle Dark Mode", None),
        &item("view.toggle-properties", "Toggle Properties Panel", None),
    ])?;

    let server = Submenu::new("&Server", true);
    server.append_items(&[&item("server.mcp", "Start MCP Server", None)])?;

    menu.append_items(&[&file, &edit, &view, &server])?;
    Ok(menu)
}

/// Installs the global event handler and the channel the subscription
/// drains. Called once from `App::boot`; the menu itself attaches later, on
/// the first `WindowOpened` — attaching from `boot` is too early on macOS,
/// where winit installs its own default menu when the app finishes launching
/// and would overwrite ours.
pub fn install() {
    let (sender, receiver) = mpsc::unbounded_channel();
    let _ = EVENTS.set(Mutex::new(Some(receiver)));
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let _ = sender.send(event.id().0.clone());
    }));
}

/// Attaches the menu bar to the application (macOS). Idempotent.
#[cfg(target_os = "macos")]
pub fn attach() {
    static ATTACHED: OnceLock<()> = OnceLock::new();
    if ATTACHED.set(()).is_err() {
        return;
    }
    match build() {
        Ok(menu) => {
            menu.init_for_nsapp();
            // The menu must outlive the app; macOS holds only weak refs.
            std::mem::forget(menu);
        }
        Err(error) => log::error!("could not build the menu bar: {error}"),
    }
}

/// Attaches the menu to the main window (Windows only; macOS menus are
/// per-application and already installed). Runs inside `iced::window::run`,
/// on the event-loop thread.
#[cfg(target_os = "windows")]
pub fn attach_to_window(window: &dyn iced::window::raw_window_handle::HasWindowHandle) {
    use iced::window::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    match build() {
        Ok(menu) => {
            // SAFETY: the handle comes from the live window on this thread.
            if let Err(error) = unsafe { menu.init_for_hwnd(win32.hwnd.get()) } {
                log::error!("could not attach the menu bar: {error}");
            }
            std::mem::forget(menu);
        }
        Err(error) => log::error!("could not build the menu bar: {error}"),
    }
}

/// Menu events as a stream of app messages.
pub fn subscription() -> Subscription<Message> {
    Subscription::run(worker)
}

fn worker() -> impl Stream<Item = Message> {
    iced::stream::channel(16, async |mut output| {
        let Some(mut receiver) = EVENTS.get().and_then(|slot| slot.lock().ok()?.take()) else {
            return;
        };
        while let Some(id) = receiver.recv().await {
            if let Some(message) = message_for(&id) {
                let _ = output.send(message).await;
            }
        }
    })
}

fn message_for(id: &str) -> Option<Message> {
    Some(match id {
        "file.new" => Message::NewRequested,
        "file.open" => Message::OpenRequested,
        "file.save" => Message::SaveRequested,
        "file.save-as" => Message::SaveAsRequested,
        "file.export-svg" => Message::ExportSvg,
        "edit.undo" => Message::Undo,
        "edit.redo" => Message::Redo,
        "edit.cut" => Message::Cut,
        "edit.copy" => Message::Copy,
        "edit.paste" => Message::Paste,
        "edit.delete" => Message::DeleteSelection,
        "edit.select-all" => Message::SelectAll,
        "edit.rename" => Message::RenameSelection,
        "view.zoom-in" => Message::ZoomIn,
        "view.zoom-out" => Message::ZoomOut,
        "view.zoom-fit" => Message::ZoomFit,
        "view.toggle-theme" => Message::ToggleTheme,
        "view.toggle-properties" => Message::ToggleProperties,
        "server.mcp" => Message::McpStart,
        _ => return None,
    })
}
