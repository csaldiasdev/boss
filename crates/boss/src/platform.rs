//! Platform I/O: file dialogs on native and web.
//!
//! `rfd::AsyncFileDialog` covers opening on both targets. Saving uses the
//! native save dialog; on the web it is not available (M2 adds a download
//! fallback there).

/// A file picked by the user, decoded as UTF-8.
#[derive(Debug, Clone)]
pub struct OpenedFile {
    /// The file name (no path on the web).
    pub name: String,
    /// The file contents.
    pub contents: String,
}

/// Asks the user for a `.bpmn` file and reads it.
pub async fn open_file() -> Option<OpenedFile> {
    let handle = rfd::AsyncFileDialog::new()
        .add_filter("BPMN diagram", &["bpmn", "xml"])
        .set_title("Open BPMN diagram")
        .pick_file()
        .await?;

    let name = handle.file_name();
    let bytes = handle.read().await;
    let contents = String::from_utf8(bytes).ok()?;
    // Strip a UTF-8 BOM if some tool left one behind.
    let contents = contents
        .strip_prefix('\u{feff}')
        .map_or(contents.clone(), str::to_string);

    Some(OpenedFile { name, contents })
}

/// Reads a file straight from a path (CLI argument on native).
#[cfg(not(target_arch = "wasm32"))]
pub fn read_path(path: &str) -> Option<OpenedFile> {
    let name = std::path::Path::new(path)
        .file_name()
        .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned());
    let contents = std::fs::read_to_string(path).ok()?;
    Some(OpenedFile { name, contents })
}

/// Asks the user where to save and writes `contents`. Returns the chosen file
/// name. Native only; the web gets a download fallback in M2.
#[cfg(not(target_arch = "wasm32"))]
pub async fn save_file(file_name: String, contents: String) -> Option<String> {
    let handle = rfd::AsyncFileDialog::new()
        .add_filter("BPMN diagram", &["bpmn"])
        .set_file_name(&file_name)
        .set_title("Save BPMN diagram")
        .save_file()
        .await?;

    handle.write(contents.as_bytes()).await.ok()?;
    Some(handle.file_name())
}
