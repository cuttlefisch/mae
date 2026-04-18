use std::collections::{BTreeMap, HashMap, HashSet};

/// A single key press, independent of any terminal library.
/// The binary converts crossterm::event::KeyEvent → KeyPress.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct KeyPress {
    pub key: Key,
    pub ctrl: bool,
    pub alt: bool,
}

/// Abstract key code — no dependency on crossterm.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub enum Key {
    Char(char),
    Escape,
    Enter,
    Backspace,
    Tab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    F(u8),
}

impl KeyPress {
    pub fn char(ch: char) -> Self {
        KeyPress {
            key: Key::Char(ch),
            ctrl: false,
            alt: false,
        }
    }

    pub fn ctrl(ch: char) -> Self {
        KeyPress {
            key: Key::Char(ch),
            ctrl: true,
            alt: false,
        }
    }

    pub fn special(key: Key) -> Self {
        KeyPress {
            key,
            ctrl: false,
            alt: false,
        }
    }
}

/// Result of looking up a key sequence in a keymap.
#[derive(Debug, PartialEq)]
pub enum LookupResult<'a> {
    /// Full match — execute this command.
    Exact(&'a str),
    /// The sequence is a prefix of one or more bindings — wait for more keys.
    Prefix,
    /// No match and not a prefix of anything.
    None,
}

/// Entry for the which-key popup: the next key to press and what it does.
#[derive(Debug, Clone)]
pub struct WhichKeyEntry {
    pub key: KeyPress,
    pub label: String,
    pub is_group: bool,
}

/// A named keymap mapping key sequences to command names.
///
/// Emacs lesson: keymap.c uses alists for O(n) lookup on every keystroke.
/// We use HashMap for O(1) lookup. Prefixes are tracked in a HashSet for
/// efficient multi-key sequence handling (dd, gg, C-c C-v, etc.)
pub struct Keymap {
    pub name: String,
    bindings: HashMap<Vec<KeyPress>, String>,
    /// All proper prefixes of bound key sequences, for multi-key detection.
    prefixes: HashSet<Vec<KeyPress>>,
    /// Human-readable group labels for key prefixes (e.g. [SPC, b] → "+buffer").
    group_names: HashMap<Vec<KeyPress>, String>,
}

impl Keymap {
    pub fn new(name: impl Into<String>) -> Self {
        Keymap {
            name: name.into(),
            bindings: HashMap::new(),
            prefixes: HashSet::new(),
            group_names: HashMap::new(),
        }
    }

    /// Bind a key sequence to a command name.
    pub fn bind(&mut self, seq: Vec<KeyPress>, command: impl Into<String>) {
        // Register all proper prefixes
        for i in 1..seq.len() {
            self.prefixes.insert(seq[..i].to_vec());
        }
        self.bindings.insert(seq, command.into());
    }

