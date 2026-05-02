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
