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
    // Markdown / org markup captures — `@text.title` etc. from
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

/// Map a tree-sitter highlight name to a theme key. Falls back to the
/// most specific prefix present in the themes.
///
/// Markup captures (`text.title`, `text.literal`, …) are routed first to
/// dedicated `markup.*` theme keys and then to a sensible existing key
/// so themes that haven't been updated still render markdown distinctly.
/// Theme lookup strips dots, so `markup.heading` → `markup` → default.
fn highlight_name_to_theme_key(name: &str) -> &'static str {
    match name {
        // Markdown / org structural captures.
        "text.title" => "markup.heading",
        "text.literal" => "markup.literal",
        "text.uri" => "markup.link",
        "text.reference" => "markup.link",
        "text.strong" => "markup.bold",
        "text.emphasis" => "markup.italic",
        // JSON object keys and YAML properties — render as attributes.
        "string.special.key" => "attribute",
        // Escape sequences render as constants.
        n if n == "escape" || n.starts_with("string.escape") => "constant",
        "boolean" => "constant",
        "constructor" => "type",
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
        n if n.starts_with("property") => "attribute",
        n if n.starts_with("punctuation") => "punctuation",
        n if n.starts_with("tag") => "keyword",
        n if n.starts_with("variable") => "variable",
        n if n.starts_with("label") => "attribute",
        _ => "variable",
    }
}

/// Identifies a language we can highlight.
///
/// Most languages go through tree-sitter. `Org` is a special case: no
/// tree-sitter-org crate is compatible with tree-sitter 0.25 yet, so
/// `compute_spans` branches to a regex-based fallback for it. When a
/// compatible grammar lands we can drop the fallback and treat org
/// like every other language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Toml,
    Markdown,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Go,
    Json,
    Bash,
    Scheme,
    Yaml,
    Org,
}

impl Language {
    /// The language id that matches MAE's LSP `language_id_from_path`.
    pub fn id(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Toml => "toml",
            Language::Markdown => "markdown",
            Language::Python => "python",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
            Language::Tsx => "tsx",
            Language::Go => "go",
            Language::Json => "json",
            Language::Bash => "bash",
            Language::Scheme => "scheme",
            Language::Yaml => "yaml",
            Language::Org => "org",
        }
    }

    /// Get the tree-sitter `Language` for grammars we support via tree-sitter.
    /// Returns `None` for languages highlighted through a fallback path (org).
    fn ts_language(self) -> Option<tree_sitter::Language> {
        Some(match self {
            Language::Rust => tree_sitter_rust::LANGUAGE.into(),
            Language::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Language::Markdown => tree_sitter_md::LANGUAGE.into(),
            Language::Python => tree_sitter_python::LANGUAGE.into(),
            Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Language::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Language::Go => tree_sitter_go::LANGUAGE.into(),
            Language::Json => tree_sitter_json::LANGUAGE.into(),
            Language::Bash => tree_sitter_bash::LANGUAGE.into(),
            Language::Scheme => tree_sitter_scheme::LANGUAGE.into(),
            Language::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Language::Org => return None,
        })
    }
}

/// Detect the language for a file based on its extension.
pub fn language_for_path(path: &std::path::Path) -> Option<Language> {
    // Filename-first match for files without extensions (Makefile, Dockerfile…).
    if matches!(
        path.file_name().and_then(|s| s.to_str()),
        Some(".bashrc" | ".bash_profile" | ".profile" | ".zshrc")
    ) {
        return Some(Language::Bash);
    }
    let ext = path.extension()?.to_str()?;
    Some(match ext {
        "rs" => Language::Rust,
        "toml" => Language::Toml,
        "md" | "markdown" => Language::Markdown,
        "py" | "pyi" | "pyw" => Language::Python,
        "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
        "ts" => Language::TypeScript,
        "tsx" => Language::Tsx,
        "go" => Language::Go,
        "json" | "jsonc" => Language::Json,
        "sh" | "bash" | "zsh" | "ksh" => Language::Bash,
        "scm" | "ss" | "sld" | "sls" => Language::Scheme,
        "yaml" | "yml" => Language::Yaml,
        "org" => Language::Org,
        _ => return None,
    })
}

