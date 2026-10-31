//! Parse configuration as DATA (ADR-112 D4): the parse is a function of the
//! text and these values, never of code, so every peer and the daemon parse the
//! same text the same way.

/// One TODO sequence: the keywords before `|` are "todo" states, those after
/// are "done" states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoSequence {
    pub todo: Vec<String>,
    pub done: Vec<String>,
}

impl TodoSequence {
    /// Parses an in-file `#+TODO:` value the way `org-set-regexps-and-options`
    /// does: words split on ASCII whitespace only (`split-string`'s default),
    /// `|` splits todo from done, a missing `|` makes the last word the only
    /// done state, and a keyword is its shortest prefix whose remainder is
    /// empty or a `(...)` fast-access suffix (`TODO(t)` → `TODO`, but
    /// `A(x)B` stays whole). An empty value gives an empty sequence, which
    /// still counts as a declaration.
    pub fn parse(value: &str) -> TodoSequence {
        let words: Vec<&str> = value
            .split(is_ascii_space)
            .filter(|w| !w.is_empty())
            .collect();
        let names = |ws: &[&str]| -> Vec<String> {
            ws.iter()
                .filter(|w| **w != "|")
                .map(|w| keyword_name(w))
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .collect()
        };
        match words.iter().position(|w| *w == "|") {
            Some(i) => TodoSequence {
                todo: names(&words[..i]),
                done: names(&words[i + 1..]),
            },
            None => {
                let mut todo = names(&words);
                let done = todo.pop().into_iter().collect();
                TodoSequence { todo, done }
            }
        }
    }

    fn keywords(&self) -> impl Iterator<Item = &str> {
        self.todo.iter().chain(&self.done).map(String::as_str)
    }
}

/// `split-string-default-separators`: `[ \f\t\n\r\v]`.
fn is_ascii_space(c: char) -> bool {
    matches!(c, ' ' | '\x0c' | '\t' | '\n' | '\r' | '\x0b')
}

/// `^\(.*?\)\(?:(\([^!@/]\)?.*?)\)?$`: the shortest prefix whose remainder
/// is empty or starts with `(` and ends with `)`.
fn keyword_name(word: &str) -> &str {
    word.char_indices()
        .map(|(i, _)| i)
        .find(|&i| {
            let rest = &word[i..];
            rest.len() >= 2 && rest.starts_with('(') && rest.ends_with(')')
        })
        .map_or(word, |i| &word[..i])
}

/// Everything that can change the SHAPE of the tree. Settings that only
/// change how a tree is read (export tags, link types) belong to later phases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseOptions {
    /// TODO sequences used when the file declares none of its own. In-file
    /// `#+TODO:`/`#+SEQ_TODO:`/`#+TYP_TODO:` lines replace these, as in Emacs.
    pub todo_sequences: Vec<TodoSequence>,
    /// Maximum nesting of greater elements. Contents nested deeper are kept
    /// as one opaque token with a [`crate::DiagnosticKind::DepthLimit`]
    /// diagnostic: the parse never recurses on input nesting. Values above
    /// [`MAX_DEPTH_CEILING`] are clamped to it.
    pub max_depth: usize,
}

/// The most `max_depth` can be: larger values are clamped to it. The parse
/// itself never recurses, but rowan drops and compares a tree recursively,
/// one stack frame per level. At this depth that fits in 512 KiB in a debug
/// build and 128 KiB in release (measured), so any tree mae-org builds can be
/// dropped on a 1 MiB stack (the WASM default) or a 2 MiB thread.
pub const MAX_DEPTH_CEILING: usize = 1024;

/// The default nesting cap. Real documents nest a few levels; this bounds the
/// tree depth (and so the stack depth of anything that walks it recursively,
/// including rowan's own drop) far below any thread's stack.
pub const DEFAULT_MAX_DEPTH: usize = 128;

impl Default for ParseOptions {
    /// `TODO | DONE`, Org's own default (`org-todo-keywords`), so an
    /// unconfigured parse agrees with `emacs -Q`.
    fn default() -> Self {
        ParseOptions {
            todo_sequences: vec![TodoSequence {
                todo: vec!["TODO".into()],
                done: vec!["DONE".into()],
            }],
            max_depth: DEFAULT_MAX_DEPTH,
        }
    }
}

