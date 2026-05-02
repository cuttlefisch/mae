//! Ex-command tokenizer — structured parsing for write/quit compound commands
//! and `:set` option syntax.
//!
//! Instead of hardcoding every combination (`wq`, `wqa!`, `xa`, etc.) as
//! separate match arms, this module parses the verb+modifier grammar into
//! structured intent that `execute_command` dispatches uniformly.

/// A single write/quit action parsed from compound ex-commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExWriteQuit {
    /// Write buffer(s) to disk.
    Write { all: bool },
    /// Write buffer(s) only if modified (`:x` semantics).
    WriteIfModified { all: bool },
    /// Quit buffer(s).
    Quit { all: bool, force: bool },
}

/// Try to parse a write/quit compound command.
///
/// Grammar:
/// - Verbs: `w` (write), `q` (quit), `x` (write-if-modified + quit)
/// - Modifier `a`: applies "all" to preceding verb
/// - `!` must be terminal, applies force to quit
///
/// Returns `None` if the command doesn't match the w/q/x grammar.
pub fn parse_write_quit(cmd: &str) -> Option<Vec<ExWriteQuit>> {
    // Only match pure w/q/x/a/! sequences (no spaces, no other chars).
    if cmd.is_empty()
        || !cmd
            .chars()
            .all(|c| matches!(c, 'w' | 'q' | 'x' | 'a' | '!'))
    {
        return None;
    }

    let mut chars = cmd.chars().peekable();
    let mut actions = Vec::new();
    let mut has_write = false;
    let mut has_quit = false;
    let mut write_all = false;
    let mut quit_all = false;
    let mut write_if_modified = false;
    let mut force = false;

    while let Some(&ch) = chars.peek() {
        match ch {
            'w' => {
                if has_write {
                    return None; // duplicate verb
                }
                has_write = true;
                chars.next();
                if chars.peek() == Some(&'a') {
                    write_all = true;
                    chars.next();
                }
            }
            'q' => {
                if has_quit {
                    return None; // duplicate verb
                }
                has_quit = true;
                chars.next();
                if chars.peek() == Some(&'a') {
                    quit_all = true;
                    chars.next();
                }
            }
            'x' => {
                if has_write || has_quit {
                    return None; // x can't combine with explicit w/q
                }
                has_write = true;
                has_quit = true;
                write_if_modified = true;
                chars.next();
                if chars.peek() == Some(&'a') {
                    write_all = true;
                    quit_all = true;
                    chars.next();
                }
            }
            '!' => {
                chars.next();
                if chars.peek().is_some() {
                    return None; // ! must be terminal
                }
                force = true;
            }
            'a' => {
                return None; // bare 'a' without preceding verb
            }
            _ => return None,
        }
    }

    // Must have at least one verb.
    if !has_write && !has_quit {
        return None;
    }

    // In compound commands like `wqa`, the `a` binds to `q` grammatically,
    // but vim semantics propagate "all" to write as well.
    if has_write && quit_all && !write_all {
        write_all = true;
    }

    // Emit write action(s) first (write before quit).
    if has_write {
        if write_if_modified {
            actions.push(ExWriteQuit::WriteIfModified { all: write_all });
        } else {
            actions.push(ExWriteQuit::Write { all: write_all });
        }
    }

    if has_quit {
        actions.push(ExWriteQuit::Quit {
            all: quit_all,
            force,
        });
    }

    Some(actions)
}

/// Parsed `:set` command action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetAction {
    /// `:set option?` — query current value.
    Query(String),
    /// `:set option value` — assign a value.
    Assign(String, String),
    /// `:set option!` — toggle a boolean.
    Toggle(String),
    /// `:set option` — enable a boolean.
    Enable(String),
    /// `:set nooption` — disable a boolean (strip `no` prefix).
    Disable(String),
}

/// Parse `:set` arguments into a structured action.
pub fn parse_set_args(args: &str) -> SetAction {
    let args = args.trim();

    // `:set option?` — query
    if let Some(name) = args.strip_suffix('?') {
        return SetAction::Query(name.to_string());
    }

    // Split into option name and optional value.
    // Support quoted values: `:set show_break "↪ "`
    let (name_part, value_part) = split_set_args(args);

    if let Some(value) = value_part {
        return SetAction::Assign(name_part.to_string(), value);
    }

    // `:set option!` — toggle
    if let Some(name) = name_part.strip_suffix('!') {
        return SetAction::Toggle(name.to_string());
    }

    // `:set nooption` — disable (only if len > 2 and starts with "no")
    if name_part.len() > 2 && name_part.starts_with("no") {
        let stripped = &name_part[2..];
        return SetAction::Disable(stripped.to_string());
    }

    // `:set option` — enable (for bools) or query (handled by caller)
    SetAction::Enable(name_part.to_string())
}

