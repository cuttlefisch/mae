//! `*KB Sharing*` buffer at-point dispatch.
//!
//! Mirrors `dispatch/notify.rs` / `dispatch/git.rs`: cursor-row → semantic line →
//! action. All actions route through existing `CollabIntent`s (the same ones the
//! commands + MCP tools + Scheme primitives use), so there is no logic
//! duplication — the buffer is just a discoverable front-end (CLAUDE.md #3, #8).

use super::super::Editor;
use crate::kb_sharing::{CollapseKey, KbSharingLineKind, KbSharingView};
use crate::CollabIntent;

impl Editor {
    /// Dispatch `*KB Sharing*` buffer commands. Returns `Some(true)` if handled.
    pub(super) fn dispatch_kb_sharing(&mut self, name: &str) -> Option<bool> {
        if !name.starts_with("kb-sharing-") {
            return None;
        }
        // Every at-point action requires the *KB Sharing* buffer to be focused.
        let idx = self.active_buffer_idx();
        if self.buffers[idx].kind != crate::buffer::BufferKind::KbSharing {
            self.set_status("Requires the *KB Sharing* buffer");
            return Some(true);
        }

        match name {
            "kb-sharing-refresh" => {
                self.refresh_kb_sharing_buffer();
                Some(true)
            }
            "kb-sharing-toggle-fold" => {
                self.kb_sharing_toggle_fold_at_cursor();
                Some(true)
            }
            "kb-sharing-approve" => self.kb_sharing_pending_action(name),
            "kb-sharing-deny" => self.kb_sharing_pending_action(name),
            "kb-sharing-role-editor"
            | "kb-sharing-role-viewer"
            | "kb-sharing-role-owner"
            | "kb-sharing-remove"
            | "kb-sharing-copy-fingerprint" => self.kb_sharing_member_action(name),
            "kb-sharing-set-policy" => self.kb_sharing_set_policy_at_cursor(),
            "kb-sharing-leave" => self.kb_sharing_leave_at_cursor(),
            _ => {
                self.set_status(format!("Unknown KB-sharing action: {name}"));
                Some(true)
            }
        }
    }

    /// The `(kb_id, fingerprint, kind)` at the focused cursor in the buffer.
    fn kb_sharing_target(&self) -> Option<(String, Option<String>, KbSharingLineKind)> {
        let win = self.window_mgr.focused_window();
        let idx = self.active_buffer_idx();
        let view: &KbSharingView = self.buffers[idx].kb_sharing_view()?;
        let line = view.line_at(win.cursor_row)?;
        Some((
            line.kb_id()?.to_string(),
            line.fingerprint().map(|s| s.to_string()),
            line.kind.clone(),
        ))
    }

    /// Guard: only the owner of `kb_id` may run membership/policy actions.
    fn kb_sharing_is_owner(&self, kb_id: &str) -> bool {
        let idx = self.active_buffer_idx();
        self.buffers[idx]
            .kb_sharing_view()
            .and_then(|v| v.entry_for(kb_id))
            .map(|e| e.is_owner)
            .unwrap_or(false)
    }

    fn kb_sharing_pending_action(&mut self, name: &str) -> Option<bool> {
        let Some((kb_id, Some(fp), KbSharingLineKind::Pending { .. })) = self.kb_sharing_target()
        else {
            self.set_status("Move the cursor onto a pending request first");
            return Some(true);
        };
        if !self.kb_sharing_is_owner(&kb_id) {
            self.set_status("Only the KB owner can approve/deny requests");
            return Some(true);
        }
        match name {
            "kb-sharing-approve" => {
                self.collab.pending_intent = Some(CollabIntent::KbApprove {
                    kb_id: kb_id.clone(),
                    principal: fp,
                    role: "editor".to_string(),
                });
                self.set_status(format!(
                    "Approving join request for KB '{kb_id}' as editor…"
                ));
            }
            "kb-sharing-deny" => {
                self.collab.pending_intent = Some(CollabIntent::KbRemoveMember {
                    kb_id: kb_id.clone(),
                    member: fp,
                });
                self.set_status(format!("Denying join request for KB '{kb_id}'…"));
            }
            _ => {}
        }
        self.mark_full_redraw();
        Some(true)
    }