/// The TODO keywords in force for one document, as a set: a headline's
/// candidate keyword is its first space-delimited word, so matching is one
/// hash lookup per headline whatever the number of keywords (scanning every
/// keyword per headline was quadratic; review finding).
#[derive(Debug, Clone, Default)]
pub(crate) struct TodoKeywords {
    words: std::collections::HashSet<String>,
}

impl TodoKeywords {
    /// `declared` is true when the file has any `#+TODO`-family keyword,
    /// even an empty one: then the file's sequences replace the configured
    /// ones entirely, as in Emacs.
    pub fn resolve(
        declared: bool,
        in_file: &[TodoSequence],
        configured: &[TodoSequence],
    ) -> TodoKeywords {
        let source = if declared { in_file } else { configured };
        TodoKeywords {
            words: source
                .iter()
                .flat_map(|s| s.keywords().map(str::to_string))
                .collect(),
        }
    }

    /// The keyword that starts `s` and is followed by a space or the end of
    /// the line (case-sensitive, as Org matches it). Keywords contain no
    /// ASCII space, so the only candidate is the text before the first space.
    pub fn match_at<'s>(&self, s: &'s str) -> Option<&'s str> {
        let word = &s[..s.find(' ').unwrap_or(s.len())];
        self.words.contains(word).then_some(word)
    }
}

/// Is `key` one of the keys that declare TODO sequences?
pub(crate) fn is_todo_key(key: &str) -> bool {
    ["TODO", "SEQ_TODO", "TYP_TODO"]
        .iter()
        .any(|k| key.eq_ignore_ascii_case(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(todo: &[&str], done: &[&str]) -> TodoSequence {
        TodoSequence {
            todo: todo.iter().map(|s| s.to_string()).collect(),
            done: done.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn todo_values_parse_like_org() {
        assert_eq!(TodoSequence::parse("A B | C"), seq(&["A", "B"], &["C"]));
        assert_eq!(
            TodoSequence::parse("TODO(t) WAIT(w@/!) | DONE(d!)"),
            seq(&["TODO", "WAIT"], &["DONE"])
        );
        // No bar: the last word is the done state.
        assert_eq!(
            TodoSequence::parse("NEXT LATER FINISHED"),
            seq(&["NEXT", "LATER"], &["FINISHED"])
        );
        // Empty declarations are empty sequences (and still declarations).
        assert_eq!(TodoSequence::parse("   "), seq(&[], &[]));
        assert_eq!(TodoSequence::parse("|"), seq(&[], &[]));
        assert_eq!(TodoSequence::parse("A |"), seq(&["A"], &[]));
    }

    #[test]
    fn in_file_sequences_replace_configured_ones() {
        let configured = vec![seq(&["TODO"], &["DONE"])];
        let kw = TodoKeywords::resolve(true, &[seq(&["A"], &["C"])], &configured);
        assert_eq!(kw.match_at("A x"), Some("A"));
        assert_eq!(kw.match_at("TODO x"), None);
        // Declared but empty: no keywords at all.
        let kw = TodoKeywords::resolve(true, &[seq(&[], &[])], &configured);
        assert_eq!(kw.match_at("TODO x"), None);
        let kw = TodoKeywords::resolve(false, &[], &configured);
        assert_eq!(kw.match_at("TODO x"), Some("TODO"));
        assert_eq!(kw.match_at("TODO"), Some("TODO"));
        // Case-sensitive, and must be followed by a space or the line end.
        assert_eq!(kw.match_at("todo x"), None);
        assert_eq!(kw.match_at("TODOx"), None);
        assert_eq!(kw.match_at("TODO\tx"), None);
    }

    #[test]
    fn a_keyword_that_prefixes_another_does_not_shadow_it() {
        let kw = TodoKeywords::resolve(true, &[seq(&["DO", "DOING"], &["DONE"])], &[]);
        assert_eq!(kw.match_at("DOING x"), Some("DOING"));
        assert_eq!(kw.match_at("DO x"), Some("DO"));
        assert_eq!(kw.match_at("DOIN x"), None);
    }
}
