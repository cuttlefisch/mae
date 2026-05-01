//! Tree-sitter syntax highlighting.
//!
//! For each buffer that has a recognized language, we maintain a parsed
//! syntax tree and (on demand) a flat list of `HighlightSpan`s -- byte
//! ranges tagged with a theme key that the renderer consumes.
//!
//! Design notes:
//! - Per-buffer state lives outside the Buffer (which stays language-agnostic)
//!   so swapping rope/tree independently is easy. The Editor owns the map.
//! - For the MVP we re-parse the whole buffer on every change. Incremental
//!   reparsing via `Tree::edit` is a straightforward follow-up but requires
//!   threading edit ranges through the edit path -- deferred.
//! - Theme keys are the bare names already present in every bundled theme
//!   (`keyword`, `string`, `function`, etc.). No `syntax.` prefix needed.
//!
//! Adding a language is three steps:
//!   1. Add the grammar crate to Cargo.toml.
//!   2. Add a match arm in `language_for_path`.
//!   3. Add a branch in `build_configuration` with its query string.

pub mod detection;
pub mod folds;
mod incremental;
pub mod languages;
pub mod markup;
pub mod spans;

// Re-export everything so `crate::syntax::*` keeps working.
pub use detection::{
    language_for_buffer, language_for_path, language_from_id, language_from_modeline,
    language_from_shebang,
};
pub use languages::{compute_spans_standalone, parse_once, Language};
pub use markup::{
    code_block_byte_ranges, compute_markdown_style_spans, compute_markup_spans,
    compute_org_style_spans, detect_code_block_lines, MarkupFlavor,
};
pub use spans::{
    cached_visible_syntax_spans, compute_visible_syntax_spans, drain_pending_reparses,
    SyntaxSpanMap,
};

use std::collections::HashMap;

use tree_sitter::{Parser, Tree};
use tree_sitter_highlight::HighlightConfiguration;

/// Highlight groups we recognize. Order matches the theme key table below
/// and is the order passed to `HighlightConfiguration::configure`.
///
/// The names here mirror the capture names used by the Helix/tree-sitter
/// highlight queries -- `@keyword`, `@string.special`, etc. -- with the
/// dotted suffixes mostly flattened since our themes currently only
/// style the top-level name.
pub(crate) const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "boolean",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "escape",
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
    "punctuation.special",
    "string",
    "string.escape",
    "string.special",
    "string.special.key",
    "tag",
    // Markdown / org markup captures -- `@text.title` etc. from
    // nvim-treesitter-style queries that the bundled grammars ship.
    "text.emphasis",
    "text.literal",
    "text.reference",
    "text.strong",
    "text.title",
    "text.uri",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

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
    /// Cached `HighlightConfiguration` per language. These are expensive to
    /// build (query compilation) but immutable per language, so we cache them.
    configs: HashMap<Language, HighlightConfiguration>,
}

struct SyntaxState {
    language: Language,
    tree: Option<Tree>,
    /// Cached highlight spans. Recomputed lazily when buffer generation changes.
    /// Wrapped in `Arc` so `compute_visible_syntax_spans` can return cheap clones
    /// instead of copying all spans every frame.
    spans: Option<std::sync::Arc<Vec<HighlightSpan>>>,
    /// Buffer generation at which `spans` was last computed.
    /// Compared against `Buffer::generation` to detect staleness.
    computed_at: u64,
    /// True when `apply_edit` has modified the tree in-place but the tree
    /// hasn't been reparsed against the current source yet. `tree_for()`
    /// checks this to trigger an incremental reparse.
    tree_dirty: bool,
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
                computed_at: 0,
                tree_dirty: false,
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

    /// Alias for `language_of` -- used by Scheme injection for `*buffer-language*`.
    pub fn language_for(&self, buf_idx: usize) -> Option<Language> {
        self.language_of(buf_idx)
    }

    /// Returns `true` if cached spans exist and are fresh for this buffer
    /// (no reparse needed). Used by renderers to skip the Rope->String
    /// allocation when the cache is still valid.
    pub fn has_cached_spans(&self, buf_idx: usize, generation: u64) -> bool {
        self.entries
            .get(&buf_idx)
            .is_some_and(|s| s.spans.is_some() && s.computed_at == generation)
    }

    /// Return cached spans only if they are fresh (computed at the given
    /// generation). Returns `None` if stale or never computed.
    pub fn cached_spans(&self, buf_idx: usize, generation: u64) -> Option<&[HighlightSpan]> {
        self.entries.get(&buf_idx).and_then(|s| {
            if s.computed_at == generation {
                s.spans.as_ref().map(|v| &v[..])
            } else {
                None
            }
        })
    }

    /// Mark the buffer as dirty so the next `spans_for` reparses.
    /// Still useful for callers that force a reparse (e.g. language change).
    pub fn invalidate(&mut self, buf_idx: usize) {
        if let Some(state) = self.entries.get_mut(&buf_idx) {
            state.spans = None;
            state.tree = None;
        }
    }