    fn kb_sharing_member_action(&mut self, name: &str) -> Option<bool> {
        let Some((kb_id, Some(fp), KbSharingLineKind::Member { .. })) = self.kb_sharing_target()
        else {
            self.set_status("Move the cursor onto a member first");
            return Some(true);
        };

        // Copy is allowed for anyone; the rest are owner-only.
        if name == "kb-sharing-copy-fingerprint" {
            match crate::clipboard::copy(&fp) {
                Ok(()) => self.set_status(format!("Copied fingerprint: {fp}")),
                Err(_) => self.set_status(format!("Fingerprint: {fp}")),
            }
            return Some(true);
        }
        if !self.kb_sharing_is_owner(&kb_id) {
            self.set_status("Only the KB owner can change roles or remove members");
            return Some(true);
        }
        match name {
            "kb-sharing-role-editor" | "kb-sharing-role-viewer" | "kb-sharing-role-owner" => {
                let role = name.trim_start_matches("kb-sharing-role-").to_string();
                self.collab.pending_intent = Some(CollabIntent::KbAddMember {
                    kb_id: kb_id.clone(),
                    member: fp,
                    role: role.clone(),
                });
                self.set_status(format!("Setting member role to {role} in KB '{kb_id}'…"));
            }
            "kb-sharing-remove" => {
                self.collab.pending_intent = Some(CollabIntent::KbRemoveMember {
                    kb_id: kb_id.clone(),
                    member: fp,
                });
                self.set_status(format!("Removing member from KB '{kb_id}'…"));
            }
            _ => {}
        }
        self.mark_full_redraw();
        Some(true)
    }

    fn kb_sharing_set_policy_at_cursor(&mut self) -> Option<bool> {
        let Some((kb_id, _, _)) = self.kb_sharing_target() else {
            self.set_status("Move the cursor onto a KB first");
            return Some(true);
        };
        if !self.kb_sharing_is_owner(&kb_id) {
            self.set_status("Only the KB owner can set the join policy");
            return Some(true);
        }
        // Cycle restrictive → invite → permissive → restrictive.
        let idx = self.active_buffer_idx();
        let current = self.buffers[idx]
            .kb_sharing_view()
            .and_then(|v| v.entry_for(&kb_id))
            .map(|e| e.policy.clone())
            .unwrap_or_else(|| "invite".to_string());
        let next = match current.as_str() {
            "restrictive" => "invite",
            "invite" => "permissive",
            _ => "restrictive",
        };
        self.collab.pending_intent = Some(CollabIntent::KbSetPolicy {
            kb_id: kb_id.clone(),
            policy: next.to_string(),
        });
        self.set_status(format!("KB '{kb_id}' join policy → {next}"));
        self.mark_full_redraw();
        Some(true)
    }

    fn kb_sharing_leave_at_cursor(&mut self) -> Option<bool> {
        let Some((kb_id, _, _)) = self.kb_sharing_target() else {
            self.set_status("Move the cursor onto a KB first");
            return Some(true);
        };
        self.collab.pending_intent = Some(CollabIntent::LeaveKb {
            kb_id: kb_id.clone(),
        });
        self.set_status(format!("Leaving KB '{kb_id}' (local copy preserved)…"));
        self.mark_full_redraw();
        Some(true)
    }

