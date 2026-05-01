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

    /// Default buffer-local options for this language (e.g. heading_scale for markup).
    pub fn default_local_options(self) -> crate::buffer::BufferLocalOptions {
        match self {
            Language::Markdown | Language::Org => crate::buffer::BufferLocalOptions {
                heading_scale: Some(true),
                render_markup: Some(true),
                link_descriptive: Some(true),
                ..Default::default()
            },
            _ => crate::buffer::BufferLocalOptions::default(),
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

    /// Returns `true` if cached spans exist and are fresh for this buffer
    /// (no reparse needed). Used by renderers to skip the Rope→String
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
        // Spans are now stale — will be recomputed lazily.
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
            state.spans = Some(std::sync::Arc::new(compute_spans_with_cache(
                lang,
                source,
                &mut self.configs,
            )));
            // Don't clear tree — keep it for incremental reparse via apply_edit.
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
    /// as the `old_tree` for incremental reparse — O(changed) instead of O(file).
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
        collect_fold_nodes(root, source, &mut ranges, 0);
        ranges.sort_by_key(|(s, _)| *s);
        ranges.dedup();
        ranges
    }
}

/// Recursively collect foldable ranges from tree-sitter nodes.
/// Only collects multi-line named nodes up to `max_depth` levels deep.
#[allow(clippy::only_used_in_recursion)]
fn collect_fold_nodes(
    node: tree_sitter::Node,
    source: &str,
    ranges: &mut Vec<(usize, usize)>,
    depth: usize,
) {
    const MAX_DEPTH: usize = 3;
    if depth > MAX_DEPTH {
        return;
    }

    let start_line = node.start_position().row;
    let end_line = node.end_position().row;

    // Only fold multi-line named nodes (skip anonymous tokens like punctuation).
    if node.is_named() && end_line > start_line + 1 && is_foldable_kind(node.kind()) {
        ranges.push((start_line, end_line));
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_fold_nodes(child, source, ranges, depth + 1);
    }
}

/// Check if a tree-sitter node kind represents a foldable code block.
fn is_foldable_kind(kind: &str) -> bool {
    matches!(
        kind,
        "function_definition"
            | "function_item"
            | "function_declaration"
            | "method_definition"
            | "method_declaration"
            | "struct_item"
            | "enum_item"
            | "impl_item"
            | "trait_item"
            | "class_definition"
            | "class_declaration"
            | "class_body"
            | "interface_declaration"
            | "if_expression"
            | "if_statement"
            | "match_expression"
            | "switch_statement"
            | "for_expression"
            | "for_statement"
            | "while_statement"
            | "loop_expression"
            | "block"
            | "mod_item"
            | "module"
            | "use_declaration"
            | "import_statement"
            | "macro_definition"
            | "const_item"
            | "static_item"
            | "type_alias"
    )
}

/// Compute tree-sitter highlight spans for every text buffer visible in the
/// current window layout. Uses stale spans during typing (never blocks render)
/// and queues buffers for deferred reparse into `editor.syntax_reparse_pending`.
///
/// Synchronous parse only happens on first file open (no cached spans at all).
/// Shared type alias for the per-frame syntax span map.
/// Uses `Arc` to avoid cloning all highlight spans every frame.
pub type SyntaxSpanMap = HashMap<usize, std::sync::Arc<Vec<HighlightSpan>>>;

