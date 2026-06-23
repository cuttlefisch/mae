//! Editor-facing API for the NotificationCenter attention bus (ADR-024).
//!
//! The data model + routing live in `crate::notifications`; the visible side
//! effects (status line, badge, modal, `*Notifications*` buffer) need `&mut
//! Editor` and live here. Any subsystem raises a notification via
//! `editor.notify(Notification::action_required("collab", ...).key(...).action(...))`.

use crate::notifications::{NotifCommand, NotificationBuilder, Resolution, Severity, Surface};

impl super::Editor {
    /// Raise a notification: mirror it to `*Messages*` (parity with `set_status`),
    /// route it by severity to a surface, and apply that surface's side effect.
    /// Returns the (possibly deduped) notification id.
    pub fn notify(&mut self, builder: NotificationBuilder) -> u64 {
        let mut builder = builder;
        let reply = builder.reply.take();
        let severity = builder.severity;
        let source = builder.source;
        let title = builder.title.clone();

        // Mirror to the *Messages* log so `read_messages` / the AI see the feed.
        self.message_log
            .push(severity.message_level(), source, title.clone());

        let ingested = self.notifications.ingest(&builder);
        let id = ingested.id;

        match ingested.surface {
            // Immediate toast on the status line. For sticky surfaces this is the
            // one-time toast; the durable signal is the badge (Phase 2) + buffer.
            Surface::Status | Surface::Badge | Surface::Buffer => {
                self.status_msg = title;
            }
            Surface::Modal => {
                self.status_msg = title;
                if let Some(r) = reply {
                    self.pending_notif_reply = Some((id, r));
                }
                // The MiniDialog surface is wired in Phase 4; until then the toast
                // + the parked reply are the behavior.
            }
            Surface::Silent => {}
        }

        if severity == Severity::Error {
            self.ring_bell();
        }
        // Sticky surfaces refresh the *Notifications* buffer if it's open (Phase 3).
        if ingested.surface.is_sticky() {
            self.refresh_notifications_buffer();
        }
        id
    }

    /// Run a notification's at-point action (by index), then auto-resolve it as
    /// `Acted`. Returns whether the action ran.
    pub fn notify_run_action(&mut self, id: u64, idx: usize) -> bool {
        let (cmd, label) = match self.notifications.action(id, idx) {
            Some(a) => (a.command.clone(), a.label.clone()),
            None => return false,
        };
        let ran = match cmd {
            NotifCommand::Command(name) => self.execute_command(&name),
            NotifCommand::AdoptRemote { kb_id, node_id } => {
                self.notify_collab_adopt_remote(&kb_id, &node_id)
            }
            NotifCommand::KeepMine { kb_id, node_id } => {
                self.notify_collab_keep_mine(&kb_id, &node_id)
            }
            NotifCommand::StashExternally { kb_id, node_id } => {
                self.notify_collab_stash_externally(&kb_id, &node_id)
            }
        };
        if ran {
            self.notifications.resolve(id, Resolution::Acted(label));
            self.refresh_notifications_buffer();
        }
        ran
    }

    pub fn resolve_notification(&mut self, id: u64, resolution: Resolution) -> bool {
        let r = self.notifications.resolve(id, resolution);
        if r {
            self.refresh_notifications_buffer();
        }
        r
    }

    pub fn dismiss_notification(&mut self, id: u64) -> bool {
        let r = self.notifications.dismiss(id);
        if r {
            self.refresh_notifications_buffer();
        }
        r
    }

    /// Refresh the `*Notifications*` buffer if it's currently displayed. Wired in
    /// Phase 3 (the magit-style buffer); a no-op until then.
    pub(crate) fn refresh_notifications_buffer(&mut self) {
        // Phase 3 will rebuild the buffer view here. Until the buffer exists, the
        // badge (Phase 2) and the status toast are the surfaces.
    }

    // --- Collab resolution verbs (real implementations land in Phase 5) ---

    pub(crate) fn notify_collab_adopt_remote(&mut self, _kb_id: &str, node_id: &str) -> bool {
        self.set_status(format!("Adopt-remote for {node_id}: not yet implemented"));
        false
    }
    pub(crate) fn notify_collab_keep_mine(&mut self, _kb_id: &str, node_id: &str) -> bool {
        self.set_status(format!(
            "Keep-mine (re-author) for {node_id}: not yet implemented"
        ));
        false
    }
    pub(crate) fn notify_collab_stash_externally(&mut self, _kb_id: &str, node_id: &str) -> bool {
        self.set_status(format!(
            "Stash-externally for {node_id}: not yet implemented"
        ));
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use crate::notifications::Notification;

    #[test]
    fn notify_routes_info_to_status_no_badge() {
        let mut ed = Editor::new();
        ed.notify(Notification::info("test", "hello world"));
        assert_eq!(ed.status_msg, "hello world");
        assert_eq!(ed.notifications.outstanding_count(), 0);
    }

    #[test]
    fn action_required_is_sticky_and_counts() {
        let mut ed = Editor::new();
        let id = ed.notify(
            Notification::action_required("collab", "edit fenced").key("collab:fence:kb:n1"),
        );
        assert_eq!(ed.notifications.outstanding_count(), 1);
        assert_eq!(
            ed.notifications.badge_severity(),
            Some(Severity::ActionRequired)
        );
        // Re-raising the same key dedups (no spam).
        ed.notify(
            Notification::action_required("collab", "edit fenced again").key("collab:fence:kb:n1"),
        );
        assert_eq!(ed.notifications.outstanding_count(), 1);
        // Dismiss clears it.
        assert!(ed.dismiss_notification(id));
        assert_eq!(ed.notifications.outstanding_count(), 0);
    }

    #[test]
    fn unknown_action_index_is_noop() {
        let mut ed = Editor::new();
        let id = ed.notify(Notification::action_required("collab", "x").key("k"));
        assert!(!ed.notify_run_action(id, 9));
    }
}
