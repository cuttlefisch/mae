//! Window management: split, resize, focus, close, balance.

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
                    Ok(_) => self.fire_hook("window-split"),
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
                    Ok(_) => self.fire_hook("window-split"),
                    Err(e) => self.set_status(e),
                }
            }
            "close-window" => {
                self.fire_hook("window-close");
                let focused = self.window_mgr.focused_id();
                if self.window_mgr.is_in_group(focused) {
                    // Group-aware close: close all members of the group
                    let buf_indices = self.window_mgr.close_group(focused);
                    if buf_indices.is_empty() {
                        // close_group refused (would leave 0 windows).
                        // If this is a conversation group, tear it down and
                        // restore the single-window layout with the previous buffer.
                        if self.conversation_pair.is_some() {
                            let pair = self.conversation_pair.take().unwrap();
                            // Collect conversation buffer indices to remove (in reverse order).
                            let mut to_remove = vec![pair.output_buffer_idx, pair.input_buffer_idx];
                            to_remove.sort_unstable();
                            to_remove.dedup();
                            // Find a destination buffer (alternate or first non-conversation).
                            let dest = self
                                .alternate_buffer_idx
                                .filter(|&i| i < self.buffers.len() && !to_remove.contains(&i))
                                .or_else(|| {
                                    self.buffers
                                        .iter()
                                        .enumerate()
                                        .position(|(i, _)| !to_remove.contains(&i))
                                })
                                .unwrap_or(0);
                            // Reset window manager to single window showing dest.
                            self.window_mgr.reset_to_single(dest);
                            // Remove conversation buffers (highest index first).
                            for &idx in to_remove.iter().rev() {
                                if idx < self.buffers.len() {
                                    self.buffers.remove(idx);
                                    self.notify_buffer_removed(idx);
                                    for win in self.window_mgr.iter_windows_mut() {
                                        if win.buffer_idx > idx {
                                            win.buffer_idx -= 1;
                                        }
                                    }
                                }
                            }
                            self.set_mode(crate::Mode::Normal);
                        } else {
                            self.set_status("Cannot close last window");
                        }
                    }
                    // Clear conversation pair if we closed its windows
                    if let Some(ref pair) = self.conversation_pair {
                        if buf_indices.contains(&pair.output_buffer_idx)
                            || buf_indices.contains(&pair.input_buffer_idx)
                        {
                            self.conversation_pair = None;
                        }
                    }
                } else if self
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
            "window-grow-width" => {
                self.window_mgr.adjust_ratio(Direction::Right, 0.05);
            }
            "window-shrink-width" => {
                self.window_mgr.adjust_ratio(Direction::Left, 0.05);
            }
            "window-grow-height" => {
                self.window_mgr.adjust_ratio(Direction::Down, 0.05);
            }
            "window-shrink-height" => {
                self.window_mgr.adjust_ratio(Direction::Up, 0.05);
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
                self.save_mode_to_buffer();
                let area = self.default_area();
                self.window_mgr.move_window(Direction::Left, area);
                self.sync_mode_to_buffer();
            }
            "window-move-right" => {
                self.save_mode_to_buffer();
                let area = self.default_area();
                self.window_mgr.move_window(Direction::Right, area);
                self.sync_mode_to_buffer();
            }
            "window-move-up" => {
                self.save_mode_to_buffer();
                let area = self.default_area();
                self.window_mgr.move_window(Direction::Up, area);
                self.sync_mode_to_buffer();
            }
            "window-move-down" => {
                self.save_mode_to_buffer();
                let area = self.default_area();
                self.window_mgr.move_window(Direction::Down, area);
                self.sync_mode_to_buffer();
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
        self.mark_full_redraw();
        Some(true)
    }
}