    /// Look up a key sequence.
    ///
    /// If the sequence is both an exact match AND a prefix of longer bindings,
    /// Prefix wins — the dispatch layer must wait for more keys or a timeout.
    /// This is critical for vi-style operators: "d" must wait for "dd"/"dw"/etc.
    pub fn lookup(&self, seq: &[KeyPress]) -> LookupResult<'_> {
        let is_prefix = self.prefixes.contains(seq);
        if is_prefix {
            return LookupResult::Prefix;
        }
        if let Some(cmd) = self.bindings.get(seq) {
            return LookupResult::Exact(cmd);
        }
        LookupResult::None
    }

    /// Look up a key sequence, ignoring prefix status.
    /// Returns `Some(cmd)` if there's an exact binding, even if the
    /// sequence is also a prefix of longer bindings.
    pub fn exact_match(&self, seq: &[KeyPress]) -> Option<&str> {
        self.bindings.get(seq).map(|s| s.as_str())
    }

    /// Remove a binding.
    pub fn unbind(&mut self, seq: &[KeyPress]) {
        self.bindings.remove(seq);
        // Note: we don't clean up prefixes — they're harmless if stale
        // and rebuilding them would require scanning all remaining bindings.
    }

    /// Iterate over all bindings.
    pub fn bindings(&self) -> impl Iterator<Item = (&Vec<KeyPress>, &String)> {
        self.bindings.iter()
    }

    /// Set a human-readable label for a key prefix group.
    /// e.g. `set_group_name(&[Char(' '), Char('b')], "+buffer")`
    pub fn set_group_name(&mut self, prefix: Vec<KeyPress>, label: impl Into<String>) {
        self.group_names.insert(prefix, label.into());
    }

    /// Given a prefix sequence, return the immediate next-level keys with labels.
    /// Groups (prefixes leading to more keys) show the group name.
    /// Leaf commands show the command name.
    pub fn which_key_entries(
        &self,
        prefix: &[KeyPress],
        commands: &crate::commands::CommandRegistry,
    ) -> Vec<WhichKeyEntry> {
        // Use BTreeMap with a string key for deterministic ordering
        let mut seen: BTreeMap<String, WhichKeyEntry> = BTreeMap::new();
        let next_idx = prefix.len();

        for (seq, cmd_name) in &self.bindings {
            if seq.len() <= next_idx {
                continue;
            }
            if &seq[..next_idx] != prefix {
                continue;
            }
            let next_key = &seq[next_idx];
            let sort_key = format!("{:?}", next_key);

            if seen.contains_key(&sort_key) {
                continue;
            }

            let is_leaf = seq.len() == next_idx + 1;

            if is_leaf {
                let label = commands
                    .get(cmd_name)
                    .map(|c| c.doc.clone())
                    .unwrap_or_else(|| cmd_name.clone());
                seen.insert(
                    sort_key,
                    WhichKeyEntry {
                        key: next_key.clone(),
                        label,
                        is_group: false,
                    },
                );
            } else {
                // It's a group — look up group label or generate one
                let mut group_prefix = prefix.to_vec();
                group_prefix.push(next_key.clone());
                let label = self
                    .group_names
                    .get(&group_prefix)
                    .cloned()
                    .unwrap_or_else(|| "+...".to_string());
                seen.insert(
                    sort_key,
                    WhichKeyEntry {
                        key: next_key.clone(),
                        label,
                        is_group: true,
                    },
                );
            }
        }

        seen.into_values().collect()
    }
}

/// Parse a human-readable key string into a key sequence.
///
/// Formats:
/// - "j" → single char key
/// - "dd" → two char keys (multi-key sequence)
/// - "C-r" → ctrl+r
/// - "M-x" → alt+x
/// - "escape" → escape key
/// - "enter", "backspace", "tab", "up", "down", "left", "right" → special keys
pub fn parse_key_seq(s: &str) -> Vec<KeyPress> {
    let mut result = Vec::new();
    let mut chars = s.chars().peekable();

    while chars.peek().is_some() {
        // Handle <Token> bracketed syntax (e.g. <F1>, <Esc>, <C-x>)
        if chars.peek() == Some(&'<') {
            chars.next(); // consume '<'
            let token: String = chars.by_ref().take_while(|&c| c != '>').collect();
            if let Some(kp) = parse_macro_token(&token) {
                result.push(kp);
            }
            continue;
        }

        // Check for modifier prefix
        let next_two: String = chars.clone().take(2).collect();
        if next_two == "C-" {
            chars.next(); // C
            chars.next(); // -
            if let Some(ch) = chars.next() {
                result.push(KeyPress::ctrl(ch));
            }
            continue;
        }
        if next_two == "M-" {
            chars.next(); // M
            chars.next(); // -
            if let Some(ch) = chars.next() {
                result.push(KeyPress {
                    key: Key::Char(ch),
                    ctrl: false,
                    alt: true,
                });
            }
            continue;
        }

        // Check for named keys
        let remaining: String = chars.clone().collect();
        let lower = remaining.to_lowercase();
        if let Some((key, len)) = match_named_key(&lower) {
            result.push(KeyPress::special(key));
            for _ in 0..len {
                chars.next();
            }
            continue;
        }

        // Single character
        if let Some(ch) = chars.next() {
            result.push(KeyPress::char(ch));
        }
    }

    result
}

