//! Vim-style named register operations (*Practical Vim* ch. 10).
//!
//! Vi exposes roughly 30 registers: the unnamed `"`, the numbered
//! `"0`–`"9`, the named `"a`–`"z` (with `"A`–`"Z` appending), the
//! black-hole `"_`, and the system clipboards `"+` / `"*`. This
//! module centralizes the side-effects so every yank / delete / paste
//! site goes through a single chokepoint instead of the previous
//! scatter of `registers.insert('"', …)` calls.
//!
//! Semantics reference: *Practical Vim* tips 60–62 and `:help
//! registers`.
//!
//! - `save_yank`: writes to `"0` (yank history) and the unnamed `"`;
//!   also writes to [`Editor::active_register`] if the user pressed
//!   `"x` first. `"_` discards.
//! - `save_delete`: same as `save_yank` but *skips* `"0` — deletions
//!   don't pollute the yank history.
//! - `paste_text`: reads from [`Editor::active_register`] if set (and
//!   consumes it), otherwise from `"`. `"+` / `"*` shell out to the
//!   system clipboard.

use super::Editor;

impl Editor {
    /// Route a yanked string to the appropriate registers.
    ///
    /// Always populates `"0` (unless the active register is `"_`). If
    /// the user pre-selected a register via `"x`, that register is also
    /// updated — uppercase letters append, lowercase replace. The
    /// unnamed `"` always mirrors the most recent yank/delete so `p`
    /// keeps working without an explicit register.
    pub(crate) fn save_yank(&mut self, text: String) {
        let target = self.active_register.take();
        if target == Some('_') {
            // Black-hole: don't even touch "" or "0.
            return;
        }
        // "0 always holds the last yank.
        self.registers.insert('0', text.clone());
        if let Some(ch) = target {
            self.write_named_register(ch, &text);
        }
        // Sync to system clipboard unless clipboard=internal.
        if self.clipboard != "internal" {
            let _ = crate::clipboard::copy(&text);
        }
        // Unnamed register mirrors the yank.
        self.registers.insert('"', text);
    }

    /// Route a deleted string to the appropriate registers.
    ///
    /// Unlike [`save_yank`] this does NOT populate `"0` — in vim, `"0`
    /// is reserved for the most recent *yank*, so you can still paste
    /// the last yank after a delete clobbered `""`.
    pub(crate) fn save_delete(&mut self, text: String) {
        let target = self.active_register.take();
        if target == Some('_') {
            return;
        }
        if let Some(ch) = target {
            self.write_named_register(ch, &text);
        }
        // Sync to system clipboard unless clipboard=internal.
        if self.clipboard != "internal" {
            let _ = crate::clipboard::copy(&text);
        }
        self.registers.insert('"', text);
    }

    /// Shared plumbing for named-register writes: uppercase = append,
    /// `+`/`*` = system clipboard + local mirror, lowercase = replace.
    pub(super) fn write_named_register(&mut self, ch: char, text: &str) {
        if matches!(ch, '+' | '*') {
            if let Err(e) = crate::clipboard::copy(text) {
                self.set_status(format!("Clipboard copy failed: {}", e));
            }
            self.registers.insert(ch, text.to_string());
            return;
        }
        if ch.is_ascii_uppercase() {
            let lower = ch.to_ascii_lowercase();
            let entry = self.registers.entry(lower).or_default();
            entry.push_str(text);
            return;
        }
        self.registers.insert(ch, text.to_string());
    }

    /// Read text for paste. Consumes [`Editor::active_register`] if
    /// set. Falls back to `"`. `"+`/`"*` query the system clipboard.
    pub(crate) fn paste_text(&mut self) -> Option<String> {
        let target = self.active_register.take();
        match target {
            Some('_') => None,
            Some(ch @ ('+' | '*')) => {
                // Prefer the live clipboard; fall back to the last
                // locally-mirrored value if the shell-out failed (eg.
                // no xclip installed).
                crate::clipboard::paste()
                    .ok()
                    .or_else(|| self.registers.get(&ch).cloned())
            }
            Some(ch) => {
                let lower = ch.to_ascii_lowercase();
                self.registers.get(&lower).cloned()
            }
            None => {
                // clipboard=unnamedplus: try system clipboard first, fall
                // back to the unnamed register if the shell-out fails.
                if self.clipboard == "unnamedplus" {
                    if let Ok(text) = crate::clipboard::paste() {
                        if !text.is_empty() {
                            return Some(text);
                        }
                    }
                }
                self.registers.get(&'"').cloned()
            }
        }
    }

