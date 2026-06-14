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
            let selected_entry = palette
                .selected_name()
                .and_then(|_| palette.entry_at(palette.selected).cloned());
            let name = selected_entry.as_ref().map(|e| e.name.clone());
            let selected_doc = selected_entry.map(|e| e.doc.clone());
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
                (Some(flavor), PalettePurpose::SetKeymapFlavor) => {
                    // Live flavor switch (the "__flavor:" sentinel is applied in
                    // the event loop, which has the SchemeRuntime to reload).
                    editor
                        .pending_module_reloads
                        .push(format!("__flavor:{flavor}"));
                    editor.set_status(format!(
                        "Keybindings: {flavor} — add (set-option! \"keymap_flavor\" \"{flavor}\") to init.scm to persist"
                    ));
                }
                (Some(scope), PalettePurpose::SetKbSearchScope) => {
                    match editor.set_option("kb_search_scope", &scope) {
                        Ok(_) => editor.set_status(format!(
                            "KB search scope: {scope} — add (set-option! \"kb_search_scope\" \"{scope}\") to init.scm to persist"
                        )),
                        Err(e) => editor.set_status(e),
                    }
                }
                (Some(node_id), PalettePurpose::KbSearch)
                | (Some(node_id), PalettePurpose::KbFindOrCreate) => {
                    editor.open_help_at(&node_id);
                }
                (None, PalettePurpose::KbFindOrCreate) => {
                    let title = query.trim();
                    if title.is_empty() {
                        editor.set_status("Note title cannot be empty");
                    } else {
                        match editor.kb_create_note_from_title(title) {
                            Ok(_) => {}
                            Err(e) => editor.set_status(e),
                        }
                    }
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
                (Some(root_str), PalettePurpose::ForgetProject) => {
                    editor.remove_project(&root_str);
                }
                (None, PalettePurpose::KbInsertLink) => {
                    // No match — create node from query, then insert link
                    let title = query.trim();
                    if title.is_empty() {
                        editor.set_status("Note title cannot be empty");
                    } else {
                        match editor.kb_create_note_from_title(title) {
                            Ok((new_id, _)) => {
                                let link = format!("[[{}|{}]]", new_id, title);
                                editor.insert_at_cursor(&link);
                                editor.set_status(format!("Created + linked: {}", title));
                            }
                            Err(e) => editor.set_status(e),
                        }
                    }
                }
                (Some(node_id), PalettePurpose::KbInsertLink) => {
                    // Insert [[id|title]] at cursor
                    let doc = selected_doc.unwrap_or_default();
                    let display = if doc.is_empty() { node_id.clone() } else { doc };
                    let link = format!("[[{}|{}]]", node_id, display);
                    editor.insert_at_cursor(&link);
                    // Record link for activity tracking.
                    editor.kb_record_link(&node_id);
                    editor.set_status(format!("Inserted link to {}", display));
                }
                (None, PalettePurpose::SwitchProject) => {
                    // No match selected — treat query as a typed path
                    if !query.is_empty() {
                        editor.add_project(&query);
                    } else {
                        editor.set_status("No project selected");
                    }
                }
                (Some(doc_name), PalettePurpose::CollabJoin) => {
                    editor.collab.pending_intent =
                        Some(mae_core::CollabIntent::JoinDoc { doc_id: doc_name });
                    editor.set_status("Joining document...");
                }
                (Some(provider), PalettePurpose::SetupAiProvider) => {
                    if provider == "skip" {
                        editor.set_status("AI setup skipped");
                        editor.setup_all_pending = false;
                    } else {
                        let default_model = match provider.as_str() {
                            "claude" => "claude-sonnet-4-20250514",
                            "openai" => "gpt-4o",
                            "gemini" => "gemini-2.5-flash",
                            "ollama" => "llama3",
                            "deepseek" => "deepseek-chat",
                            _ => "",
                        };
                        editor.mini_dialog =
                            Some(mae_core::command_palette::MiniDialogState::single_input(
                                "AI Model",
                                default_model,
                                default_model,
                                mae_core::command_palette::MiniDialogContext::SetupAiModel {
                                    provider: provider.clone(),
                                },
                            ));
                        editor.command_palette =
                            Some(mae_core::command_palette::CommandPalette::with_name_list(
                                &[],
                                mae_core::command_palette::PalettePurpose::MiniDialog,
                            ));
                        editor.set_mode(mae_core::Mode::CommandPalette);
                    }
                }
                (Some(mode), PalettePurpose::SetupCollabMode) => match mode.as_str() {
                    "skip" | "solo" => {
                        editor.set_status(if mode == "skip" {
                            "Collab setup skipped"
                        } else {
                            "Collaboration: solo mode"
                        });
                        if editor.setup_all_pending {
                            editor.dispatch_next_setup_section();
                        }
                    }
                    "loopback" => {
                        let _ = editor.set_option("collab_server_address", "127.0.0.1:9473");
                        let _ = editor.set_option("collab_auto_connect", "true");
                        let _ = editor.save_option_to_init("collab_server_address");
                        let _ = editor.save_option_to_init("collab_auto_connect");
                        editor.set_status("Collaboration: loopback (127.0.0.1:9473, auto-connect)");
                        editor.refresh_setup_hub();
                        if editor.setup_all_pending {
                            editor.dispatch_next_setup_section();
                        }
                    }
                    "network" => {
                        editor.mini_dialog =
                            Some(mae_core::command_palette::MiniDialogState::single_input(
                                "Server address",
                                "0.0.0.0:9473",
                                "0.0.0.0:9473",
                                mae_core::command_palette::MiniDialogContext::SetupCollabAddress,
                            ));
                        editor.command_palette =
                            Some(mae_core::command_palette::CommandPalette::with_name_list(
                                &[],
                                mae_core::command_palette::PalettePurpose::MiniDialog,
                            ));
                        editor.set_mode(mae_core::Mode::CommandPalette);
                    }
                    _ => editor.set_status("Unknown collab mode"),
                },
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
                // Lazy re-query for large KB-find palettes; client-filter otherwise.
                editor.kb_find_palette_query_changed();
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
            // Lazy re-query for large KB-find palettes; client-filter otherwise.
            editor.kb_find_palette_query_changed();
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
        MiniDialogContext::DailyGotoDate => {
            let date_str = dialog.fields[0].value.trim().to_string();
            if !date_str.is_empty() {
                if let Err(e) = editor.kb_goto_daily_date(&date_str) {
                    editor.set_status(format!("Daily: {}", e));
                }
            }
        }
        MiniDialogContext::CollabResolvePath {
            buf_idx,
            resolved_path,
        } => {
            let buf_idx = *buf_idx;
            if buf_idx < editor.buffers.len() {
                editor.buffers[buf_idx].set_file_path(resolved_path.clone());
                editor.set_status(format!("Mapped to local path: {}", resolved_path.display()));
            } else {
                editor.set_status("Buffer no longer exists".to_string());
            }
        }
        MiniDialogContext::RevertBuffer { buf_idx } => {
            let buf_idx = *buf_idx;
            if buf_idx < editor.buffers.len() {
                match editor.buffers[buf_idx].reload_from_disk() {
                    Ok(()) => {
                        let name = editor.buffers[buf_idx].name.clone();
                        editor.set_status(format!("Reloaded: {}", name));
                    }
                    Err(e) => editor.set_status(format!("Reload failed: {}", e)),
                }
                editor.fire_hook("file-changed-on-disk");
            }
        }

        // --- Setup wizard mini-dialog chains ---
        MiniDialogContext::SetupAiModel { provider } => {
            let model = dialog.fields[0].value.trim().to_string();
            if model.is_empty() {
                editor.set_status("AI setup cancelled");
                editor.setup_all_pending = false;
                return;
            }
            let provider = provider.clone();
            // Open next step: API key command
            let hint = match provider.as_str() {
                "claude" => "security find-generic-password -s mae-ai -w",
                "openai" => "security find-generic-password -s mae-openai -w",
                "ollama" => "",
                _ => "",
            };
            editor.mini_dialog = Some(mae_core::command_palette::MiniDialogState::single_input(
                "API key command (blank = env var)",
                "",
                hint,
                mae_core::command_palette::MiniDialogContext::SetupAiKeyCommand {
                    provider: provider.clone(),
                    model,
                },
            ));
            editor.command_palette =
                Some(mae_core::command_palette::CommandPalette::with_name_list(
                    &[],
                    mae_core::command_palette::PalettePurpose::MiniDialog,
                ));
            editor.set_mode(mae_core::Mode::CommandPalette);
        }
        MiniDialogContext::SetupAiKeyCommand { provider, model } => {
            let key_cmd = dialog.fields[0].value.trim().to_string();
            let provider = provider.clone();
            let model = model.clone();
            // Apply all three AI options
            let _ = editor.set_option("ai_provider", &provider);
            let _ = editor.set_option("ai_model", &model);
            let _ = editor.save_option_to_init("ai_provider");
            let _ = editor.save_option_to_init("ai_model");
            if !key_cmd.is_empty() {
                let _ = editor.set_option("ai_api_key_command", &key_cmd);
                let _ = editor.save_option_to_init("ai_api_key_command");
            }
            editor.set_status(format!("AI configured: {} / {}", provider, model));
            editor.refresh_setup_hub();
            if editor.setup_all_pending {
                editor.dispatch_next_setup_section();
            }
        }
        MiniDialogContext::SetupCollabAddress => {
            let address = dialog.fields[0].value.trim().to_string();
            if address.is_empty() {
                editor.set_status("Collab setup cancelled");
                editor.setup_all_pending = false;
                return;
            }
            // Chain to PSK input
            let hint = if cfg!(target_os = "macos") {
                "security find-generic-password -s mae-collab-psk -a mae -w"
            } else {
                "pass show mae/collab-psk"
            };
            editor.mini_dialog = Some(mae_core::command_palette::MiniDialogState::single_input(
                "PSK command (blank = no auth)",
                "",
                hint,
                mae_core::command_palette::MiniDialogContext::SetupCollabPsk { address },
            ));
            editor.command_palette =
                Some(mae_core::command_palette::CommandPalette::with_name_list(
                    &[],
                    mae_core::command_palette::PalettePurpose::MiniDialog,
                ));
            editor.set_mode(mae_core::Mode::CommandPalette);
        }
        MiniDialogContext::SetupCollabPsk { address } => {
            let psk_cmd = dialog.fields[0].value.trim().to_string();
            let address = address.clone();
            let _ = editor.set_option("collab_server_address", &address);
            let _ = editor.set_option("collab_auto_connect", "true");
            let _ = editor.save_option_to_init("collab_server_address");
            let _ = editor.save_option_to_init("collab_auto_connect");
            if !psk_cmd.is_empty() {
                let _ = editor.set_option("collab_psk_command", &psk_cmd);
                let _ = editor.save_option_to_init("collab_psk_command");
            }
            editor.set_status(format!("Collaboration: network ({})", address));
            editor.refresh_setup_hub();
            if editor.setup_all_pending {
                editor.dispatch_next_setup_section();
            }
        }
        MiniDialogContext::SetupKbNotesDir => {
            let dir_str = dialog.fields[0].value.trim().to_string();
            if dir_str.is_empty() {
                editor.set_status("KB notes setup cancelled");
                editor.setup_all_pending = false;
                return;
            }
            // Expand ~ and create directory
            let expanded = if dir_str.starts_with('~') {
                if let Ok(home) = std::env::var("HOME") {
                    dir_str.replacen('~', &home, 1)
                } else {
                    dir_str.clone()
                }
            } else {
                dir_str.clone()
            };
            let _ = std::fs::create_dir_all(&expanded);
            let _ = editor.set_option("kb_notes_dir", &dir_str);
            let _ = editor.save_option_to_init("kb_notes_dir");
            editor.set_status(format!("KB notes directory: {}", expanded));
            editor.refresh_setup_hub();
            if editor.setup_all_pending {
                editor.dispatch_next_setup_section();
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
