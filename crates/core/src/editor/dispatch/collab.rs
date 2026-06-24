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
                self.set_status("Starting local daemon...");
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
                self.set_status("Disconnecting from daemon...");
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
            "kb-share-p2p" => {
                // P2P "magnet link" mint. Unlike kb-share (queued over the collab
                // TCP stream), this is a SYNCHRONOUS daemon control-socket call
                // that returns the ticket immediately (ADR-025 §"Driving
                // surfaces" — same backend as the Scheme primitive + MCP tool).
                let kb_id = self
                    .kb
                    .active_instance_name()
                    .unwrap_or_else(|| "default".to_string());
                match self.kb.share_p2p(&kb_id) {
                    Ok(ticket) => {
                        // Surface via the attention bus → mirrored to *Messages*
                        // so both the human and the AI peer can copy the full link
                        // (the status line would truncate it).
                        self.notify(
                            crate::notifications::Notification::success(
                                "collab",
                                format!("P2P join link ready for KB '{kb_id}'"),
                            )
                            .body(format!(
                                "Share with a peer (they run kb-join / kb_join):\n{ticket}"
                            ))
                            .key(format!("p2p-share:{kb_id}")),
                        );
                        self.set_status(format!("P2P join link for '{kb_id}' → *Messages*"));
                    }
                    Err(e) => self.set_status(format!("kb-share-p2p: {e}")),
                }
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
                let node_svs = self.kb_join_node_svs(&kb_id);
                self.collab.pending_intent = Some(CollabIntent::JoinKb {
                    kb_id: kb_id.clone(),
                    node_svs,
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
            "kb-member-add" | "kb-member-remove" => {
                // :kb-member-add <kb-id> <fingerprint> [role]  (args via command_line).
                let line = self.vi.command_line.trim().to_string();
                let mut parts = line.split_whitespace();
                let kb_id = parts.next().unwrap_or("").to_string();
                let member = parts.next().unwrap_or("").to_string();
                let role = parts.next().unwrap_or("editor").to_string();
                if member.is_empty() {
                    // No fingerprint to type by hand → open the *KB Sharing* buffer,
                    // where members are picked at-point (the canonical pick surface).
                    self.open_kb_sharing();
                    self.set_status(
                        "Pick a member in *KB Sharing* (e/v/o = role, x = remove)".to_string(),
                    );
                    return Some(true);
                }
                let add = name == "kb-member-add";
                self.collab.pending_intent = Some(if add {
                    CollabIntent::KbAddMember {
                        kb_id: kb_id.clone(),
                        member: member.clone(),
                        role,
                    }
                } else {
                    CollabIntent::KbRemoveMember {
                        kb_id: kb_id.clone(),
                        member: member.clone(),
                    }
                });
                self.set_status(format!(
                    "{} '{member}' {} KB '{kb_id}'...",
                    if add { "Adding" } else { "Removing" },
                    if add { "to" } else { "from" }
                ));
                Some(true)
            }
            "kb-approve" => {
                // :kb-approve <kb-id> <fingerprint> [role]
                let line = self.vi.command_line.trim().to_string();
                let mut parts = line.split_whitespace();
                let kb_id = parts.next().unwrap_or("").to_string();
                let principal = parts.next().unwrap_or("").to_string();
                let role = parts.next().unwrap_or("editor").to_string();
                if principal.is_empty() {
                    // No fingerprint to type by hand → open the *KB Sharing* buffer,
                    // where pending requests are approved at-point (a = approve).
                    self.open_kb_sharing();
                    self.set_status(
                        "Pick a pending request in *KB Sharing* (a = approve, d = deny)"
                            .to_string(),
                    );
                    return Some(true);
                }
                self.set_status(format!("Approving '{principal}' for KB '{kb_id}'..."));
                self.collab.pending_intent = Some(CollabIntent::KbApprove {
                    kb_id,
                    principal,
                    role,
                });
                Some(true)
            }
            "kb-pending" => {
                // :kb-pending <kb-id>
                let kb_id = self.vi.command_line.trim().to_string();
                if kb_id.is_empty() {
                    self.set_status("usage: :kb-pending <kb-id>".to_string());
                    return Some(true);
                }
                self.set_status(format!("Listing pending requests for KB '{kb_id}'..."));
                self.collab.pending_intent = Some(CollabIntent::KbListPending { kb_id });
                Some(true)
            }
            "kb-policy" => {
                // :kb-policy <kb-id> <restrictive|invite|permissive>
                let line = self.vi.command_line.trim().to_string();
                let mut parts = line.split_whitespace();
                let kb_id = parts.next().unwrap_or("").to_string();
                let policy = parts.next().unwrap_or("").to_string();
                if kb_id.is_empty()
                    || !matches!(policy.as_str(), "restrictive" | "invite" | "permissive")
                {
                    self.set_status(
                        "usage: :kb-policy <kb-id> <restrictive|invite|permissive>".to_string(),
                    );
                    return Some(true);
                }
                self.set_status(format!("Setting KB '{kb_id}' policy to {policy}..."));
                self.collab.pending_intent = Some(CollabIntent::KbSetPolicy { kb_id, policy });
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
    fn dispatch_kb_member_add_parses_args() {
        let mut editor = Editor::new();
        // Args arrive via command_line (as the ex-command parser sets them).
        editor.vi.command_line = "my-kb SHA256:alice viewer".to_string();
        assert_eq!(editor.dispatch_collab("kb-member-add"), Some(true));
        match editor.collab.pending_intent {
            Some(CollabIntent::KbAddMember {
                ref kb_id,
                ref member,
                ref role,
            }) => {
                assert_eq!(kb_id, "my-kb");
                assert_eq!(member, "SHA256:alice");
                assert_eq!(role, "viewer");
            }
            other => panic!("expected KbAddMember, got: {other:?}"),
        }
    }

    #[test]
    fn dispatch_kb_member_remove_parses_args() {
        let mut editor = Editor::new();
        editor.vi.command_line = "my-kb bob".to_string();
        assert_eq!(editor.dispatch_collab("kb-member-remove"), Some(true));
        assert!(matches!(
            editor.collab.pending_intent,
            Some(CollabIntent::KbRemoveMember { .. })
        ));
    }

    #[test]
    fn dispatch_kb_member_add_missing_args_no_intent() {
        let mut editor = Editor::new();
        editor.vi.command_line = "only-kb-id".to_string();
        assert_eq!(editor.dispatch_collab("kb-member-add"), Some(true));
        assert!(
            editor.collab.pending_intent.is_none(),
            "incomplete args must not queue an intent"
        );
    }

    #[test]
    fn dispatch_kb_approve_parses_args() {
        let mut editor = Editor::new();
        editor.vi.command_line = "my-kb SHA256:bob editor".to_string();
        assert_eq!(editor.dispatch_collab("kb-approve"), Some(true));
        match editor.collab.pending_intent {
            Some(CollabIntent::KbApprove {
                ref kb_id,
                ref principal,
                ref role,
            }) => {
                assert_eq!(kb_id, "my-kb");
                assert_eq!(principal, "SHA256:bob");
                assert_eq!(role, "editor");
            }
            other => panic!("expected KbApprove, got: {other:?}"),
        }
    }

    #[test]
    fn dispatch_kb_pending_sets_intent() {
        let mut editor = Editor::new();
        editor.vi.command_line = "my-kb".to_string();
        assert_eq!(editor.dispatch_collab("kb-pending"), Some(true));
        assert!(matches!(
            editor.collab.pending_intent,
            Some(CollabIntent::KbListPending { .. })
        ));
    }

    #[test]
    fn dispatch_kb_policy_parses_and_rejects_bad_value() {
        let mut editor = Editor::new();
        editor.vi.command_line = "my-kb permissive".to_string();
        assert_eq!(editor.dispatch_collab("kb-policy"), Some(true));
        assert!(matches!(
            editor.collab.pending_intent,
            Some(CollabIntent::KbSetPolicy { ref policy, .. }) if policy == "permissive"
        ));
        // bad policy value → no intent queued.
        let mut e2 = Editor::new();
        e2.vi.command_line = "my-kb bogus".to_string();
        assert_eq!(e2.dispatch_collab("kb-policy"), Some(true));
        assert!(e2.collab.pending_intent.is_none());
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

    /// C2 (collab test-gap plan): `:collab-connect` must use the server address
    /// set by `(set-option!)` in the SAME breath — no tick / apply-drain wait, no
    /// manual `(get-option)` poll. `set_option` writes `collab.server_address`
    /// synchronously and the connect dispatch reads it live, so the address the
    /// connect intent carries is always the latest value. Guards against a future
    /// change that snapshots/caches the address at task-setup time instead.
    #[test]
    fn collab_connect_reads_server_address_live_no_drain() {
        let mut editor = Editor::new();
        editor
            .set_option("collab_server_address", "10.0.0.9:9999")
            .unwrap();
        // Dispatch immediately — no event-loop tick / option drain in between.
        assert_eq!(editor.dispatch_collab("collab-connect"), Some(true));
        match editor.collab.pending_intent {
            Some(CollabIntent::Connect { ref address }) => {
                assert_eq!(
                    address, "10.0.0.9:9999",
                    "connect must use the just-set address, not a stale snapshot"
                );
            }
            ref other => panic!("expected Connect intent, got: {other:?}"),
        }

        // A second change is likewise reflected with no wait.
        editor
            .set_option("collab-server-address", "host.example:1234")
            .unwrap();
        editor.dispatch_collab("collab-connect");
        assert!(matches!(
            editor.collab.pending_intent,
            Some(CollabIntent::Connect { ref address }) if address == "host.example:1234"
        ));
    }
}
