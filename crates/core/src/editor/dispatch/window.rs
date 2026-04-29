use crate::window::{Direction, SplitDirection};

use super::super::Editor;

impl Editor {
    /// Dispatch window management commands. Returns `Some(true)` if handled.
    pub(super) fn dispatch_window(&mut self, name: &str) -> Option<bool> {
        match name {
            "split-vertical" => {
                let buf_idx = self.active_buffer_idx();
                let area = self.default_area();
                match self
                    .window_mgr
                    .split(SplitDirection::Vertical, buf_idx, area)
                {
                    Ok(_) => {}
                    Err(e) => self.set_status(e),
                }
            }
            "split-horizontal" => {
                let buf_idx = self.active_buffer_idx();
                let area = self.default_area();
                match self
                    .window_mgr
                    .split(SplitDirection::Horizontal, buf_idx, area)
                {
                    Ok(_) => {}
                    Err(e) => self.set_status(e),
                }
            }
            "close-window" => {
                if self
                    .window_mgr
                    .close(self.window_mgr.focused_id())
                    .is_none()
                {
                    self.set_status("Cannot close last window");
                }
            }
            "focus-left" => self.focus_direction(Direction::Left),
            "focus-right" => self.focus_direction(Direction::Right),
            "focus-up" => self.focus_direction(Direction::Up),
            "focus-down" => self.focus_direction(Direction::Down),
            "window-grow" => {
                self.window_mgr.adjust_ratio(Direction::Right, 0.05);
            }
            "window-shrink" => {
                self.window_mgr.adjust_ratio(Direction::Left, 0.05);
            }
            "window-balance" => {
                self.window_mgr.balance();
            }
            "window-maximize" => {
                let mut saved = self.saved_maximize_layout.take();
                self.window_mgr.maximize_toggle(&mut saved);
                self.saved_maximize_layout = saved;
            }
            "window-move-left" => {
                let area = self.default_area();
                self.window_mgr.move_window(Direction::Left, area);
            }
            "window-move-right" => {
                let area = self.default_area();
                self.window_mgr.move_window(Direction::Right, area);
            }
            "window-move-up" => {
                let area = self.default_area();
                self.window_mgr.move_window(Direction::Up, area);
            }
            "window-move-down" => {
                let area = self.default_area();
                self.window_mgr.move_window(Direction::Down, area);
            }
            "focus-next-window" => {
                self.fire_hook("focus-out");
                self.save_mode_to_buffer();
                self.window_mgr.focus_next();
                self.sync_mode_to_buffer();
                self.fire_hook("focus-in");
            }
            _ => return None,
        }
        Some(true)
    }
}