/// Split `:set` args into (name, optional_value), supporting quoted values.
fn split_set_args(args: &str) -> (&str, Option<String>) {
    // Find first space that's not inside quotes.
    let bytes = args.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i] != b' ' {
        i += 1;
    }

    if i >= bytes.len() {
        return (args, None);
    }

    let name = &args[..i];
    let rest = args[i + 1..].trim();

    // Handle quoted values.
    if rest.starts_with('"') && rest.ends_with('"') && rest.len() >= 2 {
        let unquoted = &rest[1..rest.len() - 1];
        return (name, Some(unquoted.to_string()));
    }

    (name, Some(rest.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- write/quit tokenizer tests ---

    #[test]
    fn parse_w() {
        assert_eq!(
            parse_write_quit("w"),
            Some(vec![ExWriteQuit::Write { all: false }])
        );
    }

    #[test]
    fn parse_wa() {
        assert_eq!(
            parse_write_quit("wa"),
            Some(vec![ExWriteQuit::Write { all: true }])
        );
    }

    #[test]
    fn parse_q() {
        assert_eq!(
            parse_write_quit("q"),
            Some(vec![ExWriteQuit::Quit {
                all: false,
                force: false,
            }])
        );
    }

    #[test]
    fn parse_q_force() {
        assert_eq!(
            parse_write_quit("q!"),
            Some(vec![ExWriteQuit::Quit {
                all: false,
                force: true,
            }])
        );
    }

    #[test]
    fn parse_qa() {
        assert_eq!(
            parse_write_quit("qa"),
            Some(vec![ExWriteQuit::Quit {
                all: true,
                force: false,
            }])
        );
    }

    #[test]
    fn parse_qa_force() {
        assert_eq!(
            parse_write_quit("qa!"),
            Some(vec![ExWriteQuit::Quit {
                all: true,
                force: true,
            }])
        );
    }

    #[test]
    fn parse_wq() {
        assert_eq!(
            parse_write_quit("wq"),
            Some(vec![
                ExWriteQuit::Write { all: false },
                ExWriteQuit::Quit {
                    all: false,
                    force: false,
                },
            ])
        );
    }

    #[test]
    fn parse_wq_force() {
        assert_eq!(
            parse_write_quit("wq!"),
            Some(vec![
                ExWriteQuit::Write { all: false },
                ExWriteQuit::Quit {
                    all: false,
                    force: true,
                },
            ])
        );
    }

    #[test]
    fn parse_wqa() {
        assert_eq!(
            parse_write_quit("wqa"),
            Some(vec![
                ExWriteQuit::Write { all: true },
                ExWriteQuit::Quit {
                    all: true,
                    force: false,
                },
            ])
        );
    }

    #[test]
    fn parse_wqa_force() {
        assert_eq!(
            parse_write_quit("wqa!"),
            Some(vec![
                ExWriteQuit::Write { all: true },
                ExWriteQuit::Quit {
                    all: true,
                    force: true,
                },
            ])
        );
    }

    #[test]
    fn parse_x() {
        assert_eq!(
            parse_write_quit("x"),
            Some(vec![
                ExWriteQuit::WriteIfModified { all: false },
                ExWriteQuit::Quit {
                    all: false,
                    force: false,
                },
            ])
        );
    }

    #[test]
    fn parse_xa() {
        assert_eq!(
            parse_write_quit("xa"),
            Some(vec![
                ExWriteQuit::WriteIfModified { all: true },
                ExWriteQuit::Quit {
                    all: true,
                    force: false,
                },
            ])
        );
    }

    #[test]
    fn parse_xa_force() {
        assert_eq!(
            parse_write_quit("xa!"),
            Some(vec![
                ExWriteQuit::WriteIfModified { all: true },
                ExWriteQuit::Quit {
                    all: true,
                    force: true,
                },
            ])
        );
    }

    #[test]
    fn parse_invalid_bare_a() {
        assert_eq!(parse_write_quit("a"), None);
    }

    #[test]
    fn parse_invalid_bang_not_terminal() {
        assert_eq!(parse_write_quit("!q"), None);
    }

    #[test]
    fn parse_invalid_unknown_char() {
        assert_eq!(parse_write_quit("wz"), None);
    }

    #[test]
    fn parse_non_wq_commands() {
        assert_eq!(parse_write_quit("e"), None);
        assert_eq!(parse_write_quit("set"), None);
        assert_eq!(parse_write_quit("help"), None);
    }

    // --- set parser tests ---

    #[test]
    fn set_query() {
        assert_eq!(
            parse_set_args("line_numbers?"),
            SetAction::Query("line_numbers".into())
        );
    }

    #[test]
    fn set_assign() {
        assert_eq!(
            parse_set_args("font_size 18.0"),
            SetAction::Assign("font_size".into(), "18.0".into())
        );
    }

    #[test]
    fn set_assign_quoted() {
        assert_eq!(
            parse_set_args(r#"show_break "↪ ""#),
            SetAction::Assign("show_break".into(), "↪ ".into())
        );
    }

    #[test]
    fn set_toggle() {
        assert_eq!(
            parse_set_args("line_numbers!"),
            SetAction::Toggle("line_numbers".into())
        );
    }

    #[test]
    fn set_disable_no_prefix() {
        assert_eq!(
            parse_set_args("nonumber"),
            SetAction::Disable("number".into())
        );
    }

    #[test]
    fn set_enable() {
        assert_eq!(
            parse_set_args("line_numbers"),
            SetAction::Enable("line_numbers".into())
        );
    }

    #[test]
    fn set_short_no_not_treated_as_disable() {
        // "no" alone is too short to be a disable
        assert_eq!(parse_set_args("no"), SetAction::Enable("no".into()));
    }
}
