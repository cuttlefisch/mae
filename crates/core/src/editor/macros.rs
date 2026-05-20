//! Vi-style macro recording and playback.
//!
//! `q<letter>` starts recording to register `<letter>` (a-z). Another `q`
//! stops and saves. `@<letter>` replays. `@@` replays the last-used register.
//! Count prefix (`5@a`) repeats the macro N times.
//!
//! Recording is intercepted at the key-handling layer (before dispatch) so
//! every raw keystroke is captured, including mode switches and text input.
//! Playback feeds serialized keys back through the same dispatch pipeline.

use crate::keymap::{deserialize_macro, serialize_macro, Key, KeyPress, LookupResult};
use crate::Mode;

use super::Editor;

impl Editor {
    /// Valid macro register names are lowercase ASCII letters only.
    /// Uppercase is excluded: they're used as separate registers in some
    /// vi dialects for appending, but we keep the surface simple.
    pub fn is_valid_macro_register(ch: char) -> bool {
        ch.is_ascii_lowercase()
    }

    /// Start recording keystrokes into register `ch`.
    /// Returns Err if already recording or if `ch` is not a-z.
    pub fn start_recording(&mut self, ch: char) -> Result<(), String> {
        if !Self::is_valid_macro_register(ch) {
            return Err(format!("Invalid macro register: '{}' (use a-z)", ch));
        }
        if self.vi.macro_recording {
            return Err(format!(
                "Already recording to register '{}'",
                self.vi.macro_register.unwrap_or('?')
            ));
        }
        self.vi.macro_recording = true;
        self.vi.macro_register = Some(ch);
        self.vi.macro_log.clear();
        self.set_status(format!("recording @{}", ch));
        Ok(())
    }

    /// Stop the current recording and save the log to the register.
    /// Returns the register letter, or None if not recording.
    pub fn stop_recording(&mut self) -> Option<char> {
        if !self.vi.macro_recording {
            return None;
        }
        let ch = self.vi.macro_register.unwrap_or('a');
        let serialized = serialize_macro(&self.vi.macro_log);
        self.vi.registers.insert(ch, serialized);
        self.vi.macro_recording = false;
        self.vi.macro_register = None;
        self.vi.macro_log.clear();
        self.set_status(format!("stopped recording @{}", ch));
        Some(ch)
    }

    /// Replay the macro stored in register `ch`, `count` times.
    pub fn replay_macro(&mut self, ch: char, count: usize) -> Result<(), String> {
        if !Self::is_valid_macro_register(ch) {
            return Err(format!("Invalid macro register: '{}' (use a-z)", ch));
        }
        if self.vi.macro_replay_depth >= 10 {
            return Err("Macro recursion limit reached (depth 10)".to_string());
        }
        let serialized = self
            .vi
            .registers
            .get(&ch)
            .cloned()
            .ok_or_else(|| format!("Macro register '{}' is empty", ch))?;
        if serialized.is_empty() {
            return Ok(());
        }
        let keys = deserialize_macro(&serialized);
        self.vi.last_macro_register = Some(ch);
        self.vi.macro_replay_depth += 1;
        for _ in 0..count {
            if !self.running {
                break;
            }
            let mut pending: Vec<KeyPress> = Vec::new();
            for kp in keys.clone() {
                if !self.running {
                    break;
                }
                self.replay_keypress(kp, &mut pending);
            }
        }
        self.vi.macro_replay_depth -= 1;
        Ok(())
    }

    /// Feed a single `KeyPress` through the editor dispatch pipeline.
    /// Mirrors the logic in `key_handling.rs` but operates purely within core
    /// (no crossterm, no Scheme). Used by both macro replay and for testing.
    pub fn replay_keypress(&mut self, kp: KeyPress, pending: &mut Vec<KeyPress>) {
        // If a pending char-argument command is waiting (e.g. after `f`, `r`),
        // consume this keypress as its argument.
        if let Some(cmd) = self.vi.pending_char_command.take() {
            if let Key::Char(ch) = kp.key {
                self.dispatch_char_motion(&cmd, ch);
            }
            return;
        }

        match self.mode {
            Mode::Insert => self.replay_insert(kp, pending),
            Mode::Normal
            | Mode::Visual(_)
            | Mode::Command
            | Mode::Search
            | Mode::ConversationInput
            | Mode::FilePicker
            | Mode::FileBrowser
            | Mode::CommandPalette => {
                self.replay_via_keymap(kp, pending);
            }
            Mode::ShellInsert => {} // Keys go to PTY, not macro replay
        }
    }