    fn kb_sharing_toggle_fold_at_cursor(&mut self) {
        let win = self.window_mgr.focused_window();
        let idx = self.active_buffer_idx();
        let key: Option<CollapseKey> = self.buffers[idx]
            .kb_sharing_view()
            .and_then(|v| v.line_at(win.cursor_row))
            .and_then(KbSharingView::collapse_key_for_line);
        if let Some(key) = key {
            if let Some(view) = self.buffers[idx].kb_sharing_view_mut() {
                view.toggle(key);
            }
            self.refresh_kb_sharing_buffer();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Open a `*KB Sharing*` buffer for an owner KB with a member + a pending
    /// request, and place the cursor on the line matching `predicate`.
    fn editor_on_line<F>(predicate: F) -> Editor
    where
        F: Fn(&KbSharingLineKind) -> bool,
    {
        use mae_sync::kb::{KbCollectionDoc, Role};
        let mut editor = Editor::new();
        editor.collab.local_fingerprint = "alicefp".to_string();
        let mut coll = KbCollectionDoc::new_owned("Team", "alicefp", "alice");
        let _ = coll.upsert_member("bobfp", "bob", Role::Editor);
        let _ = coll.add_pending("carolfp", "carol", "2026-06-23");
        editor
            .collab
            .kb_collection_state
            .insert("team".to_string(), coll.encode_state());
        editor.open_kb_sharing();

        let idx = editor.active_buffer_idx();
        let row = editor.buffers[idx]
            .kb_sharing_view()
            .unwrap()
            .lines
            .iter()
            .position(|l| predicate(&l.kind))
            .expect("a matching line");
        editor.window_mgr.focused_window_mut().cursor_row = row;
        editor
    }

    #[test]
    fn approve_on_pending_row_queues_kb_approve() {
        let mut editor = editor_on_line(|k| matches!(k, KbSharingLineKind::Pending { .. }));
        assert_eq!(editor.dispatch_kb_sharing("kb-sharing-approve"), Some(true));
        assert!(matches!(
            &editor.collab.pending_intent,
            Some(CollabIntent::KbApprove { kb_id, principal, role })
                if kb_id == "team" && principal == "carolfp" && role == "editor"
        ));
    }

    #[test]
    fn set_role_on_member_row_queues_kb_add_member() {
        let mut editor = editor_on_line(
            |k| matches!(k, KbSharingLineKind::Member { fingerprint, .. } if fingerprint == "bobfp"),
        );
        assert_eq!(
            editor.dispatch_kb_sharing("kb-sharing-role-viewer"),
            Some(true)
        );
        assert!(matches!(
            &editor.collab.pending_intent,
            Some(CollabIntent::KbAddMember { kb_id, member, role })
                if kb_id == "team" && member == "bobfp" && role == "viewer"
        ));
    }

    #[test]
    fn set_policy_cycles_invite_to_permissive() {
        let mut editor = editor_on_line(|k| matches!(k, KbSharingLineKind::KbHeader { .. }));
        assert_eq!(
            editor.dispatch_kb_sharing("kb-sharing-set-policy"),
            Some(true)
        );
        assert!(matches!(
            &editor.collab.pending_intent,
            Some(CollabIntent::KbSetPolicy { kb_id, policy })
                if kb_id == "team" && policy == "permissive"
        ));
    }

    #[test]
    fn actions_require_the_kb_sharing_buffer() {
        // A scratch (non-KbSharing) buffer rejects the at-point action.
        let mut editor = Editor::new();
        assert_eq!(editor.dispatch_kb_sharing("kb-sharing-approve"), Some(true));
        assert!(editor.collab.pending_intent.is_none());
    }

    #[test]
    fn non_owner_cannot_approve() {
        // Bob (a viewer) viewing the buffer cannot approve — guard rejects it.
        use mae_sync::kb::{KbCollectionDoc, Role};
        let mut editor = Editor::new();
        editor.collab.local_fingerprint = "bobfp".to_string();
        let mut coll = KbCollectionDoc::new_owned("Team", "alicefp", "alice");
        let _ = coll.upsert_member("bobfp", "bob", Role::Viewer);
        let _ = coll.add_pending("carolfp", "carol", "2026-06-23");
        editor
            .collab
            .kb_collection_state
            .insert("team".to_string(), coll.encode_state());
        editor.open_kb_sharing();
        let idx = editor.active_buffer_idx();
        let row = editor.buffers[idx]
            .kb_sharing_view()
            .unwrap()
            .lines
            .iter()
            .position(|l| matches!(&l.kind, KbSharingLineKind::Pending { .. }))
            .unwrap();
        editor.window_mgr.focused_window_mut().cursor_row = row;
        editor.dispatch_kb_sharing("kb-sharing-approve");
        assert!(
            editor.collab.pending_intent.is_none(),
            "a non-owner must not be able to approve"
        );
    }
}
