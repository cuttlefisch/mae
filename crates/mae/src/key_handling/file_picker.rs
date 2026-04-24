use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mae_core::{Editor, Mode};

pub(super) fn handle_file_picker_mode(editor: &mut Editor, key: KeyEvent) {
    let picker = match editor.file_picker.as_mut() {
        Some(p) => p,
        None => {
            editor.set_mode(Mode::Normal);
            return;
        }
    };

    match key.code {
        KeyCode::Esc => {
            editor.file_picker = None;
            editor.set_mode(Mode::Normal);
        }
        KeyCode::Enter => {
            if let Some(path) = picker.selected_path() {
                let creating = picker.query_selected && !path.exists();
                editor.file_picker = None;
                editor.set_mode(Mode::Normal);
                if creating {
                    // Create parent directories and an empty file, then open it.
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Err(e) = std::fs::write(&path, "") {
                        editor.set_status(format!("Cannot create file: {}", e));
                    } else {
                        editor.open_file(&path);
                    }
                } else {
                    editor.open_file(&path);
                }
            } else {
                editor.file_picker = None;
                editor.set_mode(Mode::Normal);
                editor.set_status("No file selected");
            }
        }
        KeyCode::Up | KeyCode::BackTab => {
            picker.move_up();
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            picker.move_up();
        }
        KeyCode::Down => {
            picker.move_down();
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            picker.move_down();
        }
        KeyCode::Tab => {
            // Try path completion for absolute/home paths first, then
            // Doom-style longest-common-prefix within the current root,
            // then fall back to cycling selection.
            // Both methods have side effects — can't collapse into a match guard.
            let completed = picker.complete_path_tab() || picker.complete_longest_prefix();
            if !completed {
                picker.move_down();
            }
        }
        KeyCode::Backspace => {
            if picker.query.is_empty() {
                editor.file_picker = None;
                editor.set_mode(Mode::Normal);
            } else {
                picker.query.pop();
                picker.update_filter();
            }
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Ctrl-U: clear the query line (Emacs/readline style).
            picker.clear_query();
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Ctrl-W: delete last path component or word.
            let q = &picker.query;
            let trimmed = q.trim_end_matches('/');
            let new_end = trimmed.rfind('/').map(|i| i + 1).unwrap_or(0);
            let new_query = picker.query[..new_end].to_string();
            picker.query = new_query;
            picker.update_filter();
        }
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            picker.move_up();
        }
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            picker.move_down();
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.file_picker = None;
            editor.set_mode(Mode::Normal);
        }
        KeyCode::Char(ch) => {
            picker.query.push(ch);
            // If the query now looks like `~/dir/` or `/abs/path/`,
            // switch the picker root to that directory.
            if ch == '/' && picker.maybe_switch_root() {
                // Root switched — filter already reset by rescan.
            } else {
                picker.update_filter();
            }
        }
        _ => {}
    }
}

/// Key handling for the ranger-style `FileBrowser` overlay.
///
/// Motion keys mirror vim where it makes sense (`j`/`k`, `h`/`l`), with
/// Enter activating the selection (descend or open). A typed query
/// narrows the current directory listing; descending clears it.
///
/// Exit via Esc / `q` / Ctrl-C.
pub(super) fn handle_file_browser_mode(editor: &mut Editor, key: KeyEvent) {
    use mae_core::file_browser::Activation;

    let browser = match editor.file_browser.as_mut() {
        Some(b) => b,
        None => {
            editor.set_mode(Mode::Normal);
            return;
        }
    };

    // Ctrl- bindings first so they can't be shadowed by plain-char handling.
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('c') => {
                editor.file_browser = None;
                editor.set_mode(Mode::Normal);
                return;
            }
            KeyCode::Char('j') => {
                browser.move_down();
                return;
            }
            KeyCode::Char('k') => {
                browser.move_up();
                return;
            }
            KeyCode::Char('u') => {
                browser.query.clear();
                browser.update_filter();
                return;
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Esc => {
            editor.file_browser = None;
            editor.set_mode(Mode::Normal);
        }
        KeyCode::Char('q') if browser.query.is_empty() => {
            editor.file_browser = None;
            editor.set_mode(Mode::Normal);
        }
        KeyCode::Enter | KeyCode::Char('l') if browser.query.is_empty() => {
            if let Activation::OpenFile(path) = browser.activate() {
                editor.file_browser = None;
                editor.set_mode(Mode::Normal);
                editor.open_file(&path);
            }
            // Descended / Nothing: stay in browser mode with refreshed listing.
        }
        // Enter while a query is active: check for path navigation first,
        // then activate the selected entry.
        KeyCode::Enter => {
            // If query looks like an absolute or home-relative path to a
            // directory, navigate there directly.
            let nav_path = if browser.query.starts_with('/') {
                Some(std::path::PathBuf::from(&browser.query))
            } else if browser.query.starts_with("~/") {
                let expanded = mae_core::file_picker::expand_tilde(&browser.query);
                Some(std::path::PathBuf::from(expanded))
            } else {
                None
            };
            if let Some(p) = nav_path {
                if p.is_dir() {
                    browser.cwd = p;
                    browser.refresh();
                } else if let Activation::OpenFile(path) = browser.activate() {
                    editor.file_browser = None;
                    editor.set_mode(Mode::Normal);
                    editor.open_file(&path);
                }
            } else if let Activation::OpenFile(path) = browser.activate() {
                editor.file_browser = None;
                editor.set_mode(Mode::Normal);
                editor.open_file(&path);
            }
        }
        KeyCode::Tab => {
            browser.complete_tab();
        }
        KeyCode::Up => browser.move_up(),
        KeyCode::Down => browser.move_down(),
        KeyCode::Char('k') if browser.query.is_empty() => browser.move_up(),
        KeyCode::Char('j') if browser.query.is_empty() => browser.move_down(),
        KeyCode::Char('h') if browser.query.is_empty() => browser.ascend(),
        KeyCode::Backspace => {
            if browser.query.is_empty() {
                // Empty query → Backspace means "go up one directory".
                browser.ascend();
            } else {
                browser.query.pop();
                browser.update_filter();
            }
        }
        KeyCode::Char(ch) => {
            browser.query.push(ch);
            browser.update_filter();
        }
        _ => {}
    }
}