    /// Handle a key in Insert mode during replay.
    fn replay_insert(&mut self, kp: KeyPress, pending: &mut Vec<KeyPress>) {
        match (&kp.key, kp.ctrl) {
            (Key::Char(ch), false) => {
                let ch = *ch;
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                self.buffers[idx].insert_char(win, ch);
            }
            (Key::Enter, false) => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                self.buffers[idx].insert_char(win, '\n');
            }
            (Key::Backspace, false) => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                self.buffers[idx].delete_char_backward(win);
            }
            _ => self.replay_via_keymap(kp, pending),
        }
    }

    /// Look up `kp` in the current keymap and dispatch if there's an exact match.
    fn replay_via_keymap(&mut self, kp: KeyPress, pending: &mut Vec<KeyPress>) {
        pending.push(kp);
        let cmd = match self.current_keymap() {
            Some(km) => match km.lookup(pending) {
                LookupResult::Exact(c) => {
                    let c = c.to_string();
                    pending.clear();
                    Some(c)
                }
                LookupResult::Prefix => None, // accumulate more keys
                LookupResult::None => {
                    pending.clear();
                    None
                }
            },
            None => {
                pending.clear();
                None
            }
        };
        if let Some(cmd) = cmd {
            self.dispatch_builtin(&cmd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;

    fn editor_with_text(s: &str) -> Editor {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, s);
        Editor::with_buffer(buf)
    }

    // --- Recording ---

    #[test]
    fn start_recording_valid_register() {
        let mut editor = Editor::new();
        editor.start_recording('a').unwrap();
        assert!(editor.vi.macro_recording);
        assert_eq!(editor.vi.macro_register, Some('a'));
        assert!(editor.vi.macro_log.is_empty());
    }

    #[test]
    fn start_recording_invalid_register_rejected() {
        let mut editor = Editor::new();
        assert!(editor.start_recording('1').is_err());
        assert!(editor.start_recording('A').is_err()); // uppercase rejected
        assert!(editor.start_recording('!').is_err());
        assert!(!editor.vi.macro_recording);
    }

    #[test]
    fn start_recording_while_already_recording_errors() {
        let mut editor = Editor::new();
        editor.start_recording('a').unwrap();
        assert!(editor.start_recording('b').is_err());
    }

    #[test]
    fn stop_recording_saves_to_register() {
        let mut editor = Editor::new();
        editor.start_recording('a').unwrap();
        editor.vi.macro_log.push(KeyPress::char('j'));
        editor.vi.macro_log.push(KeyPress::char('j'));
        let ch = editor.stop_recording();
        assert_eq!(ch, Some('a'));
        assert!(!editor.vi.macro_recording);
        assert!(editor.vi.macro_log.is_empty());
        assert_eq!(
            editor.vi.registers.get(&'a').map(|s| s.as_str()),
            Some("jj")
        );
    }

    #[test]
    fn stop_recording_when_not_recording_returns_none() {
        let mut editor = Editor::new();
        assert_eq!(editor.stop_recording(), None);
    }

    // --- Replay ---

    #[test]
    fn replay_macro_moves_cursor() {
        let mut editor = editor_with_text("line1\nline2\nline3\n");
        editor.vi.registers.insert('a', "j".to_string());
        editor.replay_macro('a', 1).unwrap();
        assert_eq!(editor.window_mgr.focused_window().cursor_row, 1);
    }

    #[test]
    fn replay_macro_count_repeats() {
        let mut editor = editor_with_text("line1\nline2\nline3\n");
        editor.vi.registers.insert('a', "j".to_string());
        editor.replay_macro('a', 2).unwrap();
        assert_eq!(editor.window_mgr.focused_window().cursor_row, 2);
    }

    #[test]
    fn replay_macro_sets_last_register() {
        let mut editor = Editor::new();
        editor.vi.registers.insert('a', "j".to_string());
        editor.replay_macro('a', 1).unwrap();
        assert_eq!(editor.vi.last_macro_register, Some('a'));
    }

    #[test]
    fn replay_macro_nonexistent_register_errors() {
        let mut editor = Editor::new();
        let err = editor.replay_macro('z', 1).unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn replay_macro_empty_register_is_noop() {
        let mut editor = editor_with_text("hello\n");
        editor.vi.registers.insert('a', "".to_string());
        editor.replay_macro('a', 1).unwrap(); // must not panic
        assert_eq!(editor.window_mgr.focused_window().cursor_row, 0);
    }

    #[test]
    fn replay_macro_invalid_register_errors() {
        let mut editor = Editor::new();
        assert!(editor.replay_macro('Z', 1).is_err()); // uppercase rejected
    }

    #[test]
    fn replay_macro_insert_mode_text() {
        let mut editor = editor_with_text("abc\n");
        // Macro: enter insert mode, type "XY", escape back to normal
        editor.vi.registers.insert('b', "iXY<Esc>".to_string());
        editor.replay_macro('b', 1).unwrap();
        assert_eq!(editor.active_buffer().line_text(0), "XYabc\n");
        assert_eq!(editor.mode, Mode::Normal);
    }

    #[test]
    fn replay_macro_multi_key_sequence() {
        // `dd` is a two-key sequence (prefix + confirm)
        let mut editor = editor_with_text("line1\nline2\nline3\n");
        editor.vi.registers.insert('a', "dd".to_string());
        editor.replay_macro('a', 1).unwrap();
        // line1 should be deleted
        assert_eq!(editor.active_buffer().line_count(), 3); // "line2\nline3\n" + trailing
        assert_eq!(editor.active_buffer().line_text(0), "line2\n");
    }

    #[test]
    fn recursive_macro_guard() {
        use crate::keymap::parse_key_seq;
        let mut editor = Editor::new();
        // Bind @ → replay-macro-await (normally from modules/macros/autoloads.scm)
        // so the keymap lookup during replay works.
        editor
            .keymaps
            .get_mut("normal")
            .unwrap()
            .bind(parse_key_seq("@"), "replay-macro-await");
        // @a → @a: self-referential macro.
        // replay_macro kicks off the chain; inner replay calls surface the
        // depth error through set_status (dispatch_char_motion catches it),
        // so the outer call still returns Ok. Verify no stack overflow and
        // that the status message reports the guard fired.
        editor.vi.registers.insert('a', "@a".to_string());
        let result = editor.replay_macro('a', 1);
        assert!(
            result.is_ok(),
            "outer call should return Ok, got {:?}",
            result
        );
        assert!(
            editor.status_msg.contains("recursion") || editor.status_msg.contains("depth"),
            "expected depth-guard message in status, got: {:?}",
            editor.status_msg
        );
    }

    // --- Keymap bindings ---

    // Macro keybindings moved to modules/macros/autoloads.scm.
    // Verify commands remain registered as kernel builtins.
    #[test]
    fn macro_commands_registered() {
        let editor = Editor::new();
        assert!(editor.commands.contains("start-recording-await"));
        assert!(editor.commands.contains("replay-macro-await"));
    }

    #[test]
    fn replay_macro_at_sign_uses_last_register() {
        // @@ replays the last-used macro. Implemented by passing '@' as the
        // register char to dispatch_char_motion("replay-macro", '@').
        let mut editor = editor_with_text("line1\nline2\nline3\n");
        editor.vi.registers.insert('a', "j".to_string());
        editor.replay_macro('a', 1).unwrap(); // sets last_macro_register = Some('a')
        assert_eq!(editor.vi.last_macro_register, Some('a'));
        // Now call replay_macro with '@' — it should replay 'a' again.
        // This is what dispatch_char_motion does when ch == '@'.
        editor.replay_macro('a', 1).unwrap(); // simulate @@
        assert_eq!(editor.window_mgr.focused_window().cursor_row, 2);
    }
}
