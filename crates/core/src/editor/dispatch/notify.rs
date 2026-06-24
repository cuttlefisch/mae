//! `*Notifications*` attention-buffer dispatch (ADR-024) — open + at-point
//! actions (run-action / dismiss / fold), mirroring the git-status dispatch.

use super::super::Editor;
use crate::notifications_view::NotifView;

impl Editor {
    /// Dispatch notification commands. Returns `Some(true)` if handled.
    pub(super) fn dispatch_notifications(&mut self, name: &str) -> Option<bool> {
        // Buffer-context commands operate on the line under the cursor.
        let requires_buffer = matches!(
            name,
            "notify-run-action" | "notify-dismiss" | "notify-toggle-fold"
        );
        if requires_buffer {
            let idx = self.active_buffer_idx();
            if self.buffers[idx].kind != crate::buffer::BufferKind::Notifications {
                self.set_status("Requires the *Notifications* buffer");
                return Some(true);
            }
        }

        match name {
            "notifications-open" => {
                self.notifications_open();
                Some(true)
            }
            "notify-run-action" => {
                self.notify_run_action_at_cursor();
                Some(true)
            }
            "notify-dismiss" => {
                self.notify_dismiss_at_cursor();
                Some(true)
            }
            "notify-toggle-fold" => {
                self.notify_toggle_fold_at_cursor();
                Some(true)
            }
            _ => None,
        }
    }

    /// The `(notif_id, action_idx?)` at the cursor in the `*Notifications*` buffer.
    fn notif_cursor_target(&self) -> Option<(u64, Option<usize>)> {
        let win = self.window_mgr.focused_window();
        let idx = self.active_buffer_idx();
        let view = self.buffers[idx].notif_view()?;
        let line = view.line_at(win.cursor_row)?;
        line.notif_id().map(|id| (id, line.action_idx()))
    }

    fn notify_run_action_at_cursor(&mut self) {
        let Some((id, action_idx)) = self.notif_cursor_target() else {
            self.set_status("No notification under cursor");
            return;
        };
        // On an action row run that action; on an item row run its first action.
        let idx = action_idx.unwrap_or(0);
        if self.notifications.action(id, idx).is_none() {
            self.set_status("No action on this notification");
            return;
        }
        self.notify_run_action(id, idx);
    }

    fn notify_dismiss_at_cursor(&mut self) {
        let Some((id, _)) = self.notif_cursor_target() else {
            self.set_status("No notification under cursor");
            return;
        };
        self.dismiss_notification(id);
    }

    fn notify_toggle_fold_at_cursor(&mut self) {
        let win = self.window_mgr.focused_window();
        let idx = self.active_buffer_idx();
        let key = self.buffers[idx]
            .notif_view()
            .and_then(|v| v.line_at(win.cursor_row))
            .and_then(NotifView::collapse_key_for_line);
        if let Some(key) = key {
            if let Some(view) = self.buffers[idx].notif_view_mut() {
                view.toggle(key);
            }
            // Rebuild text from the (now-toggled) view's fold state.
            self.refresh_notifications_buffer();
        }
    }
}
