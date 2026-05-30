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
                self.collab.pending_intent = Some(CollabIntent::StartServer);
                self.set_status("Starting local state server...");
                self.mark_full_redraw();
                Some(true)
            }
            "collab-connect" => {
                let addr = self.collab.server_address.clone();
                self.collab.pending_intent = Some(CollabIntent::Connect {
                    address: addr.clone(),
                });
                self.set_status(format!("Connecting to {}...", addr));
                self.mark_full_redraw();
                Some(true)
            }
            "collab-disconnect" => {
                self.collab.pending_intent = Some(CollabIntent::Disconnect);
                self.set_status("Disconnecting from state server...");
                self.mark_full_redraw();
                Some(true)
            }
            "collab-status" => {
                self.collab.pending_intent = Some(CollabIntent::ShowStatus);
                Some(true)
            }
            "collab-share" => {
                let buf_name = self.active_buffer().name.clone();
                self.collab.pending_intent = Some(CollabIntent::ShareBuffer {
                    buffer_name: buf_name.clone(),
                });
                self.set_status(format!("Sharing buffer: {}", buf_name));
                Some(true)
            }
            "collab-sync" => {
                let buf_name = self.active_buffer().name.clone();
                self.collab.pending_intent = Some(CollabIntent::ForceSync {
                    buffer_name: buf_name,
                });
                self.set_status("Force sync...");
                Some(true)
            }
            "collab-doctor" => {
                self.collab.pending_intent = Some(CollabIntent::Doctor);
                self.set_status("Running collab diagnostics...");
                Some(true)
            }
            "collab-list" => {
                self.collab.pending_intent = Some(CollabIntent::ListDocs);
                self.set_status("Listing shared documents...");
                Some(true)
            }
            "collab-join" => {
                // No-arg dispatch (SPC C j): fetch doc list and open picker palette.
                // :collab-join <name> is handled in command.rs before reaching here.
                self.collab.pending_intent = Some(CollabIntent::ListDocsForJoin);
                self.set_status("Fetching document list...");
                Some(true)
            }
            "kb-share" => {
                // Share the active KB (default = primary). The ex-command parser
                // can pass a name via :kb-share <name>, but SPC-key dispatch
                // uses "default" which maps to editor.kb.primary.
                let kb_name = self
                    .kb
                    .active_instance_name()
                    .unwrap_or_else(|| "default".to_string());
                self.collab.pending_intent = Some(CollabIntent::ShareKb {
                    kb_name: kb_name.clone(),
                    node_ids: vec![],
                });
                self.set_status(format!("Sharing KB '{}'...", kb_name));
                self.mark_full_redraw();
                Some(true)
            }
            "kb-join" => {
                // Join a KB — SPC-key dispatch uses active name or "default".
                // :kb-join <id> is handled in command.rs before reaching here.
                let kb_id = self
                    .kb
                    .active_instance_name()
                    .unwrap_or_else(|| "default".to_string());
                self.collab.pending_intent = Some(CollabIntent::JoinKb {
                    kb_id: kb_id.clone(),
                });
                self.set_status(format!("Joining KB '{}'...", kb_id));
                self.mark_full_redraw();
                Some(true)
            }
            "kb-leave" => {
                let kb_id = self
                    .kb
                    .active_instance_name()
                    .unwrap_or_else(|| "default".to_string());
                self.collab.pending_intent = Some(CollabIntent::LeaveKb {
                    kb_id: kb_id.clone(),
                });
                self.set_status(format!("Leaving KB '{}'...", kb_id));
                self.mark_full_redraw();
                Some(true)
            }
            "kb-list-remote" => {
                // Reuse existing ListDocs mechanism to show KB list
                self.collab.pending_intent = Some(CollabIntent::ListDocs);
                self.set_status("Listing remote KBs...");
                Some(true)
            }
            "collab-discover" => {
                self.collab.pending_intent = Some(CollabIntent::DiscoverPeers);
                self.set_status("Discovering peers on local network...");
                self.mark_full_redraw();
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
        match editor.collab.pending_intent {
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
                editor.collab.pending_intent,
                Some(CollabIntent::StartServer)
            ),
            "expected StartServer, got: {:?}",
            editor.collab.pending_intent
        );
    }

    #[test]
    fn dispatch_collab_unknown_returns_none() {
        let mut editor = Editor::new();
        let result = editor.dispatch_collab("unknown-command");
        assert_eq!(result, None);
        assert!(editor.collab.pending_intent.is_none());
    }

    #[test]
    fn dispatch_collab_discover_sets_intent() {
        let mut editor = Editor::new();
        let result = editor.dispatch_collab("collab-discover");
        assert_eq!(result, Some(true));
        assert!(
            matches!(
                editor.collab.pending_intent,
                Some(CollabIntent::DiscoverPeers)
            ),
            "expected DiscoverPeers, got: {:?}",
            editor.collab.pending_intent
        );
    }

    #[test]
    fn dispatch_collab_share_uses_active_buffer() {
        let mut editor = Editor::new();
        let expected_name = editor.active_buffer().name.clone();
        let result = editor.dispatch_collab("collab-share");
        assert_eq!(result, Some(true));
        match editor.collab.pending_intent {
            Some(CollabIntent::ShareBuffer { ref buffer_name }) => {
                assert_eq!(buffer_name, &expected_name);
            }
            other => panic!("expected ShareBuffer intent, got: {other:?}"),
        }
    }
}