/// Parse a space-separated key sequence. Each whitespace-delimited token is
/// parsed independently by `parse_key_seq`, then concatenated.
///
/// "SPC b k" → [Char(' '), Char('b'), Char('k')]
/// "C-w v"   → [Ctrl-w, Char('v')]  (2 keys, not 3 — the space is a delimiter)
/// "dd"      → [Char('d'), Char('d')] (single token, no spaces)
pub fn parse_key_seq_spaced(s: &str) -> Vec<KeyPress> {
    s.split_whitespace().flat_map(parse_key_seq).collect()
}

/// Serialize one `KeyPress` to the macro string format.
/// Plain printable chars (except `<` and space) map to themselves.
/// Everything else uses `<Token>` notation.
pub fn serialize_keypress(kp: &KeyPress) -> String {
    match (&kp.key, kp.ctrl, kp.alt) {
        (Key::Char(ch), false, false) if *ch == ' ' => "<Space>".to_string(),
        (Key::Char(ch), false, false) if *ch == '<' => "<lt>".to_string(),
        (Key::Char(ch), false, false) => ch.to_string(),
        (Key::Char(ch), true, false) => format!("<C-{}>", ch),
        (Key::Char(ch), false, true) => format!("<M-{}>", ch),
        (Key::Char(ch), true, true) => format!("<C-M-{}>", ch),
        (Key::Escape, false, false) => "<Esc>".to_string(),
        (Key::Enter, false, false) => "<CR>".to_string(),
        (Key::Backspace, false, false) => "<BS>".to_string(),
        (Key::Tab, false, false) => "<Tab>".to_string(),
        (Key::Up, false, false) => "<Up>".to_string(),
        (Key::Down, false, false) => "<Down>".to_string(),
        (Key::Left, false, false) => "<Left>".to_string(),
        (Key::Right, false, false) => "<Right>".to_string(),
        (Key::Home, false, false) => "<Home>".to_string(),
        (Key::End, false, false) => "<End>".to_string(),
        (Key::PageUp, false, false) => "<PageUp>".to_string(),
        (Key::PageDown, false, false) => "<PageDown>".to_string(),
        (Key::Delete, false, false) => "<Del>".to_string(),
        (Key::F(n), false, false) => format!("<F{}>", n),
        // Special keys with modifiers (rare; emit as best-effort)
        (Key::Escape, true, _) => "<C-Esc>".to_string(),
        _ => String::new(),
    }
}

/// Serialize a key sequence (macro body) to a compact string.
pub fn serialize_macro(keys: &[KeyPress]) -> String {
    keys.iter().map(serialize_keypress).collect()
}

/// Deserialize a macro string back to a `Vec<KeyPress>`.
///
/// Format: bare printable chars plus `<Token>` bracketed specials.
/// Unknown tokens are silently skipped.
pub fn deserialize_macro(s: &str) -> Vec<KeyPress> {
    let mut result = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&ch) = chars.peek() {
        if ch == '<' {
            chars.next(); // consume '<'
            let token: String = chars.by_ref().take_while(|&c| c != '>').collect();
            if let Some(kp) = parse_macro_token(&token) {
                result.push(kp);
            }
        } else {
            chars.next();
            result.push(KeyPress::char(ch));
        }
    }
    result
}