    /// Apply an incremental edit to the cached tree-sitter tree.
    ///
    /// Calling `Tree::edit()` tells tree-sitter which byte ranges changed,
    /// so the next `parser.parse(source, Some(&old_tree))` is O(changed)
    /// instead of O(file). The `spans` are cleared (will recompute lazily).
    pub fn apply_edit(&mut self, buf_idx: usize, edit: &tree_sitter::InputEdit) {
        let Some(state) = self.entries.get_mut(&buf_idx) else {
            return;
        };
        if let Some(ref mut tree) = state.tree {
            tree.edit(edit);
            state.tree_dirty = true;
        }
        // Spans are now stale -- will be recomputed lazily.
        state.spans = None;
    }

    /// Return a cheap `Arc` clone of cached spans regardless of freshness.
    /// Returns `(arc, is_fresh)`. Used by `compute_visible_syntax_spans` to
    /// avoid cloning all spans every frame.
    pub fn cached_spans_arc(
        &self,
        buf_idx: usize,
        generation: u64,
    ) -> Option<(std::sync::Arc<Vec<HighlightSpan>>, bool)> {
        self.entries.get(&buf_idx).and_then(|s| {
            s.spans
                .as_ref()
                .map(|arc| (arc.clone(), s.computed_at == generation))
        })
    }

    /// Return cached spans regardless of freshness. Returns `(spans, is_fresh)`
    /// where `is_fresh` is true if computed at the given generation.
    /// Always returns spans if any exist (even stale), plus the freshness flag.
    /// Returns `None` only if no spans have ever been computed for this buffer.
    pub fn cached_spans_any(
        &self,
        buf_idx: usize,
        generation: u64,
    ) -> Option<(&[HighlightSpan], bool)> {
        self.entries.get(&buf_idx).and_then(|s| {
            s.spans
                .as_ref()
                .map(|spans| (&spans[..], s.computed_at == generation))
        })
    }

    /// Return cached (or freshly computed) highlight spans for the buffer.
    /// Reparses only when the buffer generation has changed since the last
    /// computation.
    /// Returns `None` if the buffer has no associated language.
    pub fn spans_for(
        &mut self,
        buf_idx: usize,
        source: &str,
        generation: u64,
    ) -> Option<&[HighlightSpan]> {
        let state = self.entries.get_mut(&buf_idx)?;
        if state.spans.is_none() || state.computed_at != generation {
            let lang = state.language;
            state.spans = Some(std::sync::Arc::new(languages::compute_spans_with_cache(
                lang,
                source,
                &mut self.configs,
            )));
            // Don't clear tree -- keep it for incremental reparse via apply_edit.
            // Mark it dirty so tree_for() re-parses against current source.
            state.tree_dirty = state.tree.is_some();
            state.computed_at = generation;
        }
        state.spans.as_ref().map(|v| &v[..])
    }

    /// Like `spans_for` but returns a cheap `Arc` clone instead of a borrow.
    pub fn spans_for_arc(
        &mut self,
        buf_idx: usize,
        source: &str,
        generation: u64,
    ) -> Option<std::sync::Arc<Vec<HighlightSpan>>> {
        // Ensure spans are computed.
        self.spans_for(buf_idx, source, generation)?;
        self.entries
            .get(&buf_idx)
            .and_then(|s| s.spans.as_ref().cloned())
    }

    /// Return a cached (or freshly parsed) tree for the buffer.
    /// Returns `None` if the buffer has no associated language or parsing failed.
    ///
    /// If a previous tree exists and was edited via `apply_edit`, uses it
    /// as the `old_tree` for incremental reparse -- O(changed) instead of O(file).
    pub fn tree_for(&mut self, buf_idx: usize, source: &str) -> Option<&Tree> {
        let state = self.entries.get_mut(&buf_idx)?;
        if state.tree.is_none() {
            state.tree = parse_once(state.language, source);
        } else if state.tree_dirty {
            // Incremental reparse: use the edited tree as old_tree.
            let ts_lang = state.language.ts_language();
            if let Some(ts_lang) = ts_lang {
                let mut parser = Parser::new();
                if parser.set_language(&ts_lang).is_ok() {
                    if let Some(new_tree) = parser.parse(source, state.tree.as_ref()) {
                        state.tree = Some(new_tree);
                    }
                }
            }
            state.tree_dirty = false;
        }
        state.tree.as_ref()
    }

    /// Compute foldable ranges from the tree-sitter parse tree.
    ///
    /// Returns `(start_line, end_line)` pairs for multi-line named nodes that
    /// represent logical code blocks (functions, structs, impl blocks, classes,
    /// if/match/loop bodies, etc.). Only top-level and one-level-deep nodes
    /// are returned to avoid excessive fold points.
    pub fn compute_fold_ranges(&mut self, buf_idx: usize, source: &str) -> Vec<(usize, usize)> {
        let Some(tree) = self.tree_for(buf_idx, source) else {
            return Vec::new();
        };
        let root = tree.root_node();
        let mut ranges = Vec::new();
        folds::collect_fold_nodes(root, source, &mut ranges, 0);
        ranges.sort_by_key(|(s, _)| *s);
        ranges.dedup();
        ranges
    }
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
    fn detects_expanded_language_set() {
        let cases: &[(&str, Language)] = &[
            ("foo.py", Language::Python),
            ("foo.pyi", Language::Python),
            ("foo.js", Language::JavaScript),
            ("foo.mjs", Language::JavaScript),
            ("foo.jsx", Language::JavaScript),
            ("foo.ts", Language::TypeScript),
            ("foo.tsx", Language::Tsx),
            ("foo.go", Language::Go),
            ("foo.json", Language::Json),
            ("foo.sh", Language::Bash),
            (".bashrc", Language::Bash),
            ("foo.scm", Language::Scheme),
            ("foo.yaml", Language::Yaml),
            ("foo.yml", Language::Yaml),
        ];
        for (path, expected) in cases {
            assert_eq!(
                language_for_path(Path::new(path)),
                Some(*expected),
                "{} should map to {:?}",
                path,
                expected
            );
        }
    }

