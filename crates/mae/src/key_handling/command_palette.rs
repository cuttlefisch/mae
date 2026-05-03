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
                        editor.switch_to_buffer(idx);
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
    use mae_core::command_palette::{MiniDialogContext, MiniDialogKind};

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
            // Apply the dialog result
            let dialog = match editor.mini_dialog.take() {
                Some(d) => d,
                None => return,
            };
            editor.command_palette = None;
            editor.set_mode(Mode::Normal);

            match (&dialog.kind, &dialog.context) {
                (
                    MiniDialogKind::EditLink,
                    MiniDialogContext::LinkEdit {
                        buf_idx,
                        byte_start,
                        byte_end,
                        is_org,
                    },
                ) => {
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
            }
        }
        KeyCode::Char(ch) => {
            if let Some(ref mut dialog) = editor.mini_dialog {
                dialog.fields[dialog.active_field].value.push(ch);
            }
        }
        _ => {}
    }
}
