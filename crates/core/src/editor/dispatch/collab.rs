//! Collaborative editing command dispatch.
//!
//! Commands here set intent flags on the Editor that the binary event loop
//! drains (same pattern as LSP/DAP intents). The editor core doesn't own
//! the network connection -- it signals the binary to act.

use super::super::{CollabIntent, Editor};

impl Editor {
    /// Dispatch collaborative editing commands.
    /// Returns `Some(true)` if recognized and handled, `None` if not.
    pub(crate) fn dispatch_collab(&mut self, name: &str) -> Option<bool> {
        match name {
            "collab-start" => {
                self.pending_collab_intent = Some(CollabIntent::StartServer);
                self.set_status("Starting local state server...");
                self.mark_full_redraw();
                Some(true)
            }
            "collab-connect" => {
                let addr = self
                    .get_option("collab_server_address")
                    .map(|(v, _)| v)
                    .unwrap_or_else(|| "127.0.0.1:9473".to_string());
                self.pending_collab_intent = Some(CollabIntent::Connect {
                    address: addr.clone(),
                });
                self.set_status(format!("Connecting to {}...", addr));
                self.mark_full_redraw();
                Some(true)
            }
            "collab-disconnect" => {
                self.pending_collab_intent = Some(CollabIntent::Disconnect);
                self.set_status("Disconnecting from state server...");
                self.mark_full_redraw();
                Some(true)
            }
            "collab-status" => {
                self.pending_collab_intent = Some(CollabIntent::ShowStatus);
                Some(true)
            }
            "collab-share" => {
                let buf_name = self.active_buffer().name.clone();
                self.pending_collab_intent = Some(CollabIntent::ShareBuffer {
                    buffer_name: buf_name.clone(),
                });
                self.set_status(format!("Sharing buffer: {}", buf_name));
                Some(true)
            }
            "collab-sync" => {
                let buf_name = self.active_buffer().name.clone();
                self.pending_collab_intent = Some(CollabIntent::ForceSync {
                    buffer_name: buf_name,
                });
                self.set_status("Force sync...");
                Some(true)
            }
            "collab-doctor" => {
                self.pending_collab_intent = Some(CollabIntent::Doctor);
                self.set_status("Running collab diagnostics...");
                Some(true)
            }
            _ => None,
        }
    }
}
