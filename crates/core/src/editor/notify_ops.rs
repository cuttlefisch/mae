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

    /// Open (or refresh + focus) the magit-style `*Notifications*` buffer.
    pub fn notifications_open(&mut self) {
        let (view, text) = self.build_notif_view();
        let buf_name = "*notifications*";
        let idx = if let Some(i) = self.find_buffer_by_name(buf_name) {
            self.buffers[i] = crate::buffer::Buffer::new();
            self.buffers[i].name = buf_name.to_string();
            self.buffers[i].kind = crate::buffer::BufferKind::Notifications;
            i
        } else {
            let mut buf = crate::buffer::Buffer::new();
            buf.name = buf_name.to_string();
            buf.kind = crate::buffer::BufferKind::Notifications;
            self.buffers.push(buf);
            self.buffers.len() - 1
        };
        self.buffers[idx].view = crate::buffer_view::BufferView::Notifications(Box::new(view));
        self.buffers[idx].insert_text_at(0, &text);
        self.buffers[idx].read_only = true;
        self.buffers[idx].modified = false;
        let prev = self.active_buffer_idx();
        self.vi.alternate_buffer_idx = Some(prev);
        self.display_buffer(idx);
        self.set_mode(crate::Mode::Normal);
    }

    /// Rebuild the `*Notifications*` buffer in place if it is currently open, so
    /// the list stays in sync as notifications are raised/resolved. Preserves the
    /// per-category fold state and clamps any showing window's cursor.
    pub(crate) fn refresh_notifications_buffer(&mut self) {
        let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.kind == crate::buffer::BufferKind::Notifications)
        else {
            return;
        };
        let (view, text) = self.build_notif_view();
        self.buffers[idx].read_only = false;
        let end = self.buffers[idx].rope().len_chars();
        self.buffers[idx].delete_range(0, end);
        self.buffers[idx].insert_text_at(0, &text);
        self.buffers[idx].read_only = true;
        self.buffers[idx].modified = false;
        self.buffers[idx].view = crate::buffer_view::BufferView::Notifications(Box::new(view));
        let last = self.buffers[idx].display_line_count().saturating_sub(1);
        for win in self.window_mgr.iter_windows_mut() {
            if win.buffer_idx == idx && win.cursor_row > last {
                win.cursor_row = last;
            }
        }
        self.mark_full_redraw();
    }

    /// Build the `*Notifications*` view model + rope text from the
    /// NotificationCenter, grouped by category (source) with at-point action rows.
    /// Preserves the fold state of any currently-open buffer.
    fn build_notif_view(&self) -> (crate::notifications_view::NotifView, String) {
        use crate::notifications::Severity;
        use crate::notifications_view::{CollapseKey, NotifLine, NotifLineKind, NotifView};

        let prev_collapsed = self
            .buffers
            .iter()
            .find(|b| b.kind == crate::buffer::BufferKind::Notifications)
            .and_then(|b| b.notif_view())
            .map(|v| v.collapsed.clone())
            .unwrap_or_default();

        let glyph = |s: Severity| match s {
            Severity::ActionRequired => '\u{2691}', // ⚑
            Severity::Error => '\u{2716}',          // ✖
            Severity::Warning => '\u{26A0}',        // ⚠
            Severity::Success => '\u{2714}',        // ✔
            Severity::Info => '\u{2139}',           // ℹ
        };

        let mut view = NotifView::new();
        view.collapsed = prev_collapsed;
        let mut text = String::new();
        let push = |view: &mut NotifView, text: &mut String, line: NotifLine| {
            text.push_str(&line.text);
            text.push('\n');
            view.lines.push(line);
        };

        let items = self.notifications.active_sorted();
        let outstanding = self.notifications.outstanding_count();
        push(
            &mut view,
            &mut text,
            NotifLine {
                text: format!("Notifications — {outstanding} outstanding"),
                kind: NotifLineKind::Header,
                category: None,
            },
        );
        push(&mut view, &mut text, NotifLine::blank());

        if items.is_empty() {
            push(
                &mut view,
                &mut text,
                NotifLine {
                    text: "  (nothing demands your attention)".to_string(),
                    kind: NotifLineKind::Blank,
                    category: None,
                },
            );
            return (view, text);
        }

        // Group by category (source), preserving active_sorted order.
        let mut categories: Vec<&'static str> = Vec::new();
        for n in &items {
            if !categories.contains(&n.source) {
                categories.push(n.source);
            }
        }

        for cat in categories {
            let collapsed = view.is_collapsed(&CollapseKey::Category(cat.to_string()));
            let marker = if collapsed { '\u{25B8}' } else { '\u{25BE}' }; // ▸ / ▾
            push(
                &mut view,
                &mut text,
                NotifLine {
                    text: format!("{marker} {cat}"),
                    kind: NotifLineKind::CategoryHeader(cat.to_string()),
                    category: Some(cat.to_string()),
                },
            );
            if collapsed {
                continue;
            }
            for n in items.iter().filter(|n| n.source == cat) {
                if n.resolved.is_some() {
                    push(
                        &mut view,
                        &mut text,
                        NotifLine {
                            text: format!("    \u{2713} {} (resolved)", n.title),
                            kind: NotifLineKind::ResolvedItem { notif_id: n.id },
                            category: Some(cat.to_string()),
                        },
                    );
                    continue;
                }
                push(
                    &mut view,
                    &mut text,
                    NotifLine {
                        text: format!("  {} {}", glyph(n.severity), n.title),
                        kind: NotifLineKind::Item { notif_id: n.id },
                        category: Some(cat.to_string()),
                    },
                );
                if let Some(body) = &n.body {
                    push(
                        &mut view,
                        &mut text,
                        NotifLine {
                            text: format!("      {body}"),
                            kind: NotifLineKind::Blank,
                            category: Some(cat.to_string()),
                        },
                    );
                }
                for (i, action) in n.actions.iter().enumerate() {
                    push(
                        &mut view,
                        &mut text,
                        NotifLine {
                            text: format!("      \u{2192} {}", action.label), // →
                            kind: NotifLineKind::ActionRow {
                                notif_id: n.id,
                                action_idx: i,
                            },
                            category: Some(cat.to_string()),
                        },
                    );
                }
            }
            push(&mut view, &mut text, NotifLine::blank());
        }
        (view, text)
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

    #[test]
    fn notifications_buffer_lists_items_and_actions() {
        use crate::notifications::NotifCommand;
        let mut ed = Editor::new();
        ed.notify(
            Notification::action_required("collab", "edit fenced")
                .key("collab:fence:kb:n1")
                .body("authored before your access changed")
                .action(
                    "Accept-remote",
                    NotifCommand::AdoptRemote {
                        kb_id: "kb".into(),
                        node_id: "n1".into(),
                    },
                ),
        );
        ed.notifications_open();
        let idx = ed
            .find_buffer_by_name("*notifications*")
            .expect("*notifications* buffer created");
        assert_eq!(
            ed.buffers[idx].kind,
            crate::buffer::BufferKind::Notifications
        );
        let text: String = ed.buffers[idx].rope().chars().collect();
        assert!(text.contains("edit fenced"), "item title shown: {text:?}");
        assert!(text.contains("Accept-remote"), "action row shown: {text:?}");
        assert!(text.contains("collab"), "category header shown: {text:?}");
        // The view model carries an Item + an ActionRow row.
        let view = ed.buffers[idx].notif_view().expect("notif view");
        assert!(view.lines.iter().any(|l| matches!(
            l.kind,
            crate::notifications_view::NotifLineKind::ActionRow { .. }
        )));
    }
}
