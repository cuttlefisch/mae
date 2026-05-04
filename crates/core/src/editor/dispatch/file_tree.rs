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
                                if let Some(ft) = self.buffers[tree_buf_idx].file_tree_mut() {
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
                if let Some(ft) = self.buffers[idx].file_tree_mut() {
                    ft.move_up();
                }
                Some(true)
            }
            "file-tree-down" => {
                let idx = self.active_buffer_idx();
                if let Some(ft) = self.buffers[idx].file_tree_mut() {
                    ft.move_down();
                }
                Some(true)
            }
            "file-tree-expand" => {
                let idx = self.active_buffer_idx();
                if let Some(ft) = self.buffers[idx].file_tree_mut() {
                    ft.toggle_expand();
                }
                Some(true)
            }
            "file-tree-open" => {
                let idx = self.active_buffer_idx();
                let path = self.buffers[idx]
                    .file_tree()
                    .and_then(|ft| ft.selected_path().map(|p| p.to_path_buf()));
                if let Some(path) = path {
                    if path.is_file() {
                        // Focus a non-tree, non-conversation window to open the file in.
                        let tree_win_id = self.file_tree_window_id;
                        let buffers = &self.buffers;
                        let conv_pair = &self.conversation_pair;
                        let target_win = self
                            .window_mgr
                            .iter_windows()
                            .find(|w| {
                                Some(w.id) != tree_win_id && {
                                    let bi = w.buffer_idx;
                                    !(bi < buffers.len()
                                        && (buffers[bi].kind == crate::BufferKind::Conversation
                                            || conv_pair
                                                .as_ref()
                                                .is_some_and(|p| bi == p.input_buffer_idx)))
                                }
                            })
                            .map(|w| w.id);
                        if let Some(win_id) = target_win {
                            self.window_mgr.set_focused(win_id);
                        }
                        self.open_file(&path);
                    } else if path.is_dir() {
                        // Toggle expand for directories.
                        if let Some(ft) = self.buffers[idx].file_tree_mut() {
                            ft.toggle_expand();
                        }
                    }
                }
                Some(true)
            }
            "file-tree-first" => {
                let idx = self.active_buffer_idx();
                if let Some(ft) = self.buffers[idx].file_tree_mut() {
                    ft.move_to_first();
                }
                Some(true)
            }
            "file-tree-last" => {
                let idx = self.active_buffer_idx();
                if let Some(ft) = self.buffers[idx].file_tree_mut() {
                    ft.move_to_last();
                }
                Some(true)
            }
            "file-tree-close-parent" => {
                let idx = self.active_buffer_idx();
                if let Some(ft) = self.buffers[idx].file_tree_mut() {
                    ft.close_parent();
                }
                Some(true)
            }
            "file-tree-cd" => {
                let idx = self.active_buffer_idx();
                let path = self.buffers[idx]
                    .file_tree()
                    .and_then(|ft| ft.selected_path().map(|p| p.to_path_buf()));
                if let Some(path) = path {
                    if path.is_dir() {
                        let display = path.display().to_string();
                        if let Some(ft) = self.buffers[idx].file_tree_mut() {
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
                    .file_tree()
                    .and_then(|ft| ft.root.parent().map(|p| p.to_path_buf()));
                if let Some(new_root) = new_root {
                    let display = new_root.display().to_string();
                    if let Some(ft) = self.buffers[idx].file_tree_mut() {
                        ft.go_parent_root();
                    }
                    self.buffers[idx].name = format!("[Tree] {}", display);
                    self.set_status(format!("Root: {}", display));
                }
                Some(true)
            }
            "file-tree-delete" => {
                use crate::command_palette::{MiniDialogContext, MiniDialogState};
                let idx = self.active_buffer_idx();
                let path = self.buffers[idx]
                    .file_tree()
                    .and_then(|ft| ft.selected_path().map(|p| p.to_path_buf()));
                if let Some(path) = path {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    self.mini_dialog = Some(MiniDialogState::confirm(
                        format!("Delete {}?", name),
                        MiniDialogContext::FileDelete {
                            path,
                            close_buffer: false,
                        },
                    ));
                    self.set_mode(crate::Mode::CommandPalette);
                }
                Some(true)
            }
            "file-tree-rename" => {
                use crate::command_palette::{MiniDialogContext, MiniDialogState};
                let idx = self.active_buffer_idx();
                let path = self.buffers[idx]
                    .file_tree()
                    .and_then(|ft| ft.selected_path().map(|p| p.to_path_buf()));
                if let Some(path) = path {
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    self.mini_dialog = Some(MiniDialogState::single_input(
                        "Rename to",
                        &name,
                        "new name",
                        MiniDialogContext::FileTreeRename { path },
                    ));
                    self.set_mode(crate::Mode::CommandPalette);
                }
                Some(true)
            }
            "file-tree-create" => {
                use crate::command_palette::{MiniDialogContext, MiniDialogState};
                let idx = self.active_buffer_idx();
                let parent = self.buffers[idx].file_tree().and_then(|ft| {
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
                    self.mini_dialog = Some(MiniDialogState::single_input(
                        "Create (end with / for dir)",
                        "",
                        "filename",
                        MiniDialogContext::FileTreeCreate { parent },
                    ));
                    self.set_mode(crate::Mode::CommandPalette);
                }
                Some(true)
            }
            "delete-this-file" => {
                use crate::command_palette::{MiniDialogContext, MiniDialogState};
                let idx = self.active_buffer_idx();
                if let Some(path) = self.buffers[idx].file_path().map(|p| p.to_path_buf()) {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    self.mini_dialog = Some(MiniDialogState::confirm(
                        format!("Delete {}?", name),
                        MiniDialogContext::FileDelete {
                            path,
                            close_buffer: true,
                        },
                    ));
                    self.set_mode(crate::Mode::CommandPalette);
                } else {
                    self.set_status("Buffer has no file");
                }
                Some(true)
            }
            "file-tree-refresh" => {
                let idx = self.active_buffer_idx();
                if let Some(ft) = self.buffers[idx].file_tree_mut() {
                    ft.refresh();
                    self.set_status("File tree refreshed");
                }
                Some(true)
            }
            "file-tree-open-vsplit" | "file-tree-open-hsplit" => {
                let idx = self.active_buffer_idx();
                let path = self.buffers[idx]
                    .file_tree()
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
                        if let Some(ft) = self.buffers[idx].file_tree_mut() {
                            ft.toggle_expand();
                        }
                    }
                }
                Some(true)
            }
            "file-tree-scroll-down" => {
                let idx = self.active_buffer_idx();
                if let Some(ft) = self.buffers[idx].file_tree_mut() {
                    ft.scroll_down(1, 30); // approximate visible height
                }
                Some(true)
            }
            "file-tree-scroll-up" => {
                let idx = self.active_buffer_idx();
                if let Some(ft) = self.buffers[idx].file_tree_mut() {
                    ft.scroll_up(1);
                }
                Some(true)
            }
            "file-tree-half-page-down" => {
                let idx = self.active_buffer_idx();
                if let Some(ft) = self.buffers[idx].file_tree_mut() {
                    ft.half_page_down(30);
                }
                Some(true)
            }
            "file-tree-half-page-up" => {
                let idx = self.active_buffer_idx();
                if let Some(ft) = self.buffers[idx].file_tree_mut() {
                    ft.half_page_up(30);
                }
                Some(true)
            }
            "file-tree-global-cycle" => {
                let idx = self.active_buffer_idx();
                if let Some(ft) = self.buffers[idx].file_tree_mut() {
                    ft.global_cycle();
                }
                Some(true)
            }
            _ => None,
        }
    }
}
