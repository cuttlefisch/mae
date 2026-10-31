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
    /// Parses an in-file `#+TODO:` value the way Org does: whitespace
    /// separated words, `|` splits todo from done, a missing `|` makes the
    /// last word the only done state, and a fast-access suffix such as
    /// `(t)` or `(w@/!)` is not part of the keyword.
    pub fn parse(value: &str) -> Option<TodoSequence> {
        let words: Vec<&str> = value.split_whitespace().collect();
        let bar = words.iter().position(|w| *w == "|");
        let strip = |w: &&str| -> Option<String> {
            let kw = w.split('(').next().unwrap_or("");
            (!kw.is_empty() && *w != "|").then(|| kw.to_string())
        };
        let (todo, done): (Vec<String>, Vec<String>) = match bar {
            Some(i) => (
                words[..i].iter().filter_map(strip).collect(),
                words[i + 1..].iter().filter_map(strip).collect(),
            ),
            None => {
                let all: Vec<String> = words.iter().filter_map(strip).collect();
                match all.split_last() {
                    Some((last, rest)) => (rest.to_vec(), vec![last.clone()]),
                    None => (Vec::new(), Vec::new()),
                }
            }
        };
        (!todo.is_empty() || !done.is_empty()).then_some(TodoSequence { todo, done })
    }

    fn keywords(&self) -> impl Iterator<Item = &str> {
        self.todo.iter().chain(&self.done).map(String::as_str)
    }
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
    /// diagnostic: the parse never recurses on input nesting.
    pub max_depth: usize,
}

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

/// The TODO keywords in force for one document.
#[derive(Debug, Clone, Default)]
pub(crate) struct TodoKeywords {
    words: Vec<String>,
}

impl TodoKeywords {
    /// In-file sequences win over configured ones when the file has any.
    pub fn resolve(in_file: &[TodoSequence], configured: &[TodoSequence]) -> TodoKeywords {
        let source = if in_file.is_empty() {
            configured
        } else {
            in_file
        };
        let mut words: Vec<String> = source
            .iter()
            .flat_map(|s| s.keywords().map(str::to_string))
            .collect();
        // Longest first, so a keyword that prefixes another cannot shadow it.
        words.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        words.dedup();
        TodoKeywords { words }
    }

    /// The keyword that starts `s` and is followed by a space or the end of
    /// the line (case-sensitive, as Org matches it).
    pub fn match_at<'s>(&self, s: &'s str) -> Option<&'s str> {
        self.words.iter().find_map(|w| {
            let rest = s.strip_prefix(w.as_str())?;
            (rest.is_empty() || rest.starts_with(' ')).then(|| &s[..w.len()])
        })
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
        assert_eq!(
            TodoSequence::parse("A B | C"),
            Some(seq(&["A", "B"], &["C"]))
        );
        assert_eq!(
            TodoSequence::parse("TODO(t) WAIT(w@/!) | DONE(d!)"),
            Some(seq(&["TODO", "WAIT"], &["DONE"]))
        );
        // No bar: the last word is the done state.
        assert_eq!(
            TodoSequence::parse("NEXT LATER FINISHED"),
            Some(seq(&["NEXT", "LATER"], &["FINISHED"]))
        );
        assert_eq!(TodoSequence::parse("   "), None);
        assert_eq!(TodoSequence::parse("|"), None);
    }

    #[test]
    fn in_file_sequences_replace_configured_ones() {
        let configured = vec![seq(&["TODO"], &["DONE"])];
        let kw = TodoKeywords::resolve(&[seq(&["A"], &["C"])], &configured);
        assert_eq!(kw.match_at("A x"), Some("A"));
        assert_eq!(kw.match_at("TODO x"), None);
        let kw = TodoKeywords::resolve(&[], &configured);
        assert_eq!(kw.match_at("TODO x"), Some("TODO"));
        assert_eq!(kw.match_at("TODO"), Some("TODO"));
        // Case-sensitive, and must be followed by a space or the line end.
        assert_eq!(kw.match_at("todo x"), None);
        assert_eq!(kw.match_at("TODOx"), None);
        assert_eq!(kw.match_at("TODO\tx"), None);
    }

    #[test]
    fn a_keyword_that_prefixes_another_does_not_shadow_it() {
        let kw = TodoKeywords::resolve(&[seq(&["DO", "DOING"], &["DONE"])], &[]);
        assert_eq!(kw.match_at("DOING x"), Some("DOING"));
        assert_eq!(kw.match_at("DO x"), Some("DO"));
    }
}
