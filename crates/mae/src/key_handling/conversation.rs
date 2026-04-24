use crate::ai_event_handler::PendingInteractiveEvent;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mae_ai::AiCommand;
use mae_core::{Editor, Mode};
use tracing::{info, warn};

fn submit_conversation_prompt(
    editor: &mut Editor,
    ai_tx: &Option<tokio::sync::mpsc::Sender<AiCommand>>,
    pending_interactive_event: &mut Option<PendingInteractiveEvent>,
) {
    let buf_idx = editor.active_buffer_idx();

    // Reject submissions while the previous turn is still streaming.
    // Otherwise a user who types faster than the provider responds ends
    // up with a visibly "off by one" transcript: multiple [You] blocks
    // appear before any [AI] reply, and the replies land interleaved
    // with the next batch of prompts. This guard keeps the conversation
    // strictly turn-by-turn so prompts stay aligned with their answers.
    let (already_streaming, has_input) = match editor.buffers[buf_idx].conversation.as_ref() {
        Some(conv) => (conv.streaming, !conv.input_line.is_empty()),
        None => (false, false),
    };
    if !has_input {
        editor.set_mode(Mode::Normal);
        return;
    }
    if already_streaming {
        editor.set_status("[AI] still responding — wait for the reply or press SPC a a to cancel");
        return;
    }

    let input = editor.buffers[buf_idx]
        .conversation
        .as_mut()
        .map(|conv| {
            let input = conv.input_line.clone();
            conv.push_user(&input);
            conv.input_line.clear();
            conv.input_cursor = 0;
            conv.scroll_to_bottom();

            // Only set streaming true if we are starting a NEW prompt turn,
            // not when fulfilling an interactive request.
            if pending_interactive_event.is_none() {
                conv.streaming = true;
                conv.streaming_start = Some(std::time::Instant::now());
            }
            input
        })
        .unwrap_or_default();

    editor.sync_conversation_buffer_rope();

    // If we have a pending interactive event, fulfill it instead of sending a prompt
    if let Some(event) = pending_interactive_event.take() {
        match event {
            PendingInteractiveEvent::AskUser(reply) => {
                let _ = reply.send(input);
                editor.set_status("[AI] User reply sent");
            }
            PendingInteractiveEvent::ProposeChanges(reply) => {
                // If the user types something while changes are proposed,
                // we'll assume they are rejecting or ignored for now.
                // In Phase 4, we'll add :ai-accept / :ai-reject commands.
                let _ = reply.send(false);
                editor.set_status("[AI] Changes rejected via chat");
            }
        }
        editor.set_mode(Mode::Normal);
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
        if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
            conv.end_streaming();
        }
    }
    editor.set_mode(Mode::Normal);
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
        KeyCode::Enter => {
            submit_conversation_prompt(editor, ai_tx, pending_interactive_event);
        }

        // --- Cancel / quit ---
        KeyCode::Char('c') if ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                if conv.streaming {
                    info!("user cancelled AI streaming");
                    conv.streaming = false;
                    conv.streaming_start = None;
                    conv.push_system("[cancelled]");
                    if let Some(tx) = ai_tx {
                        if tx.try_send(AiCommand::Cancel).is_err() {
                            warn!("failed to send cancel to AI session");
                        }
                    }
                    return;
                }
            }
            editor.running = false;
        }

        // --- Cursor movement ---
        KeyCode::Char('a') if ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_move_home();
            }
        }
        KeyCode::Char('e') if ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_move_end();
            }
        }
        KeyCode::Home => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_move_home();
            }
        }
        KeyCode::End => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_move_end();
            }
        }
        KeyCode::Char('b') if ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_move_backward();
            }
        }
        KeyCode::Char('f') if ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_move_forward();
            }
        }
        KeyCode::Left => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_move_backward();
            }
        }
        KeyCode::Right => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_move_forward();
            }
        }

        // --- Deletion ---
        KeyCode::Backspace => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_backspace();
            }
        }
        KeyCode::Char('h') if ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_backspace();
            }
        }
        KeyCode::Delete => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_delete_forward();
            }
        }
        KeyCode::Char('d') if ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_delete_forward();
            }
        }
        KeyCode::Char('w') if ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_kill_word_backward();
            }
        }
        KeyCode::Char('u') if ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_kill_to_start();
            }
        }
        KeyCode::Char('k') if ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_kill_to_end();
            }
        }

        // --- Scroll history (stay in input mode) ---
        KeyCode::PageUp => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.scroll_up(10);
            }
        }
        KeyCode::PageDown => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.scroll_down(10);
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

        KeyCode::Char(ch) if !ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_insert_char(ch);
                // Scroll to bottom when typing so the user sees the prompt.
                conv.scroll_to_bottom();
            }
        }

        KeyCode::Esc => {
            editor.set_mode(Mode::Normal);
        }

        _ => {
            // Fall through to standard keymap handling (Command keymap)
            // for unhandled keys, allowing custom bindings (like F1) in the AI prompt.
            super::normal::handle_keymap_mode(editor, scheme, key, &mut Vec::new());
        }
    }
}