    /// Insert mode `Ctrl-R <reg>` — paste the named register's contents
    /// at the cursor. `+` / `*` read the live system clipboard. Unknown
    /// registers are no-ops (status bar reports). Does NOT clobber
    /// `active_register` (Ctrl-R is its own separate pipe).
    pub fn insert_from_register(&mut self, ch: char) {
        let text = match ch {
            '+' | '*' => crate::clipboard::paste()
                .ok()
                .or_else(|| self.registers.get(&ch).cloned()),
            other => {
                let key = other.to_ascii_lowercase();
                self.registers.get(&key).cloned()
            }
        };
        let Some(text) = text else {
            self.set_status(format!("Register \"{} is empty", ch));
            return;
        };
        let idx = self.active_buffer_idx();
        let win = self.window_mgr.focused_window_mut();
        let offset = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
        self.buffers[idx].insert_text_at(offset, &text);
        // Advance cursor past the inserted text.
        let end = offset + text.chars().count();
        let rope = self.buffers[idx].rope();
        let new_row = rope.char_to_line(end);
        let line_start = rope.line_to_char(new_row);
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = new_row;
        win.cursor_col = end - line_start;
    }

    /// `:reg` / `:registers` — render all non-empty registers into the
    /// `*Registers*` scratch buffer. Mirrors the `:jumps` / `:changes`
    /// convention: one entry per line, leading column identifies the
    /// register name.
    pub fn show_registers_buffer(&mut self) {
        let mut body = String::from("*Registers*\n\n");
        // Deterministic ordering: unnamed, numbered, named, misc.
        let order: Vec<char> = {
            let mut v: Vec<char> = vec!['"', '0'];
            for d in '1'..='9' {
                v.push(d);
            }
            for a in 'a'..='z' {
                v.push(a);
            }
            v.extend(['+', '*', '_']);
            // Append any registers we might not have predicted.
            for &k in self.registers.keys() {
                if !v.contains(&k) {
                    v.push(k);
                }
            }
            v
        };
        let mut any = false;
        for ch in order {
            if let Some(text) = self.registers.get(&ch) {
                if text.is_empty() {
                    continue;
                }
                any = true;
                // Render newlines as `^J` so the table stays one-line-per-entry.
                let display: String = text
                    .chars()
                    .map(|c| match c {
                        '\n' => '\u{21B5}', // ↵
                        '\t' => '\u{21E5}', // ⇥
                        c => c,
                    })
                    .collect();
                body.push_str(&format!("\"{}  {}\n", ch, display));
            }
        }
        if !any {
            body.push_str("(all registers empty)\n");
        }
        let existing = self.buffers.iter().position(|b| b.name == "*Registers*");
        let idx = if let Some(i) = existing {
            self.buffers[i].replace_contents(&body);
            i
        } else {
            let mut buf = crate::buffer::Buffer::new();
            buf.replace_contents(&body);
            buf.name = "*Registers*".into();
            self.buffers.push(buf);
            self.buffers.len() - 1
        };
        self.display_buffer(idx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_yank_populates_unnamed_and_zero() {
        let mut ed = Editor::new();
        ed.save_yank("hello".to_string());
        assert_eq!(ed.registers.get(&'"').map(String::as_str), Some("hello"));
        assert_eq!(ed.registers.get(&'0').map(String::as_str), Some("hello"));
    }

    #[test]
    fn save_delete_populates_unnamed_but_not_zero() {
        let mut ed = Editor::new();
        ed.save_yank("original".to_string());
        ed.save_delete("trashed".to_string());
        assert_eq!(ed.registers.get(&'"').map(String::as_str), Some("trashed"));
        // "0 retains the prior yank — deletes don't clobber it.
        assert_eq!(ed.registers.get(&'0').map(String::as_str), Some("original"));
    }

    #[test]
    fn active_register_routes_yank() {
        let mut ed = Editor::new();
        ed.active_register = Some('a');
        ed.save_yank("to-a".to_string());
        assert_eq!(ed.registers.get(&'a').map(String::as_str), Some("to-a"));
        assert_eq!(ed.registers.get(&'"').map(String::as_str), Some("to-a"));
        // Active register consumed.
        assert_eq!(ed.active_register, None);
    }

    #[test]
    fn uppercase_register_appends() {
        let mut ed = Editor::new();
        ed.active_register = Some('a');
        ed.save_yank("first".to_string());
        ed.active_register = Some('A');
        ed.save_yank("-second".to_string());
        assert_eq!(
            ed.registers.get(&'a').map(String::as_str),
            Some("first-second")
        );
    }

    #[test]
    fn black_hole_discards_everything() {
        let mut ed = Editor::new();
        ed.save_yank("keep-me".to_string());
        ed.active_register = Some('_');
        ed.save_delete("bye".to_string());
        // Neither "" nor "0 were touched by the black-hole delete.
        assert_eq!(ed.registers.get(&'"').map(String::as_str), Some("keep-me"));
        assert_eq!(ed.registers.get(&'0').map(String::as_str), Some("keep-me"));
    }

    #[test]
    fn paste_text_reads_active_register() {
        let mut ed = Editor::new();
        ed.registers.insert('a', "from-a".to_string());
        ed.registers.insert('"', "from-unnamed".to_string());
        ed.active_register = Some('a');
        assert_eq!(ed.paste_text().as_deref(), Some("from-a"));
        assert_eq!(ed.active_register, None);
        // After consuming the active register, paste falls back to "".
        assert_eq!(ed.paste_text().as_deref(), Some("from-unnamed"));
    }

    #[test]
    fn paste_text_black_hole_returns_none() {
        let mut ed = Editor::new();
        ed.registers.insert('"', "x".into());
        ed.active_register = Some('_');
        assert_eq!(ed.paste_text(), None);
    }

    #[test]
    fn show_registers_buffer_lists_non_empty() {
        let mut ed = Editor::new();
        ed.registers.insert('"', "unnamed-text".into());
        ed.registers.insert('a', "alpha".into());
        // Empty register should not appear.
        ed.registers.insert('z', "".into());
        ed.show_registers_buffer();
        let buf = ed.buffers.iter().find(|b| b.name == "*Registers*").unwrap();
        let text = buf.text();
        assert!(text.contains("unnamed-text"));
        assert!(text.contains("alpha"));
        assert!(!text.contains("\"z"));
    }

    #[test]
    fn show_registers_buffer_empty_case() {
        let mut ed = Editor::new();
        ed.show_registers_buffer();
        let buf = ed.buffers.iter().find(|b| b.name == "*Registers*").unwrap();
        assert!(buf.text().contains("all registers empty"));
    }

    #[test]
    fn clipboard_internal_skips_system_clipboard() {
        let mut ed = Editor::new();
        ed.clipboard = "internal".to_string();
        // Should not panic or error — clipboard::copy is never called.
        ed.save_yank("internal-only".to_string());
        assert_eq!(
            ed.registers.get(&'"').map(String::as_str),
            Some("internal-only")
        );
        assert_eq!(
            ed.registers.get(&'0').map(String::as_str),
            Some("internal-only")
        );
    }

    #[test]
    fn clipboard_option_default_is_unnamed() {
        let ed = Editor::new();
        assert_eq!(ed.clipboard, "unnamed");
    }

    #[test]
    fn set_clipboard_option_validates() {
        let mut ed = Editor::new();
        assert!(ed.set_option("clipboard", "unnamedplus").is_ok());
        assert_eq!(ed.clipboard, "unnamedplus");
        assert!(ed.set_option("clipboard", "unnamed").is_ok());
        assert_eq!(ed.clipboard, "unnamed");
        assert!(ed.set_option("clipboard", "internal").is_ok());
        assert_eq!(ed.clipboard, "internal");
        assert!(ed.set_option("clipboard", "bogus").is_err());
    }

    #[test]
    fn paste_from_yank_register() {
        let mut ed = Editor::new();
        ed.registers.insert('0', "yanked".into());
        ed.registers.insert('"', "deleted".into());
        ed.dispatch_builtin("paste-from-yank");
        let text = ed.buffers[ed.active_buffer_idx()].text();
        assert!(
            text.contains("yanked"),
            "paste-from-yank should use register 0, got: {}",
            text
        );
    }

    #[test]
    fn spc_r_r_resolves_to_show_registers() {
        let ed = Editor::new();
        let normal = ed.keymaps.get("normal").unwrap();
        use crate::keymap::parse_key_seq_spaced;
        let result = normal.lookup(&parse_key_seq_spaced("SPC r r"));
        assert!(
            matches!(result, crate::keymap::LookupResult::Exact(cmd) if cmd == "show-registers"),
            "SPC r r should bind to show-registers"
        );
    }

    #[test]
    fn spc_r_y_resolves_to_paste_from_yank() {
        let ed = Editor::new();
        let normal = ed.keymaps.get("normal").unwrap();
        use crate::keymap::parse_key_seq_spaced;
        let result = normal.lookup(&parse_key_seq_spaced("SPC r y"));
        assert!(
            matches!(result, crate::keymap::LookupResult::Exact(cmd) if cmd == "paste-from-yank"),
            "SPC r y should bind to paste-from-yank"
        );
    }
}