fn parse_macro_token(token: &str) -> Option<KeyPress> {
    let lower = token.to_lowercase();

    // Modifier prefixes: C-M-, M-C-, C-, M-
    if let Some(rest) = lower
        .strip_prefix("c-m-")
        .or_else(|| lower.strip_prefix("m-c-"))
    {
        let ch = rest.chars().next()?;
        return Some(KeyPress {
            key: Key::Char(ch),
            ctrl: true,
            alt: true,
        });
    }
    if let Some(rest) = lower.strip_prefix("c-") {
        let ch = rest.chars().next()?;
        return Some(KeyPress {
            key: Key::Char(ch),
            ctrl: true,
            alt: false,
        });
    }
    if let Some(rest) = lower.strip_prefix("m-") {
        let ch = rest.chars().next()?;
        return Some(KeyPress {
            key: Key::Char(ch),
            ctrl: false,
            alt: true,
        });
    }

    // Named specials (case-insensitive)
    match lower.as_str() {
        "esc" | "escape" => Some(KeyPress::special(Key::Escape)),
        "cr" | "enter" | "return" => Some(KeyPress::special(Key::Enter)),
        "bs" | "backspace" => Some(KeyPress::special(Key::Backspace)),
        "tab" => Some(KeyPress::special(Key::Tab)),
        "up" => Some(KeyPress::special(Key::Up)),
        "down" => Some(KeyPress::special(Key::Down)),
        "left" => Some(KeyPress::special(Key::Left)),
        "right" => Some(KeyPress::special(Key::Right)),
        "home" => Some(KeyPress::special(Key::Home)),
        "end" => Some(KeyPress::special(Key::End)),
        "pageup" => Some(KeyPress::special(Key::PageUp)),
        "pagedown" => Some(KeyPress::special(Key::PageDown)),
        "del" | "delete" => Some(KeyPress::special(Key::Delete)),
        "space" => Some(KeyPress::char(' ')),
        "lt" => Some(KeyPress::char('<')),
        _ => {
            // F-keys: "f1", "f12"
            if let Some(n_str) = lower.strip_prefix('f') {
                if let Ok(n) = n_str.parse::<u8>() {
                    return Some(KeyPress::special(Key::F(n)));
                }
            }
            None
        }
    }
}

