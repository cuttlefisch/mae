use super::dispatch_command;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mae_core::PalettePurpose;
use mae_core::{Editor, Mode};
use mae_scheme::SchemeRuntime;

pub(super) fn handle_command_palette_mode(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    key: KeyEvent,
) {
    // Redirect to mini-dialog handler if active.
    if editor.mini_dialog.is_some() {
        handle_mini_dialog(editor, key);
        return;
    }

    // Pull the selected command name out *before* doing anything that
    // might need a mutable borrow on `editor` (like closing the palette
    // and dispatching). This avoids borrow-checker friction.
    let palette = match editor.command_palette.as_mut() {
        Some(p) => p,
        None => {
            editor.set_mode(Mode::Normal);
            return;
        }
    };

    match key.code {
        KeyCode::Esc => {
            editor.command_palette = None;
            editor.set_mode(Mode::Normal);
        }
        KeyCode::Enter => {
            let name = palette.selected_name().map(|s| s.to_string());
            let purpose = palette.purpose;
            let query = palette.query.clone();
            editor.command_palette = None;
            editor.set_mode(Mode::Normal);
            match (name, purpose) {
                (Some(cmd), PalettePurpose::Execute) => dispatch_command(editor, scheme, &cmd),
                (Some(cmd), PalettePurpose::Describe) => {
                    editor.open_help_at(&format!("cmd:{}", cmd))
                }
                (Some(theme), PalettePurpose::SetTheme) => {
                    editor.set_theme_by_name(&theme);
                    crate::config::persist_editor_preference("theme", &theme);
                }
                (Some(node_id), PalettePurpose::HelpSearch) => {
                    editor.open_help_at(&node_id);
                }
                (Some(buf_name), PalettePurpose::SwitchBuffer) => {
                    if buf_name == "*Messages*" {
                        // Create on demand if not yet opened
                        editor.open_messages_buffer();
                    } else if buf_name == "*AI*" || buf_name == "*ai-input*" {
                        // Restore the 85%/15% conversation split layout.
                        editor.open_conversation_buffer();
                    } else if let Some(idx) = editor.buffers.iter().position(|b| b.name == buf_name)
                    {
                        editor.display_buffer_and_focus(idx);
                        editor.sync_mode_to_buffer();
                    }
                }
                (Some(path), PalettePurpose::RecentFile) => {
                    editor.open_file(&path);
                }
                (Some(art), PalettePurpose::SetSplashArt) => {
                    editor.splash_art = Some(art.clone());
                    editor.set_status(format!("Splash art set to: {}", art));
                    crate::config::persist_editor_preference("splash_art", &art);
                }
                (Some(mode), PalettePurpose::AiMode) => {
                    let _ = editor.set_option("ai-mode", &mode);
                    crate::config::persist_editor_preference("ai.mode", &mode);
                }
                (Some(profile), PalettePurpose::AiProfile) => {
                    let _ = editor.set_option("ai-profile", &profile);
                    crate::config::persist_editor_preference("ai.profile", &profile);
                }
                (Some(branch), PalettePurpose::GitBranch) => {
                    editor.git_branch_switch(&branch);
                }
                (Some(root_str), PalettePurpose::SwitchProject) => {
                    editor.add_project(&root_str);
                }
                (None, PalettePurpose::SwitchProject) => {
                    // No match selected — treat query as a typed path
                    if !query.is_empty() {
                        editor.add_project(&query);
                    } else {
                        editor.set_status("No project selected");
                    }
                }
                (_, PalettePurpose::MiniDialog) => {
                    // Handled by handle_mini_dialog — should not reach here
                }
                (None, _) => editor.set_status("No command selected"),
            }
        }
        KeyCode::Up | KeyCode::BackTab => {
            palette.move_up();
        }
        KeyCode::Down | KeyCode::Tab => {
            palette.move_down();
        }
        KeyCode::Backspace => {
            if palette.query.is_empty() {
                editor.command_palette = None;
                editor.set_mode(Mode::Normal);
            } else {
                palette.query.pop();
                palette.update_filter();
            }
        }
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            palette.move_up();
        }
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            palette.move_down();
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.command_palette = None;
            editor.set_mode(Mode::Normal);
        }
        KeyCode::Char(ch) => {
            palette.query.push(ch);
            palette.update_filter();
        }
        _ => {}
    }
}

