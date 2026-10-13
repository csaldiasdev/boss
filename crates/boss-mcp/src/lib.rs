//! MCP server exposing the boss editor to AI agents.
//!
//! Serves the Model Context Protocol over streamable HTTP on localhost so an
//! agent can inspect and edit the open document while the user watches. Talks
//! to the editor exclusively through a channel-based handle (single writer:
//! the app's update loop), never through shared mutable state.

mod apply;
mod bridge;
mod server;

pub use apply::apply;
pub use bridge::{ApplyOutcome, EditAction, EditorHandle, EditorRequest, channel};
pub use server::{BossMcp, DEFAULT_PORT, serve};