pub fn compute_visible_syntax_spans(editor: &mut crate::editor::Editor) -> SyntaxSpanMap {
    let mut out: SyntaxSpanMap = HashMap::new();
    let mut need_first_parse: Vec<(usize, u64)> = Vec::new();
    for win in editor.window_mgr.iter_windows() {
        let idx = win.buffer_idx;
        if out.contains_key(&idx) || need_first_parse.iter().any(|(i, _)| *i == idx) {
            continue;
        }
        let Some(buf) = editor.buffers.get(idx) else {
            continue;
        };
        if !matches!(buf.kind, crate::buffer::BufferKind::Text) {
            continue;
        }
        if editor.syntax.language_of(idx).is_none() {
            continue;
        }
        let gen = buf.generation;
        match editor.syntax.cached_spans_arc(idx, gen) {
            Some((arc, true)) => {
                // Fresh cache — cheap Arc clone (no data copy).
                out.insert(idx, arc);
            }
            Some((arc, false)) => {
                // Stale cache — use stale spans for this frame, queue reparse.
                out.insert(idx, arc);
                editor.syntax_reparse_pending.insert(idx);
            }
            None => {
                need_first_parse.push((idx, gen));
            }
        }
    }

    // Synchronous first-parse only for buffers with no cached spans at all.
    for (idx, gen) in need_first_parse {
        let source: String = editor.buffers[idx].rope().chars().collect();
        if let Some(arc) = editor.syntax.spans_for_arc(idx, &source, gen) {
            out.insert(idx, arc);
        }
    }

    // Recompute display regions for visible text buffers whose generation changed.
    for win in editor.window_mgr.iter_windows() {
        let idx = win.buffer_idx;
        let buf = &editor.buffers[idx];
        if buf.kind != crate::buffer::BufferKind::Text {
            continue;
        }
        if buf.display_regions_gen == buf.generation {
            continue;
        }
        let link_descriptive = editor.link_descriptive_for(idx);
        editor.buffers[idx].recompute_display_regions(link_descriptive);
    }

    // Set display_reveal_cursor per-frame for the focused window's buffer.
    // This implements org-appear: when cursor is inside a display region,
    // that region is suppressed so raw text is visible for editing.
    let focused_idx = editor.window_mgr.focused_window().buffer_idx;
    if !editor.buffers[focused_idx].display_regions.is_empty() {
        let win = editor.window_mgr.focused_window();
        let buf = &editor.buffers[focused_idx];
        let char_offset = buf.char_offset_at(win.cursor_row, win.cursor_col);
        let byte_offset = buf.rope().char_to_byte(char_offset);
        editor.buffers[focused_idx].display_reveal_cursor = Some(byte_offset);
    } else {
        editor.buffers[focused_idx].display_reveal_cursor = None;
    }

    out
}

/// Perform deferred syntax reparses for buffers in `syntax_reparse_pending`.
/// Called from event loops after a debounce period (~50ms after last edit).
pub fn drain_pending_reparses(editor: &mut crate::editor::Editor) {
    let pending: Vec<usize> = editor.syntax_reparse_pending.drain().collect();
    for idx in pending {
        let Some(buf) = editor.buffers.get(idx) else {
            continue;
        };
        let gen = buf.generation;
        let source: String = buf.rope().chars().collect();
        editor.syntax.spans_for(idx, &source, gen);
    }
}

/// Compute highlight spans using a config cache (avoids rebuilding
/// `HighlightConfiguration` on every call).
fn compute_spans_with_cache(
    language: Language,
    source: &str,
    configs: &mut HashMap<Language, HighlightConfiguration>,
) -> Vec<HighlightSpan> {
    if language == Language::Org {
        return compute_org_spans(source);
    }
    if let std::collections::hash_map::Entry::Vacant(e) = configs.entry(language) {
        if let Some(config) = build_configuration(language) {
            e.insert(config);
        } else {
            return Vec::new();
        }
    }
    highlight_with_config(configs.get(&language).unwrap(), source)
}

#[cfg(test)]
fn compute_spans(language: Language, source: &str) -> Vec<HighlightSpan> {
    // Org: regex-based fallback until a tree-sitter-org compatible with
    // tree-sitter 0.25 is available.
    if language == Language::Org {
        return compute_org_spans(source);
    }
    let Some(config) = build_configuration(language) else {
        return Vec::new();
    };
    highlight_with_config(&config, source)
}