/// Handle keyboard input for a mini-dialog (edit-link, etc.).
fn handle_mini_dialog(editor: &mut Editor, key: KeyEvent) {
    let is_confirm = editor.mini_dialog.as_ref().is_some_and(|d| d.is_confirm());

    // Confirm dialogs only accept Enter (yes) or Esc (no).
    if is_confirm {
        match key.code {
            KeyCode::Esc | KeyCode::Char('n') => {
                editor.mini_dialog = None;
                editor.command_palette = None;
                editor.set_mode(Mode::Normal);
                editor.set_status("Cancelled");
            }
            KeyCode::Enter | KeyCode::Char('y') => {
                let dialog = editor.mini_dialog.take().unwrap();
                editor.command_palette = None;
                editor.set_mode(Mode::Normal);
                apply_mini_dialog(editor, dialog);
            }
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Esc => {
            editor.mini_dialog = None;
            editor.command_palette = None;
            editor.set_mode(Mode::Normal);
            editor.set_status("Cancelled");
        }
        KeyCode::Tab => {
            if let Some(ref mut dialog) = editor.mini_dialog {
                dialog.active_field = (dialog.active_field + 1) % dialog.fields.len();
            }
        }
        KeyCode::BackTab => {
            if let Some(ref mut dialog) = editor.mini_dialog {
                if dialog.active_field == 0 {
                    dialog.active_field = dialog.fields.len() - 1;
                } else {
                    dialog.active_field -= 1;
                }
            }
        }
        KeyCode::Backspace => {
            if let Some(ref mut dialog) = editor.mini_dialog {
                let field = &mut dialog.fields[dialog.active_field];
                field.value.pop();
            }
        }
        KeyCode::Enter => {
            let dialog = match editor.mini_dialog.take() {
                Some(d) => d,
                None => return,
            };
            editor.command_palette = None;
            editor.set_mode(Mode::Normal);
            apply_mini_dialog(editor, dialog);
        }
        KeyCode::Char(ch) => {
            if let Some(ref mut dialog) = editor.mini_dialog {
                dialog.fields[dialog.active_field].value.push(ch);
            }
        }
        _ => {}
    }
}

/// Centralized apply handler for all MiniDialog contexts.
fn apply_mini_dialog(editor: &mut Editor, dialog: mae_core::command_palette::MiniDialogState) {
    use mae_core::command_palette::MiniDialogContext;

    match &dialog.context {
        MiniDialogContext::LinkEdit {
            buf_idx,
            byte_start,
            byte_end,
            is_org,
        } => {
            let url = &dialog.fields[0].value;
            let label = &dialog.fields[1].value;
            let new_text = if *is_org {
                mae_core::display_region::build_org_link(
                    url,
                    if label.is_empty() { None } else { Some(label) },
                )
            } else {
                mae_core::display_region::build_md_link(url, label)
            };
            let buf_idx = *buf_idx;
            let byte_start = *byte_start;
            let byte_end = *byte_end;
            if buf_idx < editor.buffers.len() {
                let buf = &mut editor.buffers[buf_idx];
                let start_char = buf.rope().byte_to_char(byte_start);
                let end_char = buf.rope().byte_to_char(byte_end);
                buf.delete_range(start_char, end_char);
                buf.insert_text_at(start_char, &new_text);
                editor.set_status("Link updated");
            }
        }
        MiniDialogContext::FileDelete { path, close_buffer } => {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let result = if path.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            };
            match result {
                Ok(()) => {
                    if *close_buffer {
                        editor.dispatch_builtin("force-kill-buffer");
                    }
                    // Refresh file tree if open
                    let tree_idx = editor
                        .buffers
                        .iter()
                        .position(|b| b.kind == mae_core::BufferKind::FileTree);
                    if let Some(ti) = tree_idx {
                        if let Some(ft) = editor.buffers[ti].file_tree_mut() {
                            ft.refresh();
                        }
                    }
                    editor.set_status(format!("Deleted {}", name));
                }
                Err(e) => editor.set_status(format!("Delete failed: {}", e)),
            }
        }
        MiniDialogContext::FileRename { old_path } => {
            let new_path_str = &dialog.fields[0].value;
            if new_path_str.is_empty() {
                editor.set_status("Rename cancelled");
                return;
            }
            let new_path = std::path::PathBuf::from(new_path_str);
            match std::fs::rename(old_path, &new_path) {
                Ok(()) => {
                    let idx = editor.active_buffer_idx();
                    editor.buffers[idx].set_file_path(new_path.clone());
                    editor.buffers[idx].name = new_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    editor.set_status(format!("Renamed to {}", new_path.display()));
                }
                Err(e) => editor.set_status(format!("Rename failed: {}", e)),
            }
        }
        MiniDialogContext::FileCopy { src_path } => {
            let dst_str = &dialog.fields[0].value;
            if dst_str.is_empty() {
                editor.set_status("Copy cancelled");
                return;
            }
            let dst = std::path::PathBuf::from(dst_str);
            match std::fs::copy(src_path, &dst) {
                Ok(_) => {
                    editor.open_file(&dst);
                    editor.set_status(format!("Copied to {}", dst.display()));
                }
                Err(e) => editor.set_status(format!("Copy failed: {}", e)),
            }
        }
        MiniDialogContext::FileSaveAs => {
            let path_str = &dialog.fields[0].value;
            if path_str.is_empty() {
                editor.set_status("Save cancelled");
                return;
            }
            let path = std::path::PathBuf::from(path_str);
            let idx = editor.active_buffer_idx();
            editor.buffers[idx].set_file_path(path.clone());
            editor.buffers[idx].name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            editor.dispatch_builtin("save");
        }
        MiniDialogContext::FileTreeRename { path } => {
            let new_name = &dialog.fields[0].value;
            if new_name.is_empty() {
                editor.set_status("Rename cancelled");
                return;
            }
            let new_path = path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join(new_name);
            match std::fs::rename(path, &new_path) {
                Ok(()) => {
                    let tree_idx = editor
                        .buffers
                        .iter()
                        .position(|b| b.kind == mae_core::BufferKind::FileTree);
                    if let Some(ti) = tree_idx {
                        if let Some(ft) = editor.buffers[ti].file_tree_mut() {
                            ft.refresh();
                        }
                    }
                    editor.set_status(format!("Renamed to {}", new_name));
                }
                Err(e) => editor.set_status(format!("Rename failed: {}", e)),
            }
        }
        MiniDialogContext::FileTreeCreate { parent } => {
            let name = &dialog.fields[0].value;
            if name.is_empty() {
                editor.set_status("Create cancelled");
                return;
            }
            let target = parent.join(name);
            let result = if name.ends_with('/') {
                std::fs::create_dir_all(&target)
            } else {
                if let Some(p) = target.parent() {
                    let _ = std::fs::create_dir_all(p);
                }
                std::fs::write(&target, "")
            };
            match result {
                Ok(()) => {
                    let tree_idx = editor
                        .buffers
                        .iter()
                        .position(|b| b.kind == mae_core::BufferKind::FileTree);
                    if let Some(ti) = tree_idx {
                        if let Some(ft) = editor.buffers[ti].file_tree_mut() {
                            ft.refresh();
                        }
                    }
                    editor.set_status(format!("Created {}", name));
                }
                Err(e) => editor.set_status(format!("Create failed: {}", e)),
            }
        }
        MiniDialogContext::OrgSetTags { heading_line } => {
            let tag_input = dialog.fields[0].value.trim().to_string();
            let idx = editor.active_buffer_idx();
            let buf = &editor.buffers[idx];
            if *heading_line >= buf.line_count() {
                return;
            }
            let line_text: String = buf.rope().line(*heading_line).chars().collect();
            let new_line = rewrite_heading_tags(&line_text, &tag_input);
            let line_start = buf.rope().line_to_char(*heading_line);
            let line_end = if *heading_line + 1 < buf.line_count() {
                buf.rope().line_to_char(*heading_line + 1)
            } else {
                buf.rope().len_chars()
            };
            let buf = &mut editor.buffers[idx];
            buf.begin_undo_group();
            buf.delete_range(line_start, line_end);
            buf.insert_text_at(line_start, &new_line);
            buf.end_undo_group();
            editor.set_status("Tags updated");
        }
        MiniDialogContext::AgendaFilterTag => {
            let tag = dialog.fields[0].value.trim().to_string();
            if !tag.is_empty() {
                editor.set_status(format!("Agenda filter: :{tag}:"));
                // Agenda refresh with tag filter — handled by M8
            }
        }
    }
}

/// Rewrite an org heading line with new tags. Empty tag_input removes tags.
fn rewrite_heading_tags(line: &str, tag_input: &str) -> String {
    // Strip trailing `:tag1:tag2:` pattern from the line.
    let trimmed = line.trim_end_matches('\n').trim_end();
    let base = if let Some(last_space) = trimmed.rfind(char::is_whitespace) {
        let tail = &trimmed[last_space + 1..];
        if tail.starts_with(':') && tail.ends_with(':') && tail.len() >= 3 {
            trimmed[..last_space].trim_end()
        } else {
            trimmed
        }
    } else {
        trimmed
    };
    if tag_input.is_empty() {
        format!("{}\n", base)
    } else {
        let tags: Vec<&str> = tag_input.split(':').filter(|t| !t.is_empty()).collect();
        format!("{} :{}:\n", base, tags.join(":"))
    }
}
