//! Bridges the MCP server into the iced runtime (native only).
//!
//! The whole server lives inside one iced subscription stream: it owns the
//! listener future and forwards editor requests from the tool handlers into
//! the update loop as messages. No shared mutable state anywhere — the update
//! loop is the single writer, exactly like user input.

use boss_mcp::EditorRequest;
use iced::Subscription;
use iced::futures::{SinkExt, Stream};
use std::sync::{Arc, Mutex};

/// An [`EditorRequest`] wrapped to satisfy iced's `Clone + Debug` message
/// bounds; the update loop takes it out exactly once.
#[derive(Clone)]
pub struct McpRequest(Arc<Mutex<Option<EditorRequest>>>);

impl McpRequest {
    fn new(request: EditorRequest) -> Self {
        Self(Arc::new(Mutex::new(Some(request))))
    }

    /// Takes the request out; `None` if something already took it.
    pub fn take(&self) -> Option<EditorRequest> {
        self.0.lock().ok()?.take()
    }
}

impl std::fmt::Debug for McpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpRequest")
    }
}

/// The MCP server as a subscription: active while the app subscribes to it.
pub fn subscription() -> Subscription<McpRequest> {
    Subscription::run(worker)
}

fn worker() -> impl Stream<Item = McpRequest> {
    iced::stream::channel(32, async |mut output| {
        let (handle, mut requests) = boss_mcp::channel();
        let server = boss_mcp::serve(boss_mcp::DEFAULT_PORT, handle);
        tokio::pin!(server);

        loop {
            tokio::select! {
                result = &mut server => {
                    log::error!("MCP server stopped: {result:?}");
                    break;
                }
                request = requests.recv() => match request {
                    Some(request) => {
                        let _ = output.send(McpRequest::new(request)).await;
                    }
                    None => break,
                },
            }
        }
    })
}