    #[test]
    fn python_highlights_keyword_and_string() {
        let spans = languages::compute_spans(Language::Python, "def foo():\n    return \"hi\"\n");
        assert!(spans.iter().any(|s| s.theme_key == "keyword"));
        assert!(spans.iter().any(|s| s.theme_key == "string"));
    }

    #[test]
    fn javascript_highlights_keyword_and_string() {
        let spans = languages::compute_spans(Language::JavaScript, "const x = \"hi\";");
        assert!(spans.iter().any(|s| s.theme_key == "keyword"));
        assert!(spans.iter().any(|s| s.theme_key == "string"));
    }

    #[test]
    fn typescript_highlights_keyword_and_type() {
        let spans = languages::compute_spans(
            Language::TypeScript,
            "function add(a: number, b: number): number { return a + b; }",
        );
        assert!(!spans.is_empty());
        assert!(spans.iter().any(|s| s.theme_key == "keyword"));
    }

    #[test]
    fn go_highlights_keyword_and_string() {
        let spans =
            languages::compute_spans(Language::Go, "package main\nfunc main() { _ = \"hi\" }\n");
        assert!(spans.iter().any(|s| s.theme_key == "keyword"));
        assert!(spans.iter().any(|s| s.theme_key == "string"));
    }

    #[test]
    fn json_highlights_string_and_number() {
        let spans = languages::compute_spans(Language::Json, "{\"name\": \"mae\", \"count\": 42}");
        assert!(spans.iter().any(|s| s.theme_key == "string"));
        assert!(spans.iter().any(|s| s.theme_key == "number"));
    }

    #[test]
    fn bash_highlights_produce_spans() {
        let spans = languages::compute_spans(Language::Bash, "echo \"hello $USER\"\n");
        assert!(!spans.is_empty());
    }

    #[test]
    fn scheme_highlights_produce_spans() {
        let spans = languages::compute_spans(Language::Scheme, "(define (square x) (* x x))\n");
        assert!(!spans.is_empty());
    }

    #[test]
    fn yaml_highlights_produce_spans() {
        let spans = languages::compute_spans(Language::Yaml, "name: mae\nversion: 0.1.0\n");
        assert!(!spans.is_empty());
    }

    #[test]
    fn rust_highlights_keyword_and_string() {
        let src = r#"fn main() { let x = "hi"; }"#;
        let spans = languages::compute_spans(Language::Rust, src);
        assert!(!spans.is_empty(), "expected spans for rust source");
        let has_keyword = spans.iter().any(|s| s.theme_key == "keyword");
        let has_string = spans.iter().any(|s| s.theme_key == "string");
        assert!(has_keyword, "no keyword spans: {:?}", spans);
        assert!(has_string, "no string spans: {:?}", spans);
    }

    #[test]
    fn syntax_map_caches_spans() {
        let mut map = SyntaxMap::new();
        map.set_language(0, Language::Rust);
        let spans_len = map.spans_for(0, "fn x() {}", 1).unwrap().len();
        assert!(spans_len > 0);
        assert_eq!(map.spans_for(0, "fn x() {}", 1).unwrap().len(), spans_len);
    }

    #[test]
    fn syntax_map_invalidate_forces_recompute() {
        let mut map = SyntaxMap::new();
        map.set_language(0, Language::Rust);
        let _ = map.spans_for(0, "fn x() {}", 1);
        let spans = map.spans_for(0, "let y = 42;", 2).unwrap();
        assert!(spans.iter().any(|s| s.theme_key == "keyword"));
    }

    #[test]
    fn shift_after_remove_rebases_indices() {
        let mut map = SyntaxMap::new();
        map.set_language(0, Language::Rust);
        map.set_language(2, Language::Toml);
        map.shift_after_remove(1);
        assert_eq!(map.language_of(0), Some(Language::Rust));
        assert_eq!(map.language_of(1), Some(Language::Toml));
        assert_eq!(map.language_of(2), None);
    }

    #[test]
    fn toml_produces_spans() {
        let spans = languages::compute_spans(Language::Toml, "name = \"mae\"\n");
        assert!(!spans.is_empty());
    }

