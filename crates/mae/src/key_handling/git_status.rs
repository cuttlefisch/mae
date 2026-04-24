use crossterm::event::{KeyCode, KeyEvent};
use mae_core::{Editor, Mode};

pub(super) fn handle_git_status_mode(editor: &mut Editor, key: KeyEvent) {
    let buf_idx = editor.active_buffer_idx();
    // Ensure we are actually in a GitStatus buffer
    if editor.buffers[buf_idx].kind != mae_core::BufferKind::GitStatus {
        editor.set_mode(Mode::Normal);
        return;
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            editor.set_mode(Mode::Normal);
        }
        KeyCode::Char('j') | KeyCode::Down => {
            editor.dispatch_builtin("move-down");
        }
        KeyCode::Char('k') | KeyCode::Up => {
            editor.dispatch_builtin("move-up");
        }
        KeyCode::Char('s') => {
            let win = editor.window_mgr.focused_window();
            let path = editor.buffers[buf_idx]
                .git_status
                .as_ref()
                .and_then(|view| view.lines.get(win.cursor_row))
                .and_then(|line| line.file_path.clone());

            if let Some(p) = path {
                editor.git_stage_file(&p);
            }
        }
        KeyCode::Char('u') => {
            let win = editor.window_mgr.focused_window();
            let path = editor.buffers[buf_idx]
                .git_status
                .as_ref()
                .and_then(|view| view.lines.get(win.cursor_row))
                .and_then(|line| line.file_path.clone());

            if let Some(p) = path {
                editor.git_unstage_file(&p);
            }
        }
        KeyCode::Char('g') => {
            editor.git_status();
        }
        KeyCode::Enter => {
            let win = editor.window_mgr.focused_window();
            let target = editor.buffers[buf_idx]
                .git_status
                .as_ref()
                .and_then(|view| {
                    view.lines
                        .get(win.cursor_row)
                        .and_then(|line| line.file_path.as_ref().map(|p| view.repo_root.join(p)))
                });

            if let Some(full_path) = target {
                editor.open_file(full_path);
            }
        }
        _ => {}
    }
}
