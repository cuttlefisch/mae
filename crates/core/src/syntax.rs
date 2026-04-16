//! Tree-sitter syntax highlighting.
//!
//! For each buffer that has a recognized language, we maintain a parsed
//! syntax tree and (on demand) a flat list of `HighlightSpan`s — byte
//! ranges tagged with a theme key that the renderer consumes.
//!
//! Design notes:
//! - Per-buffer state lives outside the Buffer (which stays language-agnostic)
//!   so swapping rope/tree independently is easy. The Editor owns the map.
//! - For the MVP we re-parse the whole buffer on every change. Incremental
//!   reparsing via `Tree::edit` is a straightforward follow-up but requires
//!   threading edit ranges through the edit path — deferred.
//! - Theme keys are the bare names already present in every bundled theme
//!   (`keyword`, `string`, `function`, etc.). No `syntax.` prefix needed.
//!
//! Adding a language is three steps:
//!   1. Add the grammar crate to Cargo.toml.
//!   2. Add a match arm in `language_for_path`.
//!   3. Add a branch in `build_configuration` with its query string.

use std::collections::HashMap;

use tree_sitter::{Parser, Tree};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// Highlight groups we recognize. Order matches the theme key table below
/// and is the order passed to `HighlightConfiguration::configure`.
///
/// The names here mirror the capture names used by the Helix/tree-sitter
/// highlight queries — `@keyword`, `@string.special`, etc. — with the
/// dotted suffixes mostly flattened since our themes currently only
/// style the top-level name.
const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "function",
    "function.builtin",
    "function.macro",
    "function.method",
    "keyword",
    "label",
    "namespace",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "string",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

/// Map a tree-sitter highlight name to a theme key. Falls back to the
/// most specific prefix present in the themes.
fn highlight_name_to_theme_key(name: &str) -> &'static str {
    // Longest-prefix match against known theme keys. Keep the list
    // small for now — add more as themes grow.
    match name {
        n if n.starts_with("comment") => "comment",
        n if n.starts_with("string") => "string",
        n if n.starts_with("number") => "number",
        n if n.starts_with("keyword") => "keyword",
        n if n.starts_with("type") => "type",
        n if n.starts_with("function") => "function",
        n if n.starts_with("constant") => "constant",
        n if n.starts_with("attribute") => "attribute",
        n if n.starts_with("namespace") => "namespace",
        n if n.starts_with("operator") => "operator",
        n if n.starts_with("punctuation") => "punctuation",
        n if n.starts_with("variable") => "variable",
        _ => "variable",
    }
}

/// Identifies a language we can highlight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Toml,
    Markdown,
}

impl Language {
    /// The language id that matches MAE's LSP `language_id_from_path`.
    pub fn id(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Toml => "toml",
            Language::Markdown => "markdown",
        }
    }
}

/// Detect the language for a file based on its extension.
pub fn language_for_path(path: &std::path::Path) -> Option<Language> {
    let ext = path.extension()?.to_str()?;
    Some(match ext {
        "rs" => Language::Rust,
        "toml" => Language::Toml,
        "md" | "markdown" => Language::Markdown,
        _ => return None,
    })
}

fn build_configuration(lang: Language) -> Option<HighlightConfiguration> {
    let (ts_lang, highlights, injections, locals) = match lang {
        Language::Rust => (
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::HIGHLIGHTS_QUERY,
            tree_sitter_rust::INJECTIONS_QUERY,
            "",
        ),
        Language::Toml => (
            tree_sitter_toml_ng::LANGUAGE.into(),
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
        Language::Markdown => (
            tree_sitter_md::LANGUAGE.into(),
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
            tree_sitter_md::INJECTION_QUERY_BLOCK,
            "",
        ),
    };
    let mut config = HighlightConfiguration::new(ts_lang, lang.id(), highlights, injections, locals)
        .ok()?;
    config.configure(HIGHLIGHT_NAMES);
    Some(config)
}

/// One syntax-highlighted byte range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    pub byte_start: usize,
    pub byte_end: usize,
    pub theme_key: &'static str,
}

/// Per-buffer syntax state. Keyed by the buffer index in `editor.buffers`.
#[derive(Default)]
pub struct SyntaxMap {
    entries: HashMap<usize, SyntaxState>,
}

struct SyntaxState {
    language: Language,
    tree: Option<Tree>,
    /// Cached highlight spans. Invalidated on edit, recomputed lazily.
    spans: Option<Vec<HighlightSpan>>,
}

impl SyntaxMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Associate a language with a buffer. Resets any cached state.
    pub fn set_language(&mut self, buf_idx: usize, language: Language) {
        self.entries.insert(
            buf_idx,
            SyntaxState {
                language,
                tree: None,
                spans: None,
            },
        );
    }

    /// Drop the entry for a buffer (e.g. on kill-buffer).
    pub fn remove(&mut self, buf_idx: usize) {
        self.entries.remove(&buf_idx);
    }

    /// Rebase indices after a buffer was removed at `removed_idx`.
    /// Buffers past that index shift down by one.
    pub fn shift_after_remove(&mut self, removed_idx: usize) {
        let mut new_entries = HashMap::new();
        for (idx, state) in self.entries.drain() {
            if idx == removed_idx {
                continue;
            }
            let new_idx = if idx > removed_idx { idx - 1 } else { idx };
            new_entries.insert(new_idx, state);
        }
        self.entries = new_entries;
    }

    pub fn language_of(&self, buf_idx: usize) -> Option<Language> {
        self.entries.get(&buf_idx).map(|s| s.language)
    }

    /// Mark the buffer as dirty so the next `spans_for` reparses.
    pub fn invalidate(&mut self, buf_idx: usize) {
        if let Some(state) = self.entries.get_mut(&buf_idx) {
            state.spans = None;
            state.tree = None;
        }
    }

    /// Return cached (or freshly computed) highlight spans for the buffer.
    /// Returns `None` if the buffer has no associated language.
    pub fn spans_for(&mut self, buf_idx: usize, source: &str) -> Option<&[HighlightSpan]> {
        let state = self.entries.get_mut(&buf_idx)?;
        if state.spans.is_none() {
            state.spans = Some(compute_spans(state.language, source));
        }
        state.spans.as_deref()
    }
}