    #[test]
    fn markdown_produces_spans() {
        let src = "# Heading\n\nSome ```code``` and `inline` and [a link](https://example.com).\n\n```rust\nfn main() {}\n```\n";
        let spans = languages::compute_spans(Language::Markdown, src);
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.heading"),
            "expected markup.heading span for `# Heading`, got {:?}",
            spans
        );
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.literal"),
            "expected markup.literal span for code block, got {:?}",
            spans
        );
    }

    #[test]
    fn org_highlighter_detects_structure() {
        let src = "#+TITLE: Notes\n* TODO Write docs :urgent:\n** Details\n- item 1\n- [[https://example.com][example]]\n*bold* and /italic/ and ~code~ and =verbatim=\n<2026-04-16 Thu>\n";
        let spans = languages::compute_spans(Language::Org, src);
        assert!(spans.iter().any(|s| s.theme_key == "markup.heading"));
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.todo"),
            "TODO should be markup.todo"
        );
        assert!(
            spans.iter().any(|s| s.theme_key == "attribute"),
            "tags or #+ directive"
        );
        assert!(spans.iter().any(|s| s.theme_key == "markup.link"));
        assert!(spans.iter().any(|s| s.theme_key == "markup.bold"));
        assert!(spans.iter().any(|s| s.theme_key == "markup.italic"));
        assert!(spans.iter().any(|s| s.theme_key == "markup.literal"));
        assert!(spans.iter().any(|s| s.theme_key == "markup.list"));
        assert!(spans.iter().any(|s| s.theme_key == "constant"), "timestamp");
    }

    #[test]
    fn org_todo_uses_markup_todo_key() {
        let spans = markup::compute_org_spans("* TODO Write docs\n");
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.todo"),
            "expected markup.todo for TODO keyword, got: {:?}",
            spans
        );
    }

    #[test]
    fn org_done_uses_markup_done_key() {
        let spans = markup::compute_org_spans("* DONE Finished task\n");
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.done"),
            "expected markup.done for DONE keyword, got: {:?}",
            spans
        );
    }

    #[test]
    fn org_next_wait_use_markup_todo_key() {
        let spans = markup::compute_org_spans("* NEXT Pending\n* WAIT Blocked\n");
        let todo_spans: Vec<_> = spans
            .iter()
            .filter(|s| s.theme_key == "markup.todo")
            .collect();
        assert_eq!(
            todo_spans.len(),
            2,
            "NEXT and WAIT should both be markup.todo, got: {:?}",
            spans
        );
    }

    #[test]
    fn org_extension_detected() {
        assert_eq!(language_for_path(Path::new("foo.org")), Some(Language::Org));
    }

    #[test]
    fn org_parse_once_returns_none() {
        assert!(parse_once(Language::Org, "* Heading\n").is_none());
    }

    #[test]
    fn parse_once_returns_tree() {
        let tree = parse_once(Language::Rust, "fn main() {}");
        assert!(tree.is_some());
    }

    #[test]
    fn tree_for_caches_tree() {
        let mut map = SyntaxMap::new();
        map.set_language(0, Language::Rust);
        let tree = map.tree_for(0, "fn x() {}").unwrap();
        assert_eq!(tree.root_node().kind(), "source_file");
    }

    #[test]
    fn tree_for_invalidated_reparses() {
        let mut map = SyntaxMap::new();
        map.set_language(0, Language::Rust);
        let _ = map.tree_for(0, "fn x() {}");
        map.invalidate(0);
        let tree = map.tree_for(0, "let y = 42;").unwrap();
        assert_eq!(tree.root_node().kind(), "source_file");
    }

    #[test]
    fn no_language_returns_none_from_map() {
        let mut map = SyntaxMap::new();
        assert!(map.spans_for(42, "source", 0).is_none());
    }

    #[test]
    fn compute_fold_ranges_rust() {
        let mut map = SyntaxMap::new();
        map.set_language(0, Language::Rust);
        let source = "fn main() {\n    println!(\"hello\");\n    let x = 1;\n}\n";
        let ranges = map.compute_fold_ranges(0, source);
        assert!(
            !ranges.is_empty(),
            "Expected at least one fold range for a Rust function"
        );
        assert!(ranges.iter().any(|(s, e)| *s == 0 && *e == 3));
    }

    #[test]
    fn incremental_reparse_preserves_tree() {
        let mut map = SyntaxMap::new();
        map.set_language(0, Language::Rust);
        let source1 = "fn main() { let x = 1; }";
        let _ = map.tree_for(0, source1);
        assert!(map.entries.get(&0).unwrap().tree.is_some());

        let edit = tree_sitter::InputEdit {
            start_byte: 12,
            old_end_byte: 22,
            new_end_byte: 22,
            start_position: tree_sitter::Point { row: 0, column: 12 },
            old_end_position: tree_sitter::Point { row: 0, column: 22 },
            new_end_position: tree_sitter::Point { row: 0, column: 22 },
        };
        map.apply_edit(0, &edit);
        assert!(map.entries.get(&0).unwrap().tree_dirty);

        let source2 = "fn main() { let y = 2; }";
        let tree = map.tree_for(0, source2);
        assert!(tree.is_some());
        assert!(!map.entries.get(&0).unwrap().tree_dirty);
    }

    #[test]
    fn spans_for_keeps_tree_alive() {
        let mut map = SyntaxMap::new();
        map.set_language(0, Language::Rust);
        let source = "fn main() {}";
        let _ = map.spans_for(0, source, 1);
        let state = map.entries.get(&0).unwrap();
        assert_eq!(state.computed_at, 1);
    }

    #[test]
    fn incremental_reparse_spans_stable() {
        let mut map = SyntaxMap::new();
        map.set_language(0, Language::Rust);
        let source = "fn foo() {\n    42\n}\n";
        let spans1 = map.spans_for(0, source, 1).map(|s| s.len());
        let spans2 = map.spans_for(0, source, 1).map(|s| s.len());
        assert_eq!(spans1, spans2);

        let source2 = "fn foo() {\n    43\n}\n";
        let spans3 = map.spans_for(0, source2, 2).map(|s| s.len());
        assert!(spans3.is_some());
    }

    #[test]
    fn apply_edit_without_tree_is_noop() {
        let mut map = SyntaxMap::new();
        map.set_language(0, Language::Rust);
        let edit = tree_sitter::InputEdit {
            start_byte: 0,
            old_end_byte: 0,
            new_end_byte: 5,
            start_position: tree_sitter::Point { row: 0, column: 0 },
            old_end_position: tree_sitter::Point { row: 0, column: 0 },
            new_end_position: tree_sitter::Point { row: 0, column: 5 },
        };
        map.apply_edit(0, &edit);
        assert!(!map.entries.get(&0).unwrap().tree_dirty);
    }

    #[test]
    fn compute_fold_ranges_no_language() {
        let mut map = SyntaxMap::new();
        let ranges = map.compute_fold_ranges(99, "some text");
        assert!(ranges.is_empty());
    }

    #[test]
    fn compute_fold_ranges_multiple_functions() {
        let mut map = SyntaxMap::new();
        map.set_language(0, Language::Rust);
        let source = "fn foo() {\n    1\n}\nfn bar() {\n    2\n}\n";
        let ranges = map.compute_fold_ranges(0, source);
        assert!(
            ranges.len() >= 2,
            "Expected at least 2 fold ranges, got {}",
            ranges.len()
        );
    }

    #[test]
    fn markdown_style_spans_bold() {
        let spans = compute_markdown_style_spans("This is **bold** text");
        assert!(spans.iter().any(|s| s.theme_key == "markup.bold"));
    }

    #[test]
    fn markdown_style_spans_code() {
        let spans = compute_markdown_style_spans("Use `code` here");
        assert!(spans.iter().any(|s| s.theme_key == "markup.literal"));
    }

    #[test]
    fn markdown_style_spans_italic() {
        let spans = compute_markdown_style_spans("This is *italic* word");
        assert!(spans.iter().any(|s| s.theme_key == "markup.italic"));
    }

    #[test]
    fn markdown_style_spans_no_headings() {
        let spans = compute_markdown_style_spans("# Heading\n## Sub\n**bold**");
        assert!(
            !spans.iter().any(|s| s.theme_key == "markup.heading"),
            "compute_markdown_style_spans must not produce markup.heading spans"
        );
    }

    #[test]
    fn markdown_style_spans_mixed() {
        let spans = compute_markdown_style_spans("**bold** and `code` and *italic*");
        assert!(spans.iter().any(|s| s.theme_key == "markup.bold"));
        assert!(spans.iter().any(|s| s.theme_key == "markup.literal"));
        assert!(spans.iter().any(|s| s.theme_key == "markup.italic"));
        for w in spans.windows(2) {
            assert!(w[0].byte_start <= w[1].byte_start);
        }
    }

    #[test]
    fn markdown_style_spans_empty() {
        let spans = compute_markdown_style_spans("");
        assert!(spans.is_empty());
    }

    #[test]
    fn org_style_spans_bold() {
        let spans = compute_org_style_spans("This is *bold* text");
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.bold"),
            "expected markup.bold from *bold*, got: {:?}",
            spans
        );
    }

    #[test]
    fn org_style_spans_italic() {
        let spans = compute_org_style_spans("This is /italic/ text");
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.italic"),
            "expected markup.italic from /italic/, got: {:?}",
            spans
        );
    }

    #[test]
    fn org_style_spans_code() {
        let spans = compute_org_style_spans("This is =code= text");
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.literal"),
            "expected markup.literal from =code=, got: {:?}",
            spans
        );
    }

    #[test]
    fn org_style_spans_verbatim() {
        let spans = compute_org_style_spans("This is ~verbatim~ text");
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.literal"),
            "expected markup.literal from ~verbatim~, got: {:?}",
            spans
        );
    }

    #[test]
    fn org_style_spans_no_headings() {
        let spans = compute_org_style_spans("* heading\n** sub\n*bold*");
        assert!(
            !spans.iter().any(|s| s.theme_key == "markup.heading"),
            "compute_org_style_spans must not produce markup.heading spans"
        );
    }

    #[test]
    fn org_style_spans_mixed() {
        let spans = compute_org_style_spans("*bold* and /italic/ and =code=");
        assert!(spans.iter().any(|s| s.theme_key == "markup.bold"));
        assert!(spans.iter().any(|s| s.theme_key == "markup.italic"));
        assert!(spans.iter().any(|s| s.theme_key == "markup.literal"));
        for w in spans.windows(2) {
            assert!(w[0].byte_start <= w[1].byte_start);
        }
    }

    #[test]
    fn language_default_local_options() {
        let md = Language::Markdown.default_local_options();
        assert_eq!(md.heading_scale, Some(true));
        assert_eq!(md.render_markup, Some(true));
        assert_eq!(md.link_descriptive, Some(true));
        assert_eq!(md.word_wrap, Some(true));

        let org = Language::Org.default_local_options();
        assert_eq!(org.heading_scale, Some(true));
        assert_eq!(org.word_wrap, Some(true));

        let json = Language::Json.default_local_options();
        assert_eq!(json.word_wrap, Some(false));
        let yaml = Language::Yaml.default_local_options();
        assert_eq!(yaml.word_wrap, Some(false));
        let toml = Language::Toml.default_local_options();
        assert_eq!(toml.word_wrap, Some(false));

        let rs = Language::Rust.default_local_options();
        assert_eq!(rs.heading_scale, None);
        assert_eq!(rs.render_markup, None);
        assert_eq!(rs.link_descriptive, None);
    }

    #[test]
    fn shebang_python3() {
        assert_eq!(
            language_from_shebang("#!/usr/bin/env python3"),
            Some(Language::Python)
        );
    }

    #[test]
    fn shebang_bash() {
        assert_eq!(language_from_shebang("#!/bin/bash"), Some(Language::Bash));
    }

    #[test]
    fn shebang_node() {
        assert_eq!(
            language_from_shebang("#!/usr/bin/env node"),
            Some(Language::JavaScript)
        );
    }

    #[test]
    fn shebang_no_shebang() {
        assert_eq!(language_from_shebang("# just a comment"), None);
        assert_eq!(language_from_shebang(""), None);
    }

    #[test]
    fn shebang_env_with_flags() {
        assert_eq!(
            language_from_shebang("#!/usr/bin/env -S node"),
            Some(Language::JavaScript)
        );
    }

    #[test]
    fn modeline_first_line() {
        assert_eq!(
            language_from_modeline("# mae: language=rust\nsome code"),
            Some(Language::Rust)
        );
    }

    #[test]
    fn modeline_last_line() {
        let content = "line1\nline2\nline3\nline4\nline5\nline6\n# mae: language=python\n";
        assert_eq!(language_from_modeline(content), Some(Language::Python));
    }

    #[test]
    fn modeline_no_match() {
        assert_eq!(
            language_from_modeline("just some text\nno modeline here\n"),
            None
        );
    }

    #[test]
    fn modeline_priority_over_ext() {
        let content = "# mae: language=python\nfn main() {}";
        assert_eq!(
            language_for_buffer(Path::new("foo.rs"), content),
            Some(Language::Python)
        );
    }

    #[test]
    fn language_from_id_valid() {
        assert_eq!(language_from_id("rust"), Some(Language::Rust));
        assert_eq!(language_from_id("Python"), Some(Language::Python));
        assert_eq!(language_from_id("JS"), Some(Language::JavaScript));
        assert_eq!(language_from_id("golang"), Some(Language::Go));
    }

    #[test]
    fn language_from_id_invalid() {
        assert_eq!(language_from_id("cobol"), None);
    }

    #[test]
    fn shared_markup_spans_skip_code_blocks() {
        let md_src = "**bold**\n```\n**not bold**\n```\n";
        let spans = compute_markup_spans(md_src, MarkupFlavor::Markdown);
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.bold"),
            "expected markup.bold for text outside code block"
        );
        let code_start = md_src.find("**not bold**").unwrap();
        assert!(
            !spans
                .iter()
                .any(|s| s.theme_key == "markup.bold" && s.byte_start >= code_start),
            "markup.bold should NOT appear inside fenced code block"
        );

        let org_src = "*bold*\n#+begin_src python\n*not bold*\n#+end_src\n";
        let spans = compute_markup_spans(org_src, MarkupFlavor::Org);
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.bold"),
            "expected markup.bold for text outside org src block"
        );
        let code_start = org_src.find("*not bold*").unwrap();
        assert!(
            !spans
                .iter()
                .any(|s| s.theme_key == "markup.bold" && s.byte_start >= code_start),
            "markup.bold should NOT appear inside org src block"
        );
    }

    #[test]
    fn language_for_buffer_shebang_priority() {
        let content = "#!/usr/bin/env python3\n";
        assert_eq!(
            language_for_buffer(Path::new("script.rs"), content),
            Some(Language::Python)
        );
    }

    #[test]
    fn language_for_buffer_extension_fallback() {
        let content = "fn main() {}";
        assert_eq!(
            language_for_buffer(Path::new("foo.rs"), content),
            Some(Language::Rust)
        );
    }

    #[test]
    fn syntax_map_language_for() {
        let mut map = SyntaxMap::new();
        map.set_language(0, Language::Rust);
        assert_eq!(map.language_for(0), Some(Language::Rust));
        assert_eq!(map.language_for(99), None);
    }

    #[test]
    fn markdown_strikethrough_spans() {
        let spans = compute_markdown_style_spans("This is ~~deleted~~ text");
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.strikethrough"),
            "expected markup.strikethrough span, got: {:?}",
            spans
        );
    }

    fn buf_with_text_and_path(text: &str, path: &str) -> crate::Buffer {
        let mut buf = crate::Buffer::new();
        buf.insert_text_at(0, text);
        buf.set_file_path(std::path::PathBuf::from(path));
        buf
    }

    #[test]
    fn detect_code_blocks_md() {
        let buf = buf_with_text_and_path("line 0\n```rust\nfn main() {}\n```\nline 4", "test.md");
        let lines = detect_code_block_lines(&buf, MarkupFlavor::Markdown);
        assert_eq!(lines.len(), 5);
        assert!(!lines[0], "line 0 is not in code block");
        assert!(lines[1], "``` opening fence is in code block");
        assert!(lines[2], "code content is in code block");
        assert!(lines[3], "``` closing fence is in code block");
        assert!(!lines[4], "line 4 is not in code block");
    }

    #[test]
    fn detect_code_blocks_org() {
        let buf = buf_with_text_and_path(
            "text\n#+begin_src python\nprint(1)\n#+end_src\nmore",
            "test.org",
        );
        let lines = detect_code_block_lines(&buf, MarkupFlavor::Org);
        assert_eq!(lines.len(), 5);
        assert!(!lines[0]);
        assert!(lines[1]);
        assert!(lines[2]);
        assert!(lines[3]);
        assert!(!lines[4]);
    }

    #[test]
    fn detect_code_blocks_flavor_none() {
        let buf = buf_with_text_and_path("```\ncode\n```", "test.rs");
        let lines = detect_code_block_lines(&buf, MarkupFlavor::None);
        assert!(
            lines.iter().all(|&x| !x),
            "MarkupFlavor::None produces no code blocks"
        );
    }

    #[test]
    fn markup_flavor_markdown_produces_spans() {
        let spans = compute_markup_spans("**bold** and `code`", MarkupFlavor::Markdown);
        assert!(spans.iter().any(|s| s.theme_key == "markup.bold"));
        assert!(spans.iter().any(|s| s.theme_key == "markup.literal"));
    }

    #[test]
    fn markup_flavor_org_produces_spans() {
        let spans = compute_markup_spans("*bold* and =code=", MarkupFlavor::Org);
        assert!(spans.iter().any(|s| s.theme_key == "markup.bold"));
        assert!(spans.iter().any(|s| s.theme_key == "markup.literal"));
    }

    #[test]
    fn markup_flavor_none_empty() {
        let spans = compute_markup_spans("**bold** and `code`", MarkupFlavor::None);
        assert!(spans.is_empty());
    }

    #[test]
    fn language_markup_flavor() {
        assert_eq!(Language::Markdown.markup_flavor(), MarkupFlavor::Markdown);
        assert_eq!(Language::Org.markup_flavor(), MarkupFlavor::Org);
        assert_eq!(Language::Rust.markup_flavor(), MarkupFlavor::None);
        assert_eq!(Language::Python.markup_flavor(), MarkupFlavor::None);
    }

    #[test]
    fn markdown_code_block_has_injected_spans() {
        let src = "# Heading\n\n```rust\nfn main() {}\n```\n";
        let spans = languages::compute_spans(Language::Markdown, src);
        assert!(
            spans.iter().any(|s| s.theme_key == "keyword"),
            "expected keyword span for `fn` in fenced code block, got: {:?}",
            spans
        );
    }

    #[test]
    fn markdown_code_block_no_literal_on_injected_content() {
        let src = "```rust\nfn main() {}\n```\n";
        let spans = languages::compute_spans(Language::Markdown, src);
        let fn_byte = src.find("fn").unwrap();
        let literal_on_fn = spans.iter().any(|s| {
            s.theme_key == "markup.literal" && s.byte_start <= fn_byte && s.byte_end > fn_byte
        });
        assert!(
            !literal_on_fn,
            "markup.literal should be removed from injected code content"
        );
    }

    #[test]
    fn org_src_block_has_language_spans() {
        let src = "#+begin_src rust\nfn main() {}\n#+end_src\n";
        let spans = markup::compute_org_spans(src);
        assert!(
            spans.iter().any(|s| s.theme_key == "keyword"),
            "expected keyword span for `fn` in org src block, got: {:?}",
            spans
        );
    }

    #[test]
    fn injection_callback_resolves_rust() {
        assert_eq!(language_from_id("rust"), Some(Language::Rust));
        assert!(languages::build_configuration(Language::Rust).is_some());
    }

    #[test]
    fn markdown_no_spurious_markup_in_code_block() {
        let src =
            "**outside bold**\n```python\ndef **foo**():\n    \"\"\"*italic* `code`\"\"\"\n```\n";
        let spans = compute_markup_spans(src, MarkupFlavor::Markdown);
        let fence_end = src.find("```\n").unwrap();
        let code_start = src[fence_end..].find('\n').unwrap() + fence_end + 1;
        assert!(
            spans
                .iter()
                .any(|s| s.theme_key == "markup.bold" && s.byte_end <= fence_end),
            "expected markup.bold outside code block"
        );
        let spurious = spans.iter().any(|s| s.byte_start >= code_start);
        assert!(
            !spurious,
            "no markup spans should appear inside fenced code block, got: {:?}",
            spans
                .iter()
                .filter(|s| s.byte_start >= code_start)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn code_block_byte_ranges_markdown() {
        let src = "text\n```rust\ncode\n```\nmore text\n";
        let ranges = code_block_byte_ranges(src, MarkupFlavor::Markdown);
        assert_eq!(ranges.len(), 1);
        let (start, end) = ranges[0];
        assert_eq!(&src[start..end], "code\n");
    }

    #[test]
    fn code_block_byte_ranges_org() {
        let src = "text\n#+begin_src python\ncode\n#+end_src\n";
        let ranges = code_block_byte_ranges(src, MarkupFlavor::Org);
        assert_eq!(ranges.len(), 1);
        let (start, end) = ranges[0];
        assert_eq!(&src[start..end], "code\n");
    }

    #[test]
    fn checkbox_span_unchecked_theme_key() {
        let spans = markup::compute_org_spans("- [ ] unchecked item\n");
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.checkbox"),
            "expected markup.checkbox for unchecked, got: {:?}",
            spans
        );
    }

    #[test]
    fn checkbox_span_checked_theme_key() {
        let spans = markup::compute_org_spans("- [x] checked item\n");
        assert!(
            spans
                .iter()
                .any(|s| s.theme_key == "markup.checkbox.checked"),
            "expected markup.checkbox.checked for checked, got: {:?}",
            spans
        );
    }

    #[test]
    fn markdown_code_block_no_residual_markup_literal() {
        let src = include_str!("../../../../assets/markup-demo.md");
        let spans = languages::compute_spans(Language::Markdown, src);

        let code_ranges = code_block_byte_ranges(src, MarkupFlavor::Markdown);

        let residual: Vec<_> = spans
            .iter()
            .filter(|s| s.theme_key == "markup.literal")
            .filter(|s| {
                code_ranges
                    .iter()
                    .any(|(start, end)| s.byte_start < *end && s.byte_end > *start)
            })
            .collect();
        assert!(
            residual.is_empty(),
            "markup.literal must not survive in code blocks, found {} spans: {:?}",
            residual.len(),
            residual
        );

        let fn_byte = src.find("fn main()").unwrap();
        assert!(
            spans.iter().any(|s| s.theme_key == "keyword"
                && s.byte_start <= fn_byte
                && s.byte_end > fn_byte),
            "expected keyword span for `fn`"
        );

        let rust_block = code_ranges[0];
        let injected_in_rust: Vec<_> = spans
            .iter()
            .filter(|s| {
                s.byte_start >= rust_block.0
                    && s.byte_end <= rust_block.1
                    && !s.theme_key.starts_with("markup")
            })
            .collect();
        assert!(
            !injected_in_rust.is_empty(),
            "expected injected language spans in Rust code block"
        );

        let python_block = code_ranges[1];
        let injected_in_python: Vec<_> = spans
            .iter()
            .filter(|s| {
                s.byte_start >= python_block.0
                    && s.byte_end <= python_block.1
                    && !s.theme_key.starts_with("markup")
            })
            .collect();
        assert!(
            !injected_in_python.is_empty(),
            "expected injected language spans in Python code block"
        );
    }

    #[test]
    fn markdown_enrichment_no_spans_in_code_blocks() {
        let src = include_str!("../../../../assets/markup-demo.md");
        let code_ranges = code_block_byte_ranges(src, MarkupFlavor::Markdown);
        let markup_spans = compute_markup_spans(src, MarkupFlavor::Markdown);
        let inside_code: Vec<_> = markup_spans
            .iter()
            .filter(|s| {
                code_ranges
                    .iter()
                    .any(|(start, end)| s.byte_start >= *start && s.byte_end <= *end)
            })
            .collect();
        assert!(
            inside_code.is_empty(),
            "regex markup spans must not exist inside code blocks: {:?}",
            inside_code
        );
    }

    #[test]
    fn markdown_checkbox_spans() {
        let spans = compute_markdown_style_spans("- [ ] todo\n- [x] done\n");
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.checkbox"),
            "expected unchecked checkbox span in markdown, got: {:?}",
            spans
        );
        assert!(
            spans
                .iter()
                .any(|s| s.theme_key == "markup.checkbox.checked"),
            "expected checked checkbox span in markdown, got: {:?}",
            spans
        );
    }

    // --- Module split verification tests ---

    #[test]
    fn syntax_submodule_reexports_compile() {
        // Verify all re-exported items from submodules are accessible.
        let _ = Language::Rust.id();
        let _ = MarkupFlavor::Markdown;
        let _: Option<Language> = language_for_path(std::path::Path::new("test.rs"));
        let _: Option<Language> = language_from_id("rust");
        let _: Option<Language> = language_from_shebang("#!/usr/bin/env python3");
    }

    #[test]
    fn syntax_folds_accessible() {
        // Ensure fold-related code in folds.rs is callable through SyntaxMap.
        let mut sm = SyntaxMap::new();
        sm.set_language(0, Language::Rust);
        let source = "fn main() {\n    let x = 1;\n    let y = 2;\n}\n";
        let ranges = sm.compute_fold_ranges(0, source);
        assert!(!ranges.is_empty(), "expected fold ranges for Rust function");
    }

    #[test]
    fn syntax_spans_standalone_accessible() {
        let spans = compute_spans_standalone(Language::Rust, "fn main() {}");
        assert!(
            spans.iter().any(|s| s.theme_key == "keyword"),
            "expected keyword span from standalone computation"
        );
    }

    #[test]
    fn syntax_detection_priority_chain() {
        // Shebang > modeline > extension via language_for_buffer.
        let path = std::path::Path::new("test.py");
        let content = "#!/usr/bin/env bash\nprint('hello')\n";
        // Shebang says bash, extension says python — shebang wins.
        assert_eq!(language_for_buffer(path, content), Some(Language::Bash));
    }
}
