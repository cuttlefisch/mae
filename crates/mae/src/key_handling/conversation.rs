use crate::ai_event_handler::PendingInteractiveEvent;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mae_ai::AiCommand;
use mae_core::{Editor, Mode};
use tracing::{info, warn};

/// Read the full text from the input buffer, trimming trailing newlines.
fn read_input_text(editor: &Editor) -> String {
    if let Some(ref pair) = editor.conversation_pair {
        if pair.input_buffer_idx < editor.buffers.len() {
            let rope = editor.buffers[pair.input_buffer_idx].rope();
            let text: String = rope.chars().collect();
            return text.trim_end_matches('\n').to_string();
        }
    }
    // No conversation pair — empty input.
    String::new()
}

/// Clear the input buffer (for split-pair mode).
fn clear_input_buffer(editor: &mut Editor) {
    if let Some(ref pair) = editor.conversation_pair {
        if pair.input_buffer_idx < editor.buffers.len() {
            let buf = &mut editor.buffers[pair.input_buffer_idx];
            buf.replace_contents("");
            buf.modified = false;
            // Reset cursor in the input window.
            if let Some(win) = editor.window_mgr.window_mut(pair.input_window_id) {
                win.cursor_row = 0;
                win.cursor_col = 0;
                win.scroll_offset = 0;
            }
        }
    }
}

/// Scroll the output window to the bottom of the conversation.
///
/// Uses the output window's actual height (from `last_layout_area`) rather than
/// `editor.viewport_height` which reflects the focused window — typically the
/// small input pane, not the tall output pane.
pub fn scroll_output_to_bottom(editor: &mut Editor) {
    if let Some(ref pair) = editor.conversation_pair {
        if pair.output_buffer_idx < editor.buffers.len() {
            let total_lines = editor.buffers[pair.output_buffer_idx].display_line_count();

            // Compute the output window's real height from the layout tree.
            let output_vh = editor
                .window_mgr
                .layout_rects(editor.last_layout_area)
                .iter()
                .find(|(id, _)| *id == pair.output_window_id)
                .map(|(_, r)| (r.height as usize).saturating_sub(2))
                .unwrap_or(editor.viewport_height);

            tracing::debug!(
                total_lines,
                output_vh,
                layout_h = editor.last_layout_area.height,
                "scroll_output_to_bottom"
            );

            if let Some(win) = editor.window_mgr.window_mut(pair.output_window_id) {
                win.cursor_row = total_lines.saturating_sub(1);
                win.scroll_offset = total_lines.saturating_sub(output_vh);
            }
        }
        // Also reset conversation scroll to bottom.
        if let Some(conv) = editor.buffers[pair.output_buffer_idx].conversation_mut() {
            conv.scroll_to_bottom();
        }
    }
}

pub(crate) fn submit_conversation_prompt(
    editor: &mut Editor,
    ai_tx: &Option<tokio::sync::mpsc::Sender<AiCommand>>,
    pending_interactive_event: &mut Option<PendingInteractiveEvent>,
) {
    let input = read_input_text(editor);

    if input.is_empty() {
        return;
    }

    // Find the output buffer index.
    let output_idx = editor
        .conversation_pair
        .as_ref()
        .map(|p| p.output_buffer_idx)
        .unwrap_or_else(|| editor.active_buffer_idx());

    // Reject submissions while the previous turn is still streaming.
    let already_streaming = editor.buffers[output_idx]
        .conversation()
        .map(|conv| conv.streaming)
        .unwrap_or(false);

    if already_streaming {
        editor.set_status("[AI] still responding — wait for the reply or press SPC a a to cancel");
        return;
    }

    // Push user message to conversation + clear input buffer.
    if let Some(conv) = editor.buffers[output_idx].conversation_mut() {
        conv.push_user(&input);
        conv.scroll_to_bottom();

        if pending_interactive_event.is_none() {
            conv.streaming = true;
            conv.streaming_start = Some(std::time::Instant::now());
        }
    }

    clear_input_buffer(editor);
    editor.sync_conversation_buffer_rope();
    scroll_output_to_bottom(editor);

    // If we have a pending interactive event, fulfill it instead of sending a prompt.
    if let Some(event) = pending_interactive_event.take() {
        match event {
            PendingInteractiveEvent::AskUser(reply) => {
                let _ = reply.send(input);
                editor.set_status("[AI] User reply sent");
            }
            PendingInteractiveEvent::ProposeChanges(reply) => {
                let _ = reply.send(false);
                editor.set_status("[AI] Changes rejected via chat");
            }
        }
        return;
    }

    if let Some(tx) = ai_tx {
        if tx.try_send(AiCommand::Prompt(input)).is_err() {
            warn!("AI command channel full or closed — prompt dropped");
        }
        editor.set_status("[AI] Thinking...");
    } else {
        warn!("AI prompt submitted but no AI provider configured");
        editor.set_status("AI not configured. Set ANTHROPIC_API_KEY or OPENAI_API_KEY.");
        if let Some(conv) = editor.buffers[output_idx].conversation_mut() {
            conv.end_streaming();
        }
    }
}