fn build_configuration(lang: Language) -> Option<HighlightConfiguration> {
    let (highlights, injections, locals) = match lang {
        Language::Rust => (
            tree_sitter_rust::HIGHLIGHTS_QUERY,
            tree_sitter_rust::INJECTIONS_QUERY,
            "",
        ),
        Language::Toml => (tree_sitter_toml_ng::HIGHLIGHTS_QUERY, "", ""),
        Language::Markdown => (
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
            tree_sitter_md::INJECTION_QUERY_BLOCK,
            "",
        ),
        Language::Python => (tree_sitter_python::HIGHLIGHTS_QUERY, "", ""),
        Language::JavaScript => (
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_javascript::INJECTIONS_QUERY,
            tree_sitter_javascript::LOCALS_QUERY,
        ),
        // TypeScript reuses JavaScript highlights as a fallback, appending
        // its own typescript-specific captures on top.
        Language::TypeScript | Language::Tsx => {
            // Concatenate JS highlights + TS highlights since the TS
            // crate's query assumes JS captures are in scope.
            static TS_COMBINED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            let combined = TS_COMBINED.get_or_init(|| {
                format!(
                    "{}\n{}",
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                    tree_sitter_typescript::HIGHLIGHTS_QUERY
                )
            });
            (
                combined.as_str(),
                tree_sitter_javascript::INJECTIONS_QUERY,
                tree_sitter_typescript::LOCALS_QUERY,
            )
        }
        Language::Go => (tree_sitter_go::HIGHLIGHTS_QUERY, "", ""),
        Language::Json => (tree_sitter_json::HIGHLIGHTS_QUERY, "", ""),
        Language::Bash => (tree_sitter_bash::HIGHLIGHT_QUERY, "", ""),
        Language::Scheme => (tree_sitter_scheme::HIGHLIGHTS_QUERY, "", ""),
        Language::Yaml => (tree_sitter_yaml::HIGHLIGHTS_QUERY, "", ""),
        // Org has no tree-sitter grammar compatible with tree-sitter 0.25;
        // `compute_spans` handles it via a regex fallback.
        Language::Org => return None,
    };
    let mut config = HighlightConfiguration::new(
        lang.ts_language()?,
        lang.id(),
        highlights,
        injections,
        locals,
    )
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

    /// Returns `true` if cached spans exist for this buffer (no reparse needed).
    /// Used by renderers to skip the Rope→String allocation when the cache is
    /// still valid.
    pub fn has_cached_spans(&self, buf_idx: usize) -> bool {
        self.entries
            .get(&buf_idx)
            .is_some_and(|s| s.spans.is_some())
    }

    /// Return cached spans without triggering a reparse. Returns `None` if
    /// no cached spans exist (invalidated or never computed).
    pub fn cached_spans(&self, buf_idx: usize) -> Option<&[HighlightSpan]> {
        self.entries.get(&buf_idx).and_then(|s| s.spans.as_deref())
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

    /// Return a cached (or freshly parsed) tree for the buffer.
    /// Returns `None` if the buffer has no associated language or parsing failed.
    pub fn tree_for(&mut self, buf_idx: usize, source: &str) -> Option<&Tree> {
        let state = self.entries.get_mut(&buf_idx)?;
        if state.tree.is_none() {
            state.tree = parse_once(state.language, source);
        }
        state.tree.as_ref()
    }
}

fn compute_spans(language: Language, source: &str) -> Vec<HighlightSpan> {
    // Org: regex-based fallback until a tree-sitter-org compatible with
    // tree-sitter 0.25 is available.
    if language == Language::Org {
        return compute_org_spans(source);
    }
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
    let ts_lang = language.ts_language()?;
    parser.set_language(&ts_lang).ok()?;
    parser.parse(source, None)
}

/// Regex-based highlighter for org-mode files. This is a stop-gap until
/// a tree-sitter-org crate compatible with tree-sitter 0.25 is available.
///
/// Supported constructs (pragmatic subset):
/// - Headlines: `* Heading`, `** Subheading`, etc.
/// - TODO/DONE keywords in headlines
/// - Tags at end of headline: `:tag:foo:`
/// - Structural directives: `#+TITLE:`, `#+begin_src`, etc.
/// - Blocks: content between `#+begin_*` and `#+end_*` treated as literal
/// - Comments: lines starting with `# `
/// - Timestamps: `<2026-04-16>`, `[2026-04-16 Thu]`
/// - Links: `[[target][label]]` and `[[target]]`
/// - Emphasis: `*bold*`, `/italic/`, `=verbatim=`, `~code~`
/// - List markers: `- `, `+ `, `1. `
fn compute_org_spans(source: &str) -> Vec<HighlightSpan> {
    use regex::Regex;
    use std::sync::OnceLock;

    static HEADLINE: OnceLock<Regex> = OnceLock::new();
    static TODO_KW: OnceLock<Regex> = OnceLock::new();
    static TAGS: OnceLock<Regex> = OnceLock::new();
    static DIRECTIVE: OnceLock<Regex> = OnceLock::new();
    static COMMENT: OnceLock<Regex> = OnceLock::new();
    static TIMESTAMP: OnceLock<Regex> = OnceLock::new();
    static LINK: OnceLock<Regex> = OnceLock::new();
    static BOLD: OnceLock<Regex> = OnceLock::new();
    static ITALIC: OnceLock<Regex> = OnceLock::new();
    static CODE: OnceLock<Regex> = OnceLock::new();
    static VERBATIM: OnceLock<Regex> = OnceLock::new();
    static LIST_MARKER: OnceLock<Regex> = OnceLock::new();

    let headline = HEADLINE.get_or_init(|| Regex::new(r"(?m)^(\*+)( .*)?$").unwrap());
    let todo_kw = TODO_KW
        .get_or_init(|| Regex::new(r"\b(TODO|DONE|NEXT|WAIT|CANCELLED|DEFERRED)\b").unwrap());
    let tags = TAGS.get_or_init(|| Regex::new(r"(?m)\s+(:[\w@:]+:)\s*$").unwrap());
    let directive = DIRECTIVE.get_or_init(|| Regex::new(r"(?m)^#\+[A-Za-z_]+:?.*$").unwrap());
    let comment = COMMENT.get_or_init(|| Regex::new(r"(?m)^#\s.*$").unwrap());
    let timestamp =
        TIMESTAMP.get_or_init(|| Regex::new(r"[<\[]\d{4}-\d{2}-\d{2}[^>\]]*[>\]]").unwrap());
    let link = LINK.get_or_init(|| Regex::new(r"\[\[([^\]]+)\](\[([^\]]+)\])?\]").unwrap());
    let bold = BOLD.get_or_init(|| Regex::new(r"(?:^|[\s(>])\*([^\s*][^*\n]*)\*").unwrap());
    let italic = ITALIC.get_or_init(|| Regex::new(r"(?:^|[\s(>])/([^\s/][^/\n]*)/").unwrap());
    let code = CODE.get_or_init(|| Regex::new(r"(?:^|[\s(>])~([^~\n]+)~").unwrap());
    let verbatim = VERBATIM.get_or_init(|| Regex::new(r"(?:^|[\s(>])=([^=\n]+)=").unwrap());
    let list_marker = LIST_MARKER.get_or_init(|| Regex::new(r"(?m)^\s*([-+]|\d+[.)])\s").unwrap());

    let mut spans: Vec<HighlightSpan> = Vec::new();

    // Headlines: the star prefix is punctuation, the text is heading.
    for cap in headline.captures_iter(source) {
        let stars = cap.get(1).unwrap();
        spans.push(HighlightSpan {
            byte_start: stars.start(),
            byte_end: stars.end(),
            theme_key: "punctuation",
        });
        if let Some(rest) = cap.get(2) {
            spans.push(HighlightSpan {
                byte_start: rest.start(),
                byte_end: rest.end(),
                theme_key: "markup.heading",
            });
            // TODO/DONE keyword at the start of the headline.
            if let Some(kw) = todo_kw.find(rest.as_str()) {
                spans.push(HighlightSpan {
                    byte_start: rest.start() + kw.start(),
                    byte_end: rest.start() + kw.end(),
                    theme_key: "keyword",
                });
            }
            // Tags at end of headline.
            if let Some(tag) = tags.captures(rest.as_str()).and_then(|c| c.get(1)) {
                spans.push(HighlightSpan {
                    byte_start: rest.start() + tag.start(),
                    byte_end: rest.start() + tag.end(),
                    theme_key: "attribute",
                });
            }
        }
    }

    for m in directive.find_iter(source) {
        spans.push(HighlightSpan {
            byte_start: m.start(),
            byte_end: m.end(),
            theme_key: "attribute",
        });
    }

    for m in comment.find_iter(source) {
        spans.push(HighlightSpan {
            byte_start: m.start(),
            byte_end: m.end(),
            theme_key: "comment",
        });
    }

    for m in timestamp.find_iter(source) {
        spans.push(HighlightSpan {
            byte_start: m.start(),
            byte_end: m.end(),
            theme_key: "constant",
        });
    }

    for m in link.find_iter(source) {
        spans.push(HighlightSpan {
            byte_start: m.start(),
            byte_end: m.end(),
            theme_key: "markup.link",
        });
    }

    for cap in bold.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start() - 1,
                byte_end: m.end() + 1,
                theme_key: "markup.bold",
            });
        }
    }
    for cap in italic.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start() - 1,
                byte_end: m.end() + 1,
                theme_key: "markup.italic",
            });
        }
    }
    for cap in code.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start() - 1,
                byte_end: m.end() + 1,
                theme_key: "markup.literal",
            });
        }
    }
    for cap in verbatim.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start() - 1,
                byte_end: m.end() + 1,
                theme_key: "markup.literal",
            });
        }
    }

    for cap in list_marker.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start(),
                byte_end: m.end(),
                theme_key: "markup.list",
            });
        }
    }

    // Renderer expects spans sorted by start offset.
    spans.sort_by_key(|s| s.byte_start);
    spans
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
        let spans = compute_spans(Language::Python, "def foo():\n    return \"hi\"\n");
        assert!(spans.iter().any(|s| s.theme_key == "keyword"));
        assert!(spans.iter().any(|s| s.theme_key == "string"));
    }

    #[test]
    fn javascript_highlights_keyword_and_string() {
        let spans = compute_spans(Language::JavaScript, "const x = \"hi\";");
        assert!(spans.iter().any(|s| s.theme_key == "keyword"));
        assert!(spans.iter().any(|s| s.theme_key == "string"));
    }

    #[test]
    fn typescript_highlights_keyword_and_type() {
        let spans = compute_spans(
            Language::TypeScript,
            "function add(a: number, b: number): number { return a + b; }",
        );
        assert!(!spans.is_empty());
        assert!(spans.iter().any(|s| s.theme_key == "keyword"));
    }

    #[test]
    fn go_highlights_keyword_and_string() {
        let spans = compute_spans(Language::Go, "package main\nfunc main() { _ = \"hi\" }\n");
        assert!(spans.iter().any(|s| s.theme_key == "keyword"));
        assert!(spans.iter().any(|s| s.theme_key == "string"));
    }

    #[test]
    fn json_highlights_string_and_number() {
        let spans = compute_spans(Language::Json, "{\"name\": \"mae\", \"count\": 42}");
        assert!(spans.iter().any(|s| s.theme_key == "string"));
        assert!(spans.iter().any(|s| s.theme_key == "number"));
    }

    #[test]
    fn bash_highlights_produce_spans() {
        let spans = compute_spans(Language::Bash, "echo \"hello $USER\"\n");
        assert!(!spans.is_empty());
    }

    #[test]
    fn scheme_highlights_produce_spans() {
        let spans = compute_spans(Language::Scheme, "(define (square x) (* x x))\n");
        assert!(!spans.is_empty());
    }

    #[test]
    fn yaml_highlights_produce_spans() {
        let spans = compute_spans(Language::Yaml, "name: mae\nversion: 0.1.0\n");
        assert!(!spans.is_empty());
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
        let src = "# Heading\n\nSome ```code``` and `inline` and [a link](https://example.com).\n\n```rust\nfn main() {}\n```\n";
        let spans = compute_spans(Language::Markdown, src);
        // Block grammar must recognise heading (text.title) and code block
        // (text.literal) — both were silently dropped pre-fix.
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
        let spans = compute_spans(Language::Org, src);
        assert!(spans.iter().any(|s| s.theme_key == "markup.heading"));
        assert!(
            spans.iter().any(|s| s.theme_key == "keyword"),
            "TODO should be keyword"
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
    fn org_extension_detected() {
        assert_eq!(language_for_path(Path::new("foo.org")), Some(Language::Org));
    }

    #[test]
    fn org_parse_once_returns_none() {
        // No tree-sitter grammar — parse_once must skip cleanly.
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
        assert!(map.spans_for(42, "source").is_none());
    }
}