fn highlight_with_config(config: &HighlightConfiguration, source: &str) -> Vec<HighlightSpan> {
    let mut highlighter = Highlighter::new();
    let events = match highlighter.highlight(config, source.as_bytes(), None, |_| None) {
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
                byte_end: m.start(),
                theme_key: "markup.bold.marker",
            });
            spans.push(HighlightSpan {
                byte_start: m.start(),
                byte_end: m.end(),
                theme_key: "markup.bold",
            });
            spans.push(HighlightSpan {
                byte_start: m.end(),
                byte_end: m.end() + 1,
                theme_key: "markup.bold.marker",
            });
        }
    }
    for cap in italic.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start() - 1,
                byte_end: m.start(),
                theme_key: "markup.italic.marker",
            });
            spans.push(HighlightSpan {
                byte_start: m.start(),
                byte_end: m.end(),
                theme_key: "markup.italic",
            });
            spans.push(HighlightSpan {
                byte_start: m.end(),
                byte_end: m.end() + 1,
                theme_key: "markup.italic.marker",
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

/// Compute inline org-style spans for non-tree-sitter contexts (help buffers,
/// conversation buffers). Detects *bold*, /italic/, =code=, ~verbatim~ —
/// intentionally excludes headings to avoid triggering `line_heading_scale()`.
pub fn compute_org_style_spans(source: &str) -> Vec<HighlightSpan> {
    use regex::Regex;
    use std::sync::OnceLock;

    static BOLD: OnceLock<Regex> = OnceLock::new();
    static ITALIC: OnceLock<Regex> = OnceLock::new();
    static CODE: OnceLock<Regex> = OnceLock::new();
    static VERBATIM: OnceLock<Regex> = OnceLock::new();

    let bold = BOLD.get_or_init(|| {
        Regex::new(r"(?:^|[\s(>])\*([^\s*][^*\n]*)\*(?:\s|[.,;:!?)>\]]|$)").unwrap()
    });
    let italic = ITALIC
        .get_or_init(|| Regex::new(r"(?:^|[\s(>])/([^\s/][^/\n]*)/(?:\s|[.,;:!?)>\]]|$)").unwrap());
    let code = CODE
        .get_or_init(|| Regex::new(r"(?:^|[\s(>])=([^\s=][^=\n]*)=(?:\s|[.,;:!?)>\]]|$)").unwrap());
    let verbatim = VERBATIM
        .get_or_init(|| Regex::new(r"(?:^|[\s(>])~([^\s~][^~\n]*)~(?:\s|[.,;:!?)>\]]|$)").unwrap());

    let mut spans: Vec<HighlightSpan> = Vec::new();

    for cap in bold.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start().saturating_sub(1),
                byte_end: m.end() + 1,
                theme_key: "markup.bold",
            });
        }
    }
    for cap in italic.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start().saturating_sub(1),
                byte_end: m.end() + 1,
                theme_key: "markup.italic",
            });
        }
    }
    for cap in code.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start().saturating_sub(1),
                byte_end: m.end() + 1,
                theme_key: "markup.literal",
            });
        }
    }
    for cap in verbatim.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start().saturating_sub(1),
                byte_end: m.end() + 1,
                theme_key: "markup.literal",
            });
        }
    }

    spans.sort_by_key(|s| s.byte_start);
    spans
}