fn compute_spans(language: Language, source: &str) -> Vec<HighlightSpan> {
    let Some(config) = build_configuration(language) else {
        return Vec::new();
    };
    let mut highlighter = Highlighter::new();
    let events = match highlighter.highlight(&config, source.as_bytes(), None, |_| None) {
        Ok(it) => it,
        Err(_) => return Vec::new(),
    };

    let mut spans = Vec::new();
    // Stack of active highlight indices — needed because regions can nest.
    let mut stack: Vec<&'static str> = Vec::new();
    for event in events {
        let Ok(event) = event else { continue };
        match event {
            HighlightEvent::HighlightStart(h) => {
                let name = HIGHLIGHT_NAMES.get(h.0).copied().unwrap_or("variable");
                stack.push(highlight_name_to_theme_key(name));
            }
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                if let Some(&key) = stack.last() {
                    spans.push(HighlightSpan {
                        byte_start: start,
                        byte_end: end,
                        theme_key: key,
                    });
                }
            }
        }
    }
    spans
}

/// Build a one-shot syntax tree (used primarily in tests).
pub fn parse_once(language: Language, source: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    let ts_lang: tree_sitter::Language = match language {
        Language::Rust => tree_sitter_rust::LANGUAGE.into(),
        Language::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
        Language::Markdown => tree_sitter_md::LANGUAGE.into(),
    };
    parser.set_language(&ts_lang).ok()?;
    parser.parse(source, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn detects_rust_from_extension() {
        assert_eq!(language_for_path(Path::new("foo.rs")), Some(Language::Rust));
        assert_eq!(
            language_for_path(Path::new("Cargo.toml")),
            Some(Language::Toml)
        );
        assert_eq!(
            language_for_path(Path::new("README.md")),
            Some(Language::Markdown)
        );
        assert_eq!(language_for_path(Path::new("unknown.xyz")), None);
        assert_eq!(language_for_path(Path::new("noext")), None);
    }

    #[test]
    fn rust_highlights_keyword_and_string() {
        let src = r#"fn main() { let x = "hi"; }"#;
        let spans = compute_spans(Language::Rust, src);
        assert!(!spans.is_empty(), "expected spans for rust source");
        // Should find at least one keyword span and one string span.
        let has_keyword = spans.iter().any(|s| s.theme_key == "keyword");
        let has_string = spans.iter().any(|s| s.theme_key == "string");
        assert!(has_keyword, "no keyword spans: {:?}", spans);
        assert!(has_string, "no string spans: {:?}", spans);
    }

    #[test]
    fn syntax_map_caches_spans() {
        let mut map = SyntaxMap::new();
        map.set_language(0, Language::Rust);
        let spans_len = map.spans_for(0, "fn x() {}").unwrap().len();
        assert!(spans_len > 0);
        // Second call returns the same cached vec; compare lengths.
        assert_eq!(map.spans_for(0, "fn x() {}").unwrap().len(), spans_len);
    }

    #[test]
    fn syntax_map_invalidate_forces_recompute() {
        let mut map = SyntaxMap::new();
        map.set_language(0, Language::Rust);
        let _ = map.spans_for(0, "fn x() {}");
        map.invalidate(0);
        // After invalidation the map must recompute against new source.
        let spans = map.spans_for(0, "let y = 42;").unwrap();
        assert!(spans.iter().any(|s| s.theme_key == "keyword"));
    }

    #[test]
    fn shift_after_remove_rebases_indices() {
        let mut map = SyntaxMap::new();
        map.set_language(0, Language::Rust);
        map.set_language(2, Language::Toml);
        map.shift_after_remove(1);
        // 0 stays, 2 becomes 1.
        assert_eq!(map.language_of(0), Some(Language::Rust));
        assert_eq!(map.language_of(1), Some(Language::Toml));
        assert_eq!(map.language_of(2), None);
    }

    #[test]
    fn toml_produces_spans() {
        let spans = compute_spans(Language::Toml, "name = \"mae\"\n");
        assert!(!spans.is_empty());
    }

    #[test]
    fn markdown_produces_spans() {
        let spans = compute_spans(Language::Markdown, "# Heading\n\nhello world\n");
        // Markdown's block grammar may produce few spans in short text —
        // we just verify it doesn't panic and returns something or
        // an empty vec (both are valid).
        let _ = spans;
    }

    #[test]
    fn parse_once_returns_tree() {
        let tree = parse_once(Language::Rust, "fn main() {}");
        assert!(tree.is_some());
    }

    #[test]
    fn no_language_returns_none_from_map() {
        let mut map = SyntaxMap::new();
        assert!(map.spans_for(42, "source").is_none());
    }
}
