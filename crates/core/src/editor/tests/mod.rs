// Test modules split from monolithic tests.rs (339 tests)

pub(crate) use super::*;
pub(crate) use crate::buffer::Buffer;

mod buffer_tests;
mod change_tests;
mod command_tests;
mod count_tests;
mod editing_tests;
mod lsp_tests;
mod misc_tests;
mod mouse_tests;
mod navigation_tests;
mod operator_tests;
mod search_tests;
mod shell_tests;
mod text_object_tests;
mod visual_tests;

// Shared test helpers used across multiple test modules

pub(crate) fn editor_with_text(text: &str) -> Editor {
    let mut editor = Editor::new();
    for ch in text.chars() {
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, ch);
    }
    editor.window_mgr.focused_window_mut().cursor_row = 0;
    editor.window_mgr.focused_window_mut().cursor_col = 0;
    editor
}

pub(crate) fn ed_with_rust(src: &str) -> Editor {
    let mut buf = Buffer::new();
    buf.set_file_path(std::path::PathBuf::from("/tmp/x.rs"));
    let mut editor = Editor::with_buffer(buf);
    for ch in src.chars() {
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, ch);
    }
    editor.syntax.invalidate(0);
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 0;
    editor
}

pub(crate) fn ed_with_text(text: &str) -> Editor {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, text);
    let mut editor = Editor::with_buffer(buf);
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 0;
    editor
}
