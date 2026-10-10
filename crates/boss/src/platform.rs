//! Platform I/O: file dialogs on native, download fallback on the web.

/// A file picked by the user, decoded as UTF-8.
#[derive(Debug, Clone)]
pub struct OpenedFile {
    /// The file name (no path on the web).
    pub name: String,
    /// The file contents.
    pub contents: String,
    /// The on-disk location, when the platform exposes one.
    #[cfg(not(target_arch = "wasm32"))]
    pub path: Option<std::path::PathBuf>,
}

/// The outcome of a successful save.
#[derive(Debug, Clone)]
pub struct SavedFile {
    /// The file name the user ended up with.
    pub name: String,
    /// Where it landed, for direct re-saving (native only).
    #[cfg(not(target_arch = "wasm32"))]
    pub path: Option<std::path::PathBuf>,
}

fn strip_bom(contents: &str) -> String {
    contents
        .strip_prefix('\u{feff}')
        .unwrap_or(contents)
        .to_string()
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
    let contents = strip_bom(&String::from_utf8(bytes).ok()?);

    Some(OpenedFile {
        name,
        contents,
        #[cfg(not(target_arch = "wasm32"))]
        path: Some(handle.path().to_path_buf()),
    })
}

/// Reads a file straight from a path (CLI argument on native).
#[cfg(not(target_arch = "wasm32"))]
pub fn read_path(path: &str) -> Option<OpenedFile> {
    let path_buf = std::path::PathBuf::from(path);
    let name = path_buf
        .file_name()
        .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned());
    let contents = strip_bom(&std::fs::read_to_string(&path_buf).ok()?);
    Some(OpenedFile {
        name,
        contents,
        path: Some(path_buf),
    })
}

/// Writes directly to a known path (plain Save on native).
#[cfg(not(target_arch = "wasm32"))]
pub fn write_path(path: &std::path::Path, contents: &str) -> Option<SavedFile> {
    std::fs::write(path, contents).ok()?;
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    Some(SavedFile {
        name,
        path: Some(path.to_path_buf()),
    })
}

/// Asks the user where to save and writes `contents` (Save As on native).
#[cfg(not(target_arch = "wasm32"))]
pub async fn save_file(file_name: String, contents: String) -> Option<SavedFile> {
    let handle = rfd::AsyncFileDialog::new()
        .add_filter("BPMN diagram", &["bpmn"])
        .set_file_name(&file_name)
        .set_title("Save BPMN diagram")
        .save_file()
        .await?;

    handle.write(contents.as_bytes()).await.ok()?;
    Some(SavedFile {
        name: handle.file_name(),
        path: Some(handle.path().to_path_buf()),
    })
}

/// Hands the file to the browser as a download — browsers offer no real
/// "Save As" dialog to sandboxed pages.
#[cfg(target_arch = "wasm32")]
pub fn save_file(file_name: &str, contents: &str) -> Option<SavedFile> {
    use wasm_bindgen::JsCast;

    let parts = js_sys::Array::new();
    parts.push(&wasm_bindgen::JsValue::from_str(contents));
    let blob = web_sys::Blob::new_with_str_sequence(&parts).ok()?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).ok()?;

    let document = web_sys::window()?.document()?;
    let anchor: web_sys::HtmlAnchorElement = document.create_element("a").ok()?.dyn_into().ok()?;
    anchor.set_href(&url);
    anchor.set_download(file_name);
    anchor.click();
    let _ = web_sys::Url::revoke_object_url(&url);

    Some(SavedFile {
        name: file_name.to_string(),
    })
}
