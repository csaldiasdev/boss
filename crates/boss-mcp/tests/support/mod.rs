//! A headless stand-in for the boss app: services bridge requests against a
//! plain document with the same snapshot semantics as the app's edit session
//! (one undo step per changed apply; unchanged applies leave no trace).

use boss_mcp::{EditAction, EditorHandle, EditorRequest};
use bpmn_core::Document;

/// Spawns the editor loop and hands back the handle tools talk to.
pub fn editor(document: Document) -> EditorHandle {
    let (handle, mut requests) = boss_mcp::channel();
    tokio::spawn(async move {
        let mut doc = document;
        let mut undo: Vec<Document> = Vec::new();
        let mut redo: Vec<Document> = Vec::new();
        while let Some(request) = requests.recv().await {
            match request {
                EditorRequest::Snapshot { reply } => {
                    let _ = reply.send(doc.clone());
                }
                EditorRequest::Apply { action, reply } => {
                    let result = if action.is_history() {
                        let stepped = match action {
                            EditAction::Undo => undo.pop().map(|prev| {
                                redo.push(std::mem::replace(&mut doc, prev));
                            }),
                            _ => redo.pop().map(|next| {
                                undo.push(std::mem::replace(&mut doc, next));
                            }),
                        };
                        Ok(match stepped {
                            Some(()) => boss_mcp::ApplyOutcome::Changed,
                            None => boss_mcp::ApplyOutcome::Nothing,
                        })
                    } else {
                        let snapshot = doc.clone();
                        let result = boss_mcp::apply(action, &mut doc);
                        match &result {
                            Ok(outcome) if outcome.changed_document() => {
                                undo.push(snapshot);
                                redo.clear();
                            }
                            _ => doc = snapshot,
                        }
                        result
                    };
                    let _ = reply.send(result);
                }
            }
        }
    });
    handle
}
