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
            "collab-rotate-identity" => {
                // ADR-040 PR2b: rotate this peer's collab identity key across every KB it owns.
                self.collab.pending_intent = Some(CollabIntent::RotateIdentity);
                self.set_status(
                    "Rotating collab identity — authorize the new key on the daemon, then reconnect",
                );
                self.mark_full_redraw();
                Some(true)
            }
            "collab-register-recovery-key" => {
                // ADR-040 §Recovery-key: register an offline recovery key across my KBs.
                self.collab.pending_intent = Some(CollabIntent::RegisterRecoveryKey);
                self.set_status(
                    "Registering recovery key — back up the saved recovery key OFFLINE",
                );
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
                    .unwrap_or_else(|| crate::editor::KB_DEFAULT_NAME.to_string());
                self.collab.pending_intent = Some(CollabIntent::ShareKb {
                    // A NAME, correctly: `kb_intent_to_command` resolves it and
                    // mints/reuses the KB's collab id (ADR-105 D4).
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
                // ADR-105 D4/H4: `share_p2p` sends this straight to the daemon as
                // the KB's id, so it must BE the collab id — not the display name
                // `active_instance_name()` returns. Passing the name would mesh-share
                // the same KB under a second, different id from its hub share.
                let kb_name = self
                    .kb
                    .active_instance_name()
                    .unwrap_or_else(|| crate::editor::KB_DEFAULT_NAME.to_string());
                let Some(target) = self.kb.registry.target_of_name(&kb_name) else {
                    self.set_status(format!("kb-share-p2p: KB '{kb_name}' not found"));
                    self.mark_full_redraw();
                    return Some(true);
                };
                let Some(kb_id) = self.kb_collab_id_for_share(&target) else {
                    self.set_status(format!(
                        "kb-share-p2p: could not establish a collab id for '{kb_name}'"
                    ));
                    self.mark_full_redraw();
                    return Some(true);
                };
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
            "kb-join-p2p" => {
                // P2P join from a "magnet link" ticket: `:kb-join-p2p <mae://join/…>`
                // (the ticket arrives in command_line). SYNCHRONOUS daemon
                // control-socket call — same backend as the CLI / Scheme / MCP
                // (ADR-025 §"Driving surfaces"). The background dialer then connects
                // + pulls the KB once the owner approves.
                let ticket = self.vi.command_line.trim().to_string();
                if ticket.is_empty() {
                    self.set_status("usage: :kb-join-p2p <mae://join/…ticket>".to_string());
                } else {
                    match self.kb.join_p2p(&ticket) {
                        Ok(msg) => {
                            self.notify(
                                crate::notifications::Notification::success(
                                    "collab",
                                    "P2P join queued",
                                )
                                .body(msg)
                                .key("p2p-join"),
                            );
                            self.set_status("P2P join queued → *Messages*".to_string());
                        }
                        Err(e) => self.set_status(format!("kb-join-p2p: {e}")),
                    }
                }
                self.mark_full_redraw();
                Some(true)
            }
            "kb-join" => {
                // Join a KB — SPC-key dispatch uses the active KB's own collab id.
                // :kb-join <id> is handled in command.rs before reaching here.
                //
                // ADR-105 D4/H4: `JoinKb.kb_id` goes on the wire as a KB id, so it
                // must be one. `active_instance_name()` returns a display NAME, which
                // only doubled as an id while every KB synced under its name. The
                // no-arg form can therefore only re-join a KB this editor already
                // knows an id for; joining a stranger's KB needs their id, which is
                // what `:kb-join <id>` is for.
                let kb_name = self
                    .kb
                    .active_instance_name()
                    .unwrap_or_else(|| crate::editor::KB_DEFAULT_NAME.to_string());
                let Some(kb_id) = self
                    .kb
                    .registry
                    .target_of_name(&kb_name)
                    .and_then(|t| self.kb.registry.collab_id_of_target(&t))
                else {
                    self.set_status(format!(
                        "kb-join: '{kb_name}' has no collab id — use :kb-join <id>                          with the id the owner shared"
                    ));
                    self.mark_full_redraw();
                    return Some(true);
                };
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
                // ADR-105 D4/H4: leaving addresses the KB by its collab id, same as
                // joining. A name reaches the daemon as an id it does not know, and
                // the leave silently applies to nothing.
                let kb_name = self
                    .kb
                    .active_instance_name()
                    .unwrap_or_else(|| crate::editor::KB_DEFAULT_NAME.to_string());
                let Some(kb_id) = self
                    .kb
                    .registry
                    .target_of_name(&kb_name)
                    .and_then(|t| self.kb.registry.collab_id_of_target(&t))
                else {
                    self.set_status(format!("kb-leave: '{kb_name}' is not a shared KB"));
                    self.mark_full_redraw();
                    return Some(true);
                };
                self.collab.pending_intent = Some(CollabIntent::LeaveKb {
                    kb_id: kb_id.clone(),
                });
                self.set_status(format!("Leaving KB '{}'...", kb_id));
                self.mark_full_redraw();
                Some(true)
            }
            "kb-set-encryption" => {
                // :kb-set-encryption <kb> [mode]  (args via command_line, mode
                // defaults to "e2e" — the only supported mode; encryption is
                // one-way). Was previously registered in `CommandRegistry` but
                // had NO dispatch arm anywhere in mae-core — reachable only via
                // the `kb_set_encryption` MCP tool (`execute_kb_set_encryption`,
                // `crates/ai/src/executor/collab_exec.rs`) directly setting this
                // same `CollabIntent`, never through `dispatch_builtin` by name.
                // That meant `:kb-set-encryption <kb> e2e` was a dead ex-command
                // for a human too (not just an AI-parity gap) — the #521-era
                // permission-enforcement audit's "registered but unreachable"
                // defect class, closed here for real (matching behavior, not a
                // stub), not just papered over with a recognized-but-inert arm.
                let line = self.vi.command_line.trim().to_string();
                let mut parts = line.split_whitespace();
                let kb_id = parts.next().unwrap_or("").to_string();
                let mode = parts.next().unwrap_or("e2e").to_string();
                if kb_id.is_empty() {
                    self.set_status(
                        "Usage: :kb-set-encryption <kb> [mode]  (only 'e2e' is supported)",
                    );
                    return Some(true);
                }
                if mode != "e2e" {
                    self.set_status(format!(
                        "Invalid mode '{mode}' (only 'e2e' is supported; encryption is one-way)"
                    ));
                    return Some(true);
                }
                self.collab.pending_intent = Some(CollabIntent::KbSetEncryption {
                    kb_id: kb_id.clone(),
                    mode,
                });
                self.set_status(format!("Enabling E2E encryption on KB '{kb_id}'..."));
                Some(true)
            }
            // Accept both the editor's historical `kb-member-*` spelling AND the
            // canonical `kb-add-member`/`kb-remove-member` names used by the docs,
            // the Scheme prims, and the MCP tools (three-surface parity, #3).
            "kb-member-add" | "kb-member-remove" | "kb-add-member" | "kb-remove-member" => {
                // :kb-add-member <kb-id> <fingerprint> [role]  (args via command_line).
                let line = self.vi.command_line.trim().to_string();
                let mut parts = line.split_whitespace();
                // ADR-105 D4: the user types a NAME or an id; the wire needs the id.
                let kb_id = self.kb_collab_id_arg(parts.next().unwrap_or(""));
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
                let add = matches!(name, "kb-member-add" | "kb-add-member");
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
            "kb-member-block" | "kb-member-unblock" | "kb-block-member" | "kb-unblock-member" => {
                // :kb-block-member <kb-id> <fingerprint> — ADR-039 A2 (#162) local
                // self-protection deny-list. Local-only to the daemon; not owner-gated.
                let line = self.vi.command_line.trim().to_string();
                let mut parts = line.split_whitespace();
                let kb_id = parts.next().unwrap_or("").to_string();
                let member = parts.next().unwrap_or("").to_string();
                if member.is_empty() {
                    self.open_kb_sharing();
                    self.set_status(
                        "Pick a member in *KB Sharing* (b = block, e/v/o = role, x = remove)"
                            .to_string(),
                    );
                    return Some(true);
                }
                let blocked = matches!(name, "kb-member-block" | "kb-block-member");
                self.collab.pending_intent = Some(CollabIntent::KbSetBlock {
                    kb_id: kb_id.clone(),
                    member: member.clone(),
                    blocked,
                });
                self.set_status(format!(
                    "{} '{member}' {} KB '{kb_id}' (local self-protection)...",
                    if blocked { "Blocking" } else { "Unblocking" },
                    if blocked { "on" } else { "from" }
                ));
                Some(true)
            }
            "kb-approve" => {
                // :kb-approve <kb-id> <fingerprint> [role]
                let line = self.vi.command_line.trim().to_string();
                let mut parts = line.split_whitespace();
                // ADR-105 D4: resolve a typed name to the KB's collab id.
                let kb_id = self.kb_collab_id_arg(parts.next().unwrap_or(""));
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
                let kb_id = self.kb_collab_id_arg(self.vi.command_line.trim());
                if kb_id.is_empty() {
                    self.set_status("usage: :kb-pending <kb-id>".to_string());
                    return Some(true);
                }
                self.set_status(format!("Listing pending requests for KB '{kb_id}'..."));
                self.collab.pending_intent = Some(CollabIntent::KbListPending { kb_id });
                Some(true)
            }
            "kb-policy" | "kb-set-policy" => {
                // :kb-set-policy <kb-id> <restrictive|invite|permissive>
                let line = self.vi.command_line.trim().to_string();
                let mut parts = line.split_whitespace();
                // ADR-105 D4: resolve a typed name to the KB's collab id.
                let kb_id = self.kb_collab_id_arg(parts.next().unwrap_or(""));
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

    /// Three-surface parity (#3): the canonical `kb-add-member` / `kb-remove-member`
    /// / `kb-block-member` / `kb-unblock-member` / `kb-set-policy` names — the ones
    /// the docs, Scheme prims, and MCP tools use — must route through dispatch too,
    /// not just the historical `kb-member-*` / `kb-policy` spellings. This is the
    /// exact gap the verifiable-docs guard caught: a user following the manual and
    /// typing `:kb-add-member` must not hit "unknown command".
    #[test]
    fn dispatch_accepts_canonical_member_and_policy_names() {
        let mut editor = Editor::new();

        editor.vi.command_line = "my-kb SHA256:alice editor".to_string();
        assert_eq!(editor.dispatch_collab("kb-add-member"), Some(true));
        assert!(matches!(
            editor.collab.pending_intent,
            Some(CollabIntent::KbAddMember { .. })
        ));

        editor.collab.pending_intent = None;
        editor.vi.command_line = "my-kb SHA256:bob".to_string();
        assert_eq!(editor.dispatch_collab("kb-remove-member"), Some(true));
        assert!(matches!(
            editor.collab.pending_intent,
            Some(CollabIntent::KbRemoveMember { .. })
        ));

        editor.collab.pending_intent = None;
        editor.vi.command_line = "my-kb invite".to_string();
        assert_eq!(editor.dispatch_collab("kb-set-policy"), Some(true));
        assert!(matches!(
            editor.collab.pending_intent,
            Some(CollabIntent::KbSetPolicy { .. })
        ));

        // The legacy spellings still resolve (back-compat).
        editor.collab.pending_intent = None;
        editor.vi.command_line = "my-kb SHA256:carol".to_string();
        assert_eq!(editor.dispatch_collab("kb-member-add"), Some(true));
        assert!(matches!(
            editor.collab.pending_intent,
            Some(CollabIntent::KbAddMember { .. })
        ));
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
    fn dispatch_kb_member_block_unblock_parse_args() {
        // block → KbSetBlock { blocked: true }
        let mut editor = Editor::new();
        editor.vi.command_line = "my-kb bob".to_string();
        assert_eq!(editor.dispatch_collab("kb-member-block"), Some(true));
        assert!(matches!(
            editor.collab.pending_intent,
            Some(CollabIntent::KbSetBlock { blocked: true, .. })
        ));
        // unblock → KbSetBlock { blocked: false }
        let mut editor = Editor::new();
        editor.vi.command_line = "my-kb bob".to_string();
        assert_eq!(editor.dispatch_collab("kb-member-unblock"), Some(true));
        assert!(matches!(
            editor.collab.pending_intent,
            Some(CollabIntent::KbSetBlock { blocked: false, .. })
        ));
        // missing fingerprint → no intent queued (opens the picker instead).
        let mut editor = Editor::new();
        editor.vi.command_line = "only-kb-id".to_string();
        assert_eq!(editor.dispatch_collab("kb-member-block"), Some(true));
        assert!(editor.collab.pending_intent.is_none());
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