pub(super) fn handle_conversation_input(
    editor: &mut Editor,
    scheme: &mut mae_scheme::SchemeRuntime,
    key: KeyEvent,
    ai_tx: &Option<tokio::sync::mpsc::Sender<AiCommand>>,
    pending_interactive_event: &mut Option<PendingInteractiveEvent>,
) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        // Enter submits; Shift-Enter (GUI) or Alt-Enter (TUI fallback) inserts newline.
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].insert_char(win, '\n');
        }
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].insert_char(win, '\n');
        }
        KeyCode::Enter => {
            submit_conversation_prompt(editor, ai_tx, pending_interactive_event);
        }

        // --- Cancel / quit ---
        KeyCode::Char('c') if ctrl => {
            // Find the output conversation buffer to check streaming state.
            let output_idx = editor
                .conversation_pair
                .as_ref()
                .map(|p| p.output_buffer_idx);
            let is_streaming = output_idx
                .and_then(|idx| editor.buffers.get(idx))
                .and_then(|b| b.conversation())
                .map(|conv| conv.streaming)
                .unwrap_or(false);

            if is_streaming {
                if let Some(idx) = output_idx {
                    if let Some(conv) = editor.buffers[idx].conversation_mut() {
                        info!("user cancelled AI streaming");
                        conv.streaming = false;
                        conv.streaming_start = None;
                        conv.push_system("[cancelled]");
                    }
                }
                if let Some(tx) = ai_tx {
                    if tx.try_send(AiCommand::Cancel).is_err() {
                        warn!("failed to send cancel to AI session");
                    }
                }
                return;
            }
            editor.running = false;
        }

        // --- Tab inserts soft-tab (spaces) ---
        KeyCode::Tab => {
            let tab_w: usize = 4;
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            let col = win.cursor_col;
            let spaces = tab_w - (col % tab_w);
            for _ in 0..spaces {
                editor.buffers[idx].insert_char(win, ' ');
            }
        }

        // --- Standard buffer editing (delegates to Buffer::insert_char etc.) ---
        KeyCode::Char(ch) if !ctrl => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].insert_char(win, ch);
        }
        KeyCode::Backspace => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].delete_char_backward(win);
        }
        KeyCode::Char('h') if ctrl => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].delete_char_backward(win);
        }
        KeyCode::Delete => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].delete_char_forward(win);
        }
        KeyCode::Char('d') if ctrl => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].delete_char_forward(win);
        }

        // --- Readline-style cursor movement ---
        KeyCode::Char('a') if ctrl => {
            editor.window_mgr.focused_window_mut().cursor_col = 0;
        }
        KeyCode::Char('e') if ctrl => {
            let idx = editor.active_buffer_idx();
            let row = editor.window_mgr.focused_window().cursor_row;
            let len = editor.buffers[idx].line_len(row);
            editor.window_mgr.focused_window_mut().cursor_col = len;
        }
        KeyCode::Char('b') if ctrl => {
            let win = editor.window_mgr.focused_window_mut();
            if win.cursor_col > 0 {
                win.cursor_col -= 1;
            }
        }
        KeyCode::Char('f') if ctrl => {
            let idx = editor.active_buffer_idx();
            let row = editor.window_mgr.focused_window().cursor_row;
            let len = editor.buffers[idx].line_len(row);
            let win = editor.window_mgr.focused_window_mut();
            if win.cursor_col < len {
                win.cursor_col += 1;
            }
        }
        KeyCode::Left => {
            let win = editor.window_mgr.focused_window_mut();
            if win.cursor_col > 0 {
                win.cursor_col -= 1;
            }
        }
        KeyCode::Right => {
            let idx = editor.active_buffer_idx();
            let row = editor.window_mgr.focused_window().cursor_row;
            let len = editor.buffers[idx].line_len(row);
            let win = editor.window_mgr.focused_window_mut();
            if win.cursor_col < len {
                win.cursor_col += 1;
            }
        }
        KeyCode::Home => {
            editor.window_mgr.focused_window_mut().cursor_col = 0;
        }
        KeyCode::End => {
            let idx = editor.active_buffer_idx();
            let row = editor.window_mgr.focused_window().cursor_row;
            let len = editor.buffers[idx].line_len(row);
            editor.window_mgr.focused_window_mut().cursor_col = len;
        }

        // --- Kill line ---
        KeyCode::Char('u') if ctrl => {
            let idx = editor.active_buffer_idx();
            let row = editor.window_mgr.focused_window().cursor_row;
            let col = editor.window_mgr.focused_window().cursor_col;
            if col > 0 {
                let start = editor.buffers[idx].char_offset_at(row, 0);
                let end = editor.buffers[idx].char_offset_at(row, col);
                editor.buffers[idx].delete_range(start, end);
                editor.window_mgr.focused_window_mut().cursor_col = 0;
            }
        }
        KeyCode::Char('k') if ctrl => {
            let idx = editor.active_buffer_idx();
            let row = editor.window_mgr.focused_window().cursor_row;
            let col = editor.window_mgr.focused_window().cursor_col;
            let line_len = editor.buffers[idx].line_len(row);
            if col < line_len {
                let start = editor.buffers[idx].char_offset_at(row, col);
                let end = editor.buffers[idx].char_offset_at(row, line_len);
                editor.buffers[idx].delete_range(start, end);
            }
        }
        KeyCode::Char('w') if ctrl => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].delete_word_backward(win);
        }

        // --- Scroll output window (stay in input mode) ---
        KeyCode::PageUp => {
            if let Some(ref pair) = editor.conversation_pair {
                if let Some(win) = editor.window_mgr.window_mut(pair.output_window_id) {
                    win.scroll_offset = win.scroll_offset.saturating_sub(10);
                    win.cursor_row = win.cursor_row.saturating_sub(10);
                }
            }
        }
        KeyCode::PageDown => {
            if let Some(ref pair) = editor.conversation_pair {
                let total = editor.buffers[pair.output_buffer_idx].display_line_count();
                if let Some(win) = editor.window_mgr.window_mut(pair.output_window_id) {
                    win.scroll_offset = (win.scroll_offset + 10).min(total.saturating_sub(1));
                    win.cursor_row = (win.cursor_row + 10).min(total.saturating_sub(1));
                }
            }
        }

        // --- Cycle AI Mode ---
        KeyCode::BackTab => {
            editor.ai_mode = match editor.ai_mode.as_str() {
                "standard" => "auto-accept".into(),
                "auto-accept" => "plan".into(),
                _ => "standard".into(),
            };
            editor.set_status(format!("[AI] Mode: {}", editor.ai_mode));
        }

        KeyCode::Esc => {
            editor.set_mode(Mode::Normal);
        }

        _ => {
            // Fall through to standard keymap handling for unhandled keys.
            super::normal::handle_keymap_mode(editor, scheme, key, &mut Vec::new());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_core::WinRect;

    /// Helper: create an editor with an AI conversation pair and a realistic layout.
    fn editor_with_conversation(layout_height: u16) -> Editor {
        let mut editor = Editor::new();
        editor.last_layout_area = WinRect {
            x: 0,
            y: 0,
            width: 80,
            height: layout_height,
        };
        editor.dispatch_builtin("ai-prompt");
        editor
    }

    #[test]
    fn scroll_uses_output_window_height_not_viewport_height() {
        let mut editor = editor_with_conversation(40);
        // Simulate the focused window being the small input pane.
        editor.viewport_height = 5;

        // Add a long response to the conversation output.
        let pair = editor.conversation_pair.clone().unwrap();
        if let Some(conv) = editor.buffers[pair.output_buffer_idx].conversation_mut() {
            let long_response = (0..60)
                .map(|i| format!("Line {}", i))
                .collect::<Vec<_>>()
                .join("\n");
            conv.push_assistant(&long_response);
            conv.push_system("Transcript saved to: /tmp/test.json");
        }
        editor.sync_conversation_buffer_rope();

        scroll_output_to_bottom(&mut editor);

        let total = editor.buffers[pair.output_buffer_idx].display_line_count();
        let win = editor.window_mgr.window(pair.output_window_id).unwrap();

        // If the old bug were present (using viewport_height=5), scroll_offset
        // would be total-5, leaving only 5 lines visible. With the real output
        // window height (~18+ rows from the split), many more lines should be visible.
        let visible_lines = total - win.scroll_offset;
        assert!(
            visible_lines > 10,
            "Only {} lines visible (scroll_offset={}, total={}); \
             output window height should be used, not viewport_height=5",
            visible_lines,
            win.scroll_offset,
            total
        );
    }

    #[test]
    fn scroll_falls_back_gracefully_with_zero_layout() {
        let mut editor = editor_with_conversation(0);
        editor.viewport_height = 20;

        let pair = editor.conversation_pair.clone().unwrap();
        if let Some(conv) = editor.buffers[pair.output_buffer_idx].conversation_mut() {
            conv.push_assistant("Test response");
        }
        editor.sync_conversation_buffer_rope();

        // Should not panic with a zero-height layout.
        scroll_output_to_bottom(&mut editor);
    }

    #[test]
    fn scroll_positions_cursor_at_last_line() {
        let mut editor = editor_with_conversation(40);

        let pair = editor.conversation_pair.clone().unwrap();
        if let Some(conv) = editor.buffers[pair.output_buffer_idx].conversation_mut() {
            conv.push_assistant("Hello world");
        }
        editor.sync_conversation_buffer_rope();

        scroll_output_to_bottom(&mut editor);

        let total = editor.buffers[pair.output_buffer_idx].display_line_count();
        let win = editor.window_mgr.window(pair.output_window_id).unwrap();
        assert_eq!(win.cursor_row, total.saturating_sub(1));
    }
}
