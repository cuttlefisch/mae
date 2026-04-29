use crate::buffer::{Buffer, BufferKind};
use crate::window::SplitDirection;

use super::super::Editor;

impl Editor {
    /// Dispatch file-tree commands. Returns Some(true) if handled.
    pub(crate) fn dispatch_file_tree(&mut self, name: &str) -> Option<bool> {
        match name {
            "file-tree-toggle" => {
                if let Some(win_id) = self.file_tree_window_id.take() {
                    // Close the tree window. `close()` returns the buffer_idx it was showing.
                    let closed_buf_idx = self.window_mgr.close(win_id);
                    // Remove the tree buffer if it still exists.
                    if let Some(idx) = closed_buf_idx {
                        if idx < self.buffers.len()
                            && self.buffers[idx].kind == BufferKind::FileTree
                        {
                            self.buffers.remove(idx);
                            self.syntax.shift_after_remove(idx);
                            self.adjust_ai_target_after_remove(idx);
                            for win in self.window_mgr.iter_windows_mut() {
                                if win.buffer_idx > idx {
                                    win.buffer_idx -= 1;
                                }
                            }
                            if let Some(ref mut alt) = self.alternate_buffer_idx {
                                if *alt > idx {
                                    *alt -= 1;
                                }
                            }
                        }
                    }
                    self.set_status("File tree closed");
                } else {
                    // Determine project root for the tree.
                    let root = self
                        .active_project_root()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| {
                            std::env::current_dir()
                                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                        });

                    // Add the tree buffer.
                    let tree_buf = Buffer::new_file_tree(&root);
                    let tree_buf_idx = self.buffers.len();
                    self.buffers.push(tree_buf);

                    // Remember which buffer the current window shows.
                    let original_buf_idx = self.active_buffer_idx();

                    // Split the focused window vertically with ~20% for the tree.
                    let area = self.default_area();
                    match self.window_mgr.split_with_ratio(
                        SplitDirection::Vertical,
                        original_buf_idx,
                        area,
                        0.2,
                    ) {
                        Ok(new_win_id) => {
                            // After split: focused window (left) shows original_buf_idx,
                            // new window (right) shows original_buf_idx too.
                            // We want: focused window (left) = tree, new window (right) = content.
                            // Swap: give focused window the tree buffer, new window keeps original.
                            let focused_id = self.window_mgr.focused_id();
                            if let Some(focused_win) = self.window_mgr.window_mut(focused_id) {
                                focused_win.buffer_idx = tree_buf_idx;
                            }
                            // The new window already has original_buf_idx, so focus it.
                            self.window_mgr.set_focused(new_win_id);
                            self.file_tree_window_id = Some(focused_id);
                            // Auto-reveal the current file in the tree.
                            if let Some(current_path) = self
                                .buffers
                                .get(original_buf_idx)
                                .and_then(|b| b.file_path().map(|p| p.to_path_buf()))
                            {
                                if let Some(ref mut ft) = self.buffers[tree_buf_idx].file_tree {
                                    ft.reveal(&current_path);
                                }
                            }
                            self.set_status("File tree opened");
                        }
                        Err(e) => {
                            // Window too small — clean up the buffer we just added.
                            self.buffers.pop();
                            self.set_status(format!("Cannot open file tree: {}", e));
                        }
                    }
                }
                Some(true)
            }
            "file-tree-up" => {
                let idx = self.active_buffer_idx();
                if let Some(ref mut ft) = self.buffers[idx].file_tree {
                    ft.move_up();
                }
                Some(true)
            }
            "file-tree-down" => {
                let idx = self.active_buffer_idx();
                if let Some(ref mut ft) = self.buffers[idx].file_tree {
                    ft.move_down();
                }
                Some(true)
            }
            "file-tree-expand" => {
                let idx = self.active_buffer_idx();
                if let Some(ref mut ft) = self.buffers[idx].file_tree {
                    ft.toggle_expand();
                }
                Some(true)
            }
            "file-tree-open" => {
                let idx = self.active_buffer_idx();
                let path = self.buffers[idx]
                    .file_tree
                    .as_ref()
                    .and_then(|ft| ft.selected_path().map(|p| p.to_path_buf()));
                if let Some(path) = path {
                    if path.is_file() {
                        // Focus a non-tree window to open the file in it.
                        let tree_win_id = self.file_tree_window_id;
                        let target_win = self
                            .window_mgr
                            .iter_windows()
                            .find(|w| Some(w.id) != tree_win_id)
                            .map(|w| w.id);
                        if let Some(win_id) = target_win {
                            self.window_mgr.set_focused(win_id);
                        }
                        self.open_file(&path);
                    } else if path.is_dir() {
                        // Toggle expand for directories.
                        if let Some(ref mut ft) = self.buffers[idx].file_tree {
                            ft.toggle_expand();
                        }
                    }
                }
                Some(true)
            }
            "file-tree-first" => {
                let idx = self.active_buffer_idx();
                if let Some(ref mut ft) = self.buffers[idx].file_tree {
                    ft.move_to_first();
                }
                Some(true)
            }
            "file-tree-last" => {
                let idx = self.active_buffer_idx();
                if let Some(ref mut ft) = self.buffers[idx].file_tree {
                    ft.move_to_last();
                }
                Some(true)
            }
            "file-tree-close-parent" => {
                let idx = self.active_buffer_idx();
                if let Some(ref mut ft) = self.buffers[idx].file_tree {
                    ft.close_parent();
                }
                Some(true)
            }
            "file-tree-cd" => {
                let idx = self.active_buffer_idx();
                let path = self.buffers[idx]
                    .file_tree
                    .as_ref()
                    .and_then(|ft| ft.selected_path().map(|p| p.to_path_buf()));
                if let Some(path) = path {
                    if path.is_dir() {
                        let display = path.display().to_string();
                        if let Some(ref mut ft) = self.buffers[idx].file_tree {
                            ft.change_root(&path);
                        }
                        self.buffers[idx].name = format!("[Tree] {}", display);
                        self.set_status(format!("Root: {}", display));
                    } else {
                        self.set_status("Not a directory");
                    }
                }
                Some(true)
            }
            "file-tree-parent" => {
                let idx = self.active_buffer_idx();
                let new_root = self.buffers[idx]
                    .file_tree
                    .as_ref()
                    .and_then(|ft| ft.root.parent().map(|p| p.to_path_buf()));
                if let Some(new_root) = new_root {
                    let display = new_root.display().to_string();
                    if let Some(ref mut ft) = self.buffers[idx].file_tree {
                        ft.go_parent_root();
                    }
                    self.buffers[idx].name = format!("[Tree] {}", display);
                    self.set_status(format!("Root: {}", display));
                }
                Some(true)
            }
            "file-tree-delete" => {
                let idx = self.active_buffer_idx();
                let path = self.buffers[idx]
                    .file_tree
                    .as_ref()
                    .and_then(|ft| ft.selected_path().map(|p| p.to_path_buf()));
                if let Some(path) = path {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    self.set_status(format!("Delete {}? (y/n)", name));
                    self.pending_file_delete = Some((path, false));
                }
                Some(true)
            }
            "file-tree-rename" => {
                let idx = self.active_buffer_idx();
                let path = self.buffers[idx]
                    .file_tree
                    .as_ref()
                    .and_then(|ft| ft.selected_path().map(|p| p.to_path_buf()));
                if let Some(path) = path {
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    self.file_tree_action = Some(crate::file_tree::FileTreeAction::Rename(path));
                    self.set_mode(crate::Mode::Command);
                    self.command_line = name;
                    self.command_cursor = self.command_line.len();
                    self.set_status("Rename to:");
                }
                Some(true)
            }
            "file-tree-create" => {
                let idx = self.active_buffer_idx();
                // Use selected dir, or parent of selected file
                let parent = self.buffers[idx].file_tree.as_ref().and_then(|ft| {
                    ft.selected_path().map(|p| {
                        if p.is_dir() {
                            p.to_path_buf()
                        } else {
                            p.parent()
                                .unwrap_or_else(|| std::path::Path::new("."))
                                .to_path_buf()
                        }
                    })
                });
                if let Some(parent) = parent {
                    self.file_tree_action = Some(crate::file_tree::FileTreeAction::Create(parent));
                    self.set_mode(crate::Mode::Command);
                    self.command_line = String::new();
                    self.command_cursor = 0;
                    self.set_status("Create (end with / for dir):");
                }
                Some(true)
            }
            "delete-this-file" => {
                let idx = self.active_buffer_idx();
                if let Some(path) = self.buffers[idx].file_path().map(|p| p.to_path_buf()) {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    self.set_status(format!("Delete {}? (y/n)", name));
                    self.pending_file_delete = Some((path, true));
                } else {
                    self.set_status("Buffer has no file");
                }
                Some(true)
            }
            "file-tree-refresh" => {
                let idx = self.active_buffer_idx();
                if let Some(ref mut ft) = self.buffers[idx].file_tree {
                    ft.refresh();
                    self.set_status("File tree refreshed");
                }
                Some(true)
            }
            "file-tree-open-vsplit" | "file-tree-open-hsplit" => {
                let idx = self.active_buffer_idx();
                let path = self.buffers[idx]
                    .file_tree
                    .as_ref()
                    .and_then(|ft| ft.selected_path().map(|p| p.to_path_buf()));
                if let Some(path) = path {
                    if path.is_file() {
                        // Focus a non-tree window first
                        let tree_win_id = self.file_tree_window_id;
                        let target_win = self
                            .window_mgr
                            .iter_windows()
                            .find(|w| Some(w.id) != tree_win_id)
                            .map(|w| w.id);
                        if let Some(win_id) = target_win {
                            self.window_mgr.set_focused(win_id);
                        }
                        // Split then open
                        if name == "file-tree-open-vsplit" {
                            self.dispatch_builtin("split-vertical");
                        } else {
                            self.dispatch_builtin("split-horizontal");
                        }
                        self.open_file(&path);
                    } else if path.is_dir() {
                        if let Some(ref mut ft) = self.buffers[idx].file_tree {
                            ft.toggle_expand();
                        }
                    }
                }
                Some(true)
            }
            "file-tree-scroll-down" => {
                let idx = self.active_buffer_idx();
                if let Some(ref mut ft) = self.buffers[idx].file_tree {
                    ft.scroll_down(1, 30); // approximate visible height
                }
                Some(true)
            }
            "file-tree-scroll-up" => {
                let idx = self.active_buffer_idx();
                if let Some(ref mut ft) = self.buffers[idx].file_tree {
                    ft.scroll_up(1);
                }
                Some(true)
            }
            "file-tree-half-page-down" => {
                let idx = self.active_buffer_idx();
                if let Some(ref mut ft) = self.buffers[idx].file_tree {
                    ft.half_page_down(30);
                }
                Some(true)
            }
            "file-tree-half-page-up" => {
                let idx = self.active_buffer_idx();
                if let Some(ref mut ft) = self.buffers[idx].file_tree {
                    ft.half_page_up(30);
                }
                Some(true)
            }
            "file-tree-global-cycle" => {
                let idx = self.active_buffer_idx();
                if let Some(ref mut ft) = self.buffers[idx].file_tree {
                    ft.global_cycle();
                }
                Some(true)
            }
            _ => None,
        }
    }
}
