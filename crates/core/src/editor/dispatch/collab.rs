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
                let addr = self.collab_server_address.clone();
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

#[cfg(test)]
mod tests {
    use super::super::super::{CollabIntent, Editor};

    #[test]
    fn dispatch_collab_connect_sets_intent() {
        let mut editor = Editor::new();
        let result = editor.dispatch_collab("collab-connect");
        assert_eq!(result, Some(true));
        match editor.pending_collab_intent {
            Some(CollabIntent::Connect { ref address }) => {
                assert_eq!(address, "127.0.0.1:9473");
            }
            other => panic!("expected Connect intent, got: {other:?}"),
        }
    }

    #[test]
    fn dispatch_collab_start_sets_intent() {
        let mut editor = Editor::new();
        let result = editor.dispatch_collab("collab-start");
        assert_eq!(result, Some(true));
        assert!(
            matches!(
                editor.pending_collab_intent,
                Some(CollabIntent::StartServer)
            ),
            "expected StartServer, got: {:?}",
            editor.pending_collab_intent
        );
    }

    #[test]
    fn dispatch_collab_unknown_returns_none() {
        let mut editor = Editor::new();
        let result = editor.dispatch_collab("unknown-command");
        assert_eq!(result, None);
        assert!(editor.pending_collab_intent.is_none());
    }

    #[test]
    fn dispatch_collab_share_uses_active_buffer() {
        let mut editor = Editor::new();
        let expected_name = editor.active_buffer().name.clone();
        let result = editor.dispatch_collab("collab-share");
        assert_eq!(result, Some(true));
        match editor.pending_collab_intent {
            Some(CollabIntent::ShareBuffer { ref buffer_name }) => {
                assert_eq!(buffer_name, &expected_name);
            }
            other => panic!("expected ShareBuffer intent, got: {other:?}"),
        }
    }
}