fn match_named_key(s: &str) -> Option<(Key, usize)> {
    // Only match named keys at the start of the string
    // SPC must come before longer names to avoid being shadowed
    if s.starts_with("spc") && (s.len() == 3 || !s.as_bytes()[3].is_ascii_alphabetic()) {
        return Some((Key::Char(' '), 3));
    }

    let named = [
        ("escape", Key::Escape),
        ("enter", Key::Enter),
        ("backspace", Key::Backspace),
        ("tab", Key::Tab),
        ("up", Key::Up),
        ("down", Key::Down),
        ("left", Key::Left),
        ("right", Key::Right),
        ("home", Key::Home),
        ("end", Key::End),
        ("pageup", Key::PageUp),
        ("pagedown", Key::PageDown),
        ("delete", Key::Delete),
    ];

    for (name, key) in &named {
        if s.starts_with(name) {
            // Make sure it's a complete word (not a prefix of something else)
            // For multi-char sequences like "dd", "escape" shouldn't match "e"
            if s.len() == name.len() || !s.as_bytes()[name.len()].is_ascii_alphabetic() {
                return Some((key.clone(), name.len()));
            }
        }
    }

    // F-keys
    if let Some(rest) = s.strip_prefix('f') {
        if let Some(end) = rest.find(|c: char| !c.is_ascii_digit()) {
            if let Ok(n) = rest[..end].parse::<u8>() {
                return Some((Key::F(n), 1 + end));
            }
        } else if let Ok(n) = rest.parse::<u8>() {
            return Some((Key::F(n), s.len()));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- KeyPress construction ---

    #[test]
    fn keypress_char() {
        let k = KeyPress::char('j');
        assert_eq!(k.key, Key::Char('j'));
        assert!(!k.ctrl);
        assert!(!k.alt);
    }

    #[test]
    fn keypress_ctrl() {
        let k = KeyPress::ctrl('r');
        assert_eq!(k.key, Key::Char('r'));
        assert!(k.ctrl);
    }

    // --- Key sequence parsing ---

    #[test]
    fn parse_single_char() {
        let seq = parse_key_seq("j");
        assert_eq!(seq.len(), 1);
        assert_eq!(seq[0], KeyPress::char('j'));
    }

    #[test]
    fn parse_multi_char() {
        let seq = parse_key_seq("dd");
        assert_eq!(seq.len(), 2);
        assert_eq!(seq[0], KeyPress::char('d'));
        assert_eq!(seq[1], KeyPress::char('d'));
    }

    #[test]
    fn parse_ctrl_modifier() {
        let seq = parse_key_seq("C-r");
        assert_eq!(seq.len(), 1);
        assert_eq!(seq[0], KeyPress::ctrl('r'));
    }

    #[test]
    fn parse_alt_modifier() {
        let seq = parse_key_seq("M-x");
        assert_eq!(seq.len(), 1);
        assert!(seq[0].alt);
        assert_eq!(seq[0].key, Key::Char('x'));
    }

    #[test]
    fn parse_escape() {
        let seq = parse_key_seq("escape");
        assert_eq!(seq.len(), 1);
        assert_eq!(seq[0].key, Key::Escape);
    }

    #[test]
    fn parse_enter() {
        let seq = parse_key_seq("enter");
        assert_eq!(seq.len(), 1);
        assert_eq!(seq[0].key, Key::Enter);
    }

    #[test]
    fn parse_gg() {
        let seq = parse_key_seq("gg");
        assert_eq!(seq.len(), 2);
        assert_eq!(seq[0], KeyPress::char('g'));
        assert_eq!(seq[1], KeyPress::char('g'));
    }

    // --- Keymap binding and lookup ---

    #[test]
    fn bind_and_lookup_single_key() {
        let mut km = Keymap::new("normal");
        km.bind(vec![KeyPress::char('j')], "move-down");
        assert_eq!(
            km.lookup(&[KeyPress::char('j')]),
            LookupResult::Exact("move-down")
        );
    }

    #[test]
    fn lookup_missing_key_returns_none() {
        let km = Keymap::new("normal");
        assert_eq!(km.lookup(&[KeyPress::char('z')]), LookupResult::None);
    }

    #[test]
    fn multi_key_sequence_with_prefix() {
        let mut km = Keymap::new("normal");
        km.bind(
            vec![KeyPress::char('d'), KeyPress::char('d')],
            "delete-line",
        );
        // Single 'd' is a prefix
        assert_eq!(km.lookup(&[KeyPress::char('d')]), LookupResult::Prefix);
        // 'dd' is an exact match
        assert_eq!(
            km.lookup(&[KeyPress::char('d'), KeyPress::char('d')]),
            LookupResult::Exact("delete-line")
        );
    }

    #[test]
    fn prefix_wins_over_exact_when_longer_bindings_exist() {
        let mut km = Keymap::new("normal");
        km.bind(vec![KeyPress::char('d')], "delete-char");
        km.bind(
            vec![KeyPress::char('d'), KeyPress::char('d')],
            "delete-line",
        );
        // When both 'd' (exact) and 'dd' (longer) are bound,
        // Prefix wins — dispatch must wait for more keys.
        // This is critical for vi-style operators.
        assert_eq!(km.lookup(&[KeyPress::char('d')]), LookupResult::Prefix);
        assert_eq!(
            km.lookup(&[KeyPress::char('d'), KeyPress::char('d')]),
            LookupResult::Exact("delete-line")
        );
    }

    #[test]
    fn ctrl_key_binding() {
        let mut km = Keymap::new("normal");
        km.bind(vec![KeyPress::ctrl('r')], "redo");
        assert_eq!(
            km.lookup(&[KeyPress::ctrl('r')]),
            LookupResult::Exact("redo")
        );
        // Plain 'r' should not match
        assert_eq!(km.lookup(&[KeyPress::char('r')]), LookupResult::None);
    }

    #[test]
    fn unbind_removes_binding() {
        let mut km = Keymap::new("normal");
        km.bind(vec![KeyPress::char('j')], "move-down");
        km.unbind(&[KeyPress::char('j')]);
        assert_eq!(km.lookup(&[KeyPress::char('j')]), LookupResult::None);
    }

    #[test]
    fn parse_and_bind_integration() {
        let mut km = Keymap::new("normal");
        km.bind(parse_key_seq("C-r"), "redo");
        km.bind(parse_key_seq("gg"), "move-to-first-line");
        km.bind(parse_key_seq("G"), "move-to-last-line");

        assert_eq!(
            km.lookup(&parse_key_seq("C-r")),
            LookupResult::Exact("redo")
        );
        assert_eq!(
            km.lookup(&parse_key_seq("gg")),
            LookupResult::Exact("move-to-first-line")
        );
        assert_eq!(km.lookup(&parse_key_seq("g")), LookupResult::Prefix);
        assert_eq!(
            km.lookup(&parse_key_seq("G")),
            LookupResult::Exact("move-to-last-line")
        );
    }

    // --- parse_key_seq_spaced ---

    #[test]
    fn parse_key_seq_spaced_ctrl_w_v() {
        let seq = parse_key_seq_spaced("C-w v");
        assert_eq!(seq.len(), 2);
        assert_eq!(seq[0], KeyPress::ctrl('w'));
        assert_eq!(seq[1], KeyPress::char('v'));
    }

    #[test]
    fn parse_key_seq_spaced_leader() {
        let seq = parse_key_seq_spaced("SPC b k");
        assert_eq!(seq.len(), 3);
        assert_eq!(seq[0], KeyPress::char(' '));
        assert_eq!(seq[1], KeyPress::char('b'));
        assert_eq!(seq[2], KeyPress::char('k'));
    }

    #[test]
    fn parse_spc_named_key() {
        let seq = parse_key_seq("SPC");
        assert_eq!(seq.len(), 1);
        assert_eq!(seq[0].key, Key::Char(' '));
    }

    #[test]
    fn parse_key_seq_spaced_single_token() {
        let seq = parse_key_seq_spaced("dd");
        assert_eq!(seq.len(), 2);
        assert_eq!(seq[0], KeyPress::char('d'));
        assert_eq!(seq[1], KeyPress::char('d'));
    }

    // --- which_key_entries ---

    #[test]
    fn which_key_entries_empty_prefix() {
        use crate::commands::CommandRegistry;

        let mut km = Keymap::new("normal");
        km.bind(parse_key_seq_spaced("SPC b s"), "save");
        km.bind(parse_key_seq_spaced("SPC w v"), "split-vertical");
        km.set_group_name(parse_key_seq_spaced("SPC b"), "+buffer");
        km.set_group_name(parse_key_seq_spaced("SPC w"), "+window");

        let reg = CommandRegistry::with_builtins();
        let entries = km.which_key_entries(&parse_key_seq("SPC"), &reg);

        // Should have 'b' and 'w' groups
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.is_group));
        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"+buffer"));
        assert!(labels.contains(&"+window"));
    }

    #[test]
    fn which_key_entries_buffer_prefix() {
        use crate::commands::CommandRegistry;

        let mut km = Keymap::new("normal");
        km.bind(parse_key_seq_spaced("SPC b s"), "save");
        km.bind(parse_key_seq_spaced("SPC b d"), "kill-buffer");

        let mut reg = CommandRegistry::with_builtins();
        reg.register_builtin("kill-buffer", "Close current buffer");

        let prefix = parse_key_seq_spaced("SPC b");
        let entries = km.which_key_entries(&prefix, &reg);

        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| !e.is_group));
        // Should have doc strings as labels
        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"Save current buffer"));
        assert!(labels.contains(&"Close current buffer"));
    }

    #[test]
    fn lookup_prefix_only_returns_prefix() {
        let mut km = Keymap::new("normal");
        km.bind(parse_key_seq("dd"), "delete-line");
        km.bind(parse_key_seq("gg"), "move-to-first-line");

        // 'd' is prefix, 'g' is prefix
        assert_eq!(km.lookup(&parse_key_seq("d")), LookupResult::Prefix);
        assert_eq!(km.lookup(&parse_key_seq("g")), LookupResult::Prefix);
        // 'x' is nothing
        assert_eq!(km.lookup(&parse_key_seq("x")), LookupResult::None);
    }

    // --- Macro serialization ---

    #[test]
    fn serialize_plain_char() {
        assert_eq!(serialize_keypress(&KeyPress::char('j')), "j");
        assert_eq!(serialize_keypress(&KeyPress::char('$')), "$");
        assert_eq!(serialize_keypress(&KeyPress::char('0')), "0");
    }

    #[test]
    fn serialize_space() {
        assert_eq!(serialize_keypress(&KeyPress::char(' ')), "<Space>");
    }

    #[test]
    fn serialize_lt() {
        assert_eq!(serialize_keypress(&KeyPress::char('<')), "<lt>");
    }

    #[test]
    fn serialize_special_keys() {
        assert_eq!(serialize_keypress(&KeyPress::special(Key::Escape)), "<Esc>");
        assert_eq!(serialize_keypress(&KeyPress::special(Key::Enter)), "<CR>");
        assert_eq!(
            serialize_keypress(&KeyPress::special(Key::Backspace)),
            "<BS>"
        );
        assert_eq!(serialize_keypress(&KeyPress::special(Key::Tab)), "<Tab>");
        assert_eq!(serialize_keypress(&KeyPress::special(Key::F(1))), "<F1>");
        assert_eq!(serialize_keypress(&KeyPress::special(Key::F(12))), "<F12>");
    }

    #[test]
    fn serialize_ctrl() {
        assert_eq!(serialize_keypress(&KeyPress::ctrl('r')), "<C-r>");
    }

    #[test]
    fn serialize_alt() {
        let kp = KeyPress {
            key: Key::Char('x'),
            ctrl: false,
            alt: true,
        };
        assert_eq!(serialize_keypress(&kp), "<M-x>");
    }

    #[test]
    fn serialize_macro_sequence() {
        let keys = vec![
            KeyPress::char('d'),
            KeyPress::char('d'),
            KeyPress::char('j'),
            KeyPress::special(Key::Escape),
        ];
        assert_eq!(serialize_macro(&keys), "ddj<Esc>");
    }

    #[test]
    fn deserialize_plain_chars() {
        let keys = deserialize_macro("ddj");
        assert_eq!(
            keys,
            vec![
                KeyPress::char('d'),
                KeyPress::char('d'),
                KeyPress::char('j'),
            ]
        );
    }

    #[test]
    fn deserialize_special_tokens() {
        let keys = deserialize_macro("<Esc><CR><BS>");
        assert_eq!(keys[0].key, Key::Escape);
        assert_eq!(keys[1].key, Key::Enter);
        assert_eq!(keys[2].key, Key::Backspace);
    }

    #[test]
    fn deserialize_space_token() {
        let keys = deserialize_macro("<Space>");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, Key::Char(' '));
    }

    #[test]
    fn deserialize_ctrl_token() {
        let keys = deserialize_macro("<C-r>");
        assert_eq!(keys.len(), 1);
        assert!(keys[0].ctrl);
        assert_eq!(keys[0].key, Key::Char('r'));
    }

    #[test]
    fn serialize_deserialize_roundtrip() {
        let keys = vec![
            KeyPress::char('d'),
            KeyPress::char('d'),
            KeyPress::special(Key::Escape),
            KeyPress::ctrl('r'),
            KeyPress::char(' '),
            KeyPress::char('<'),
        ];
        let s = serialize_macro(&keys);
        let back = deserialize_macro(&s);
        assert_eq!(back, keys);
    }

    #[test]
    fn deserialize_unknown_token_skipped() {
        let keys = deserialize_macro("<bogus>j");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, Key::Char('j'));
    }

    // --- parse_key_seq bracket syntax ---

    #[test]
    fn parse_key_seq_bracket_f_keys() {
        let keys = parse_key_seq("<F1>");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, Key::F(1));

        let keys = parse_key_seq("<F12>");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, Key::F(12));
    }

    #[test]
    fn parse_key_seq_bracket_specials() {
        let keys = parse_key_seq("<Esc>");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, Key::Escape);

        let keys = parse_key_seq("<CR>");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, Key::Enter);
    }

    #[test]
    fn parse_key_seq_bracket_ctrl() {
        let keys = parse_key_seq("<C-x>");
        assert_eq!(keys.len(), 1);
        assert!(keys[0].ctrl);
        assert_eq!(keys[0].key, Key::Char('x'));
    }

    #[test]
    fn parse_key_seq_spaced_bracket_in_shell_keymap() {
        // This is how define-key from Scheme passes "<F1>"
        let keys = parse_key_seq_spaced("<F1>");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, Key::F(1));
    }
}