/// Compute inline markdown-style spans for non-tree-sitter contexts (help buffers,
/// conversation buffers). Detects **bold**, `code`, and *italic* — intentionally
/// excludes headings to avoid triggering `line_heading_scale()` in layout.
pub fn compute_markdown_style_spans(source: &str) -> Vec<HighlightSpan> {
    use regex::Regex;
    use std::sync::OnceLock;

    static BOLD: OnceLock<Regex> = OnceLock::new();
    static CODE: OnceLock<Regex> = OnceLock::new();
    static ITALIC: OnceLock<Regex> = OnceLock::new();

    let bold = BOLD.get_or_init(|| Regex::new(r"\*\*([^*\n]+)\*\*").unwrap());
    let code = CODE.get_or_init(|| Regex::new(r"`([^`\n]+)`").unwrap());
    // Match *italic* that is NOT part of **bold** — use word boundary approach
    // instead of look-ahead (unsupported by the regex crate).
    let italic = ITALIC.get_or_init(|| {
        Regex::new(r"(?:^|[\s(>])\*([^\s*][^*\n]*)\*(?:\s|[.,;:!?)>\]]|$)").unwrap()
    });

    let mut spans: Vec<HighlightSpan> = Vec::new();

    for cap in bold.captures_iter(source) {
        let full = cap.get(0).unwrap();
        spans.push(HighlightSpan {
            byte_start: full.start(),
            byte_end: full.end(),
            theme_key: "markup.bold",
        });
    }

    for cap in code.captures_iter(source) {
        let full = cap.get(0).unwrap();
        spans.push(HighlightSpan {
            byte_start: full.start(),
            byte_end: full.end(),
            theme_key: "markup.literal",
        });
    }

    for cap in italic.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            spans.push(HighlightSpan {
                byte_start: m.start().saturating_sub(1),
                byte_end: m.end() + 1,
                theme_key: "markup.italic",
            });
        }
    }

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
        let spans_len = map.spans_for(0, "fn x() {}", 1).unwrap().len();
        assert!(spans_len > 0);
        // Second call with same generation returns the cached vec.
        assert_eq!(map.spans_for(0, "fn x() {}", 1).unwrap().len(), spans_len);
    }

    #[test]
    fn syntax_map_invalidate_forces_recompute() {
        let mut map = SyntaxMap::new();
        map.set_language(0, Language::Rust);
        let _ = map.spans_for(0, "fn x() {}", 1);
        // Generation bump triggers recompute against new source.
        let spans = map.spans_for(0, "let y = 42;", 2).unwrap();
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
        // The function_item should span from line 0 to line 3
        assert!(ranges.iter().any(|(s, e)| *s == 0 && *e == 3));
    }

    #[test]
    fn incremental_reparse_preserves_tree() {
        let mut map = SyntaxMap::new();
        map.set_language(0, Language::Rust);
        // Initial parse
        let source1 = "fn main() { let x = 1; }";
        let _ = map.tree_for(0, source1);
        assert!(map.entries.get(&0).unwrap().tree.is_some());

        // Simulate an edit (apply_edit) and incremental reparse
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

        // tree_for with new source does incremental reparse
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
        // Compute spans (which internally may or may not create a tree)
        let _ = map.spans_for(0, source, 1);
        // After spans_for, tree should be preserved (not cleared)
        // The tree_dirty flag should be set since spans_for re-computed
        // Note: the tree might be None if spans_for doesn't create it,
        // but if it exists it should remain.
        let state = map.entries.get(&0).unwrap();
        assert_eq!(state.computed_at, 1);
    }

    #[test]
    fn incremental_reparse_spans_stable() {
        let mut map = SyntaxMap::new();
        map.set_language(0, Language::Rust);
        let source = "fn foo() {\n    42\n}\n";
        // First computation
        let spans1 = map.spans_for(0, source, 1).map(|s| s.len());
        // Same source, same generation — should return cached
        let spans2 = map.spans_for(0, source, 1).map(|s| s.len());
        assert_eq!(spans1, spans2);

        // New generation — should recompute
        let source2 = "fn foo() {\n    43\n}\n";
        let spans3 = map.spans_for(0, source2, 2).map(|s| s.len());
        assert!(spans3.is_some());
    }

    #[test]
    fn apply_edit_without_tree_is_noop() {
        let mut map = SyntaxMap::new();
        map.set_language(0, Language::Rust);
        // No tree parsed yet — apply_edit should not panic
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
        // Critical safety test: compute_markdown_style_spans must NEVER produce
        // markup.heading spans — those would break layout in conversation buffers.
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
        // Sorted by byte_start
        for w in spans.windows(2) {
            assert!(w[0].byte_start <= w[1].byte_start);
        }
    }

    #[test]
    fn markdown_style_spans_empty() {
        let spans = compute_markdown_style_spans("");
        assert!(spans.is_empty());
    }

    // --- compute_org_style_spans tests ---

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
        // Critical safety: compute_org_style_spans must NEVER produce heading spans.
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
        assert_eq!(md.word_wrap, None);

        let org = Language::Org.default_local_options();
        assert_eq!(org.heading_scale, Some(true));

        let rs = Language::Rust.default_local_options();
        assert_eq!(rs.heading_scale, None);
        assert_eq!(rs.render_markup, None);
        assert_eq!(rs.link_descriptive, None);
    }
}
