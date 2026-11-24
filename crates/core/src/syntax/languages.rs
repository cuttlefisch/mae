//! Language definitions, tree-sitter configuration, and highlight computation.

use std::collections::HashMap;

use mae_syntax::Grammar;
use tree_sitter::{Parser, Tree};
use tree_sitter_highlight::HighlightConfiguration;

use super::markup::{code_block_byte_ranges, compute_org_spans, MarkupFlavor};
use super::HighlightSpan;

pub(crate) fn highlight_name_to_theme_key(name: &str) -> &'static str {
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
    C,
    Cpp,
    Ruby,
    Terraform,
    Dockerfile,
}

impl Language {
    /// A tree-sitter/display identifier for this variant (used for injection
    /// query names and status/tool-output display, e.g.
    /// `render_common/status.rs`, `tool_impls/syntax.rs`). Deliberately NOT
    /// guaranteed to equal the LSP wire `language_id` a given path resolves
    /// to via `lsp_intent::language_id_from_path` — the two registries are
    /// intentionally decoupled (ADR-075): tree-sitter grammar selection and
    /// LSP server routing are independently-owned concerns, and nothing in
    /// this codebase relies on their strings matching (see e.g. `Tsx.id()`
    /// returning `"tsx"` while `.tsx` files resolve to `"typescript"` for
    /// LSP purposes — both are correct for their own registry).
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
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Ruby => "ruby",
            Language::Terraform => "terraform",
            Language::Dockerfile => "dockerfile",
        }
    }

    /// The markup flavor associated with this language, if any.
    pub fn markup_flavor(self) -> MarkupFlavor {
        match self {
            Language::Markdown => MarkupFlavor::Markdown,
            Language::Org => MarkupFlavor::Org,
            _ => MarkupFlavor::None,
        }
    }

    /// Default buffer-local options for this language (e.g. heading_scale for markup).
    pub fn default_local_options(self) -> crate::buffer::BufferLocalOptions {
        match self {
            Language::Markdown | Language::Org => crate::buffer::BufferLocalOptions {
                heading_scale: Some(true),
                render_markup: Some(true),
                link_descriptive: Some(true),
                word_wrap: Some(true),
                inline_images: Some(true),
                ..Default::default()
            },
            Language::Json | Language::Yaml | Language::Toml => crate::buffer::BufferLocalOptions {
                word_wrap: Some(false),
                ..Default::default()
            },
            _ => crate::buffer::BufferLocalOptions::default(),
        }
    }

    /// The grammar this language is highlighted and parsed with, from the
    /// shared registry (`mae-syntax`). `None` for languages highlighted
    /// through a fallback path (org).
    pub(crate) fn grammar(self) -> Option<Grammar> {
        Some(match self {
            Language::Rust => Grammar::Rust,
            Language::Toml => Grammar::Toml,
            Language::Markdown => Grammar::Markdown,
            Language::Python => Grammar::Python,
            Language::JavaScript => Grammar::JavaScript,
            Language::TypeScript => Grammar::TypeScript,
            Language::Tsx => Grammar::Tsx,
            Language::Go => Grammar::Go,
            Language::Json => Grammar::Json,
            Language::Bash => Grammar::Bash,
            Language::Scheme => Grammar::Scheme,
            Language::Yaml => Grammar::Yaml,
            Language::C => Grammar::C,
            Language::Cpp => Grammar::Cpp,
            Language::Ruby => Grammar::Ruby,
            Language::Terraform => Grammar::Terraform,
            Language::Dockerfile => Grammar::Dockerfile,
            Language::Org => return None,
        })
    }

    /// The editor language a grammar is shown as. `None` for a grammar the
    /// editor does not open buffers in (the KB site's extra grammars, when
    /// `mae-syntax`'s `site-grammars` feature is on).
    pub(crate) fn from_grammar(grammar: Grammar) -> Option<Language> {
        Some(match grammar {
            Grammar::Rust => Language::Rust,
            Grammar::Toml => Language::Toml,
            Grammar::Markdown => Language::Markdown,
            Grammar::Python => Language::Python,
            Grammar::JavaScript => Language::JavaScript,
            Grammar::TypeScript => Language::TypeScript,
            Grammar::Tsx => Language::Tsx,
            Grammar::Go => Language::Go,
            Grammar::Json => Language::Json,
            Grammar::Bash => Language::Bash,
            Grammar::Scheme => Language::Scheme,
            Grammar::Yaml => Language::Yaml,
            Grammar::C => Language::C,
            Grammar::Cpp => Language::Cpp,
            Grammar::Ruby => Language::Ruby,
            Grammar::Terraform => Language::Terraform,
            Grammar::Dockerfile => Language::Dockerfile,
            _ => return None,
        })
    }

    /// Get the tree-sitter `Language` for grammars we support via tree-sitter.
    /// Returns `None` for languages highlighted through a fallback path (org).
    pub(crate) fn ts_language(self) -> Option<tree_sitter::Language> {
        self.grammar().map(Grammar::language)
    }
}

/// The highlight configuration for `lang`, from the shared registry.
/// Org has no tree-sitter grammar compatible with tree-sitter 0.25;
/// `compute_spans` handles it via a regex fallback.
pub(crate) fn build_configuration(lang: Language) -> Option<HighlightConfiguration> {
    mae_syntax::build_configuration(lang.grammar()?)
}

/// Compute highlight spans using a config cache (avoids rebuilding
/// `HighlightConfiguration` on every call).
pub(crate) fn compute_spans_with_cache(
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
    if language == Language::Markdown {
        // Same per-block standalone highlighting approach as org:
        // 1. Highlight structural markdown (headings, emphasis, etc.)
        // 2. Remove markup.literal from code block ranges
        // 3. Add per-block tree-sitter highlighting for each fenced code block
        let mut spans = highlight_with_config(configs.get(&language).unwrap(), source);

        let code_ranges = code_block_byte_ranges(source, MarkupFlavor::Markdown);
        if !code_ranges.is_empty() {
            spans.retain(|s| {
                if s.theme_key != "markup.literal" {
                    return true;
                }
                !code_ranges
                    .iter()
                    .any(|(start, end)| s.byte_start < *end && s.byte_end > *start)
            });
        }

        inject_fenced_code_blocks(source, &mut spans);
        spans.sort_by_key(|s| s.byte_start);
        return spans;
    }
    highlight_with_config(configs.get(&language).unwrap(), source)
}

#[cfg(test)]
pub(crate) fn compute_spans(language: Language, source: &str) -> Vec<HighlightSpan> {
    // Org: regex-based fallback until a tree-sitter-org compatible with
    // tree-sitter 0.25 is available.
    if language == Language::Org {
        return compute_org_spans(source);
    }
    let Some(config) = build_configuration(language) else {
        return Vec::new();
    };
    if language == Language::Markdown {
        // Use the same per-block standalone highlighting approach as org:
        // 1. Highlight structural markdown (headings, emphasis, etc.) without injection
        // 2. Remove markup.literal from code block ranges
        // 3. Add per-block tree-sitter highlighting for each fenced code block
        let mut spans = highlight_with_config(&config, source);

        let code_ranges = code_block_byte_ranges(source, MarkupFlavor::Markdown);
        // Remove markup.literal from code block content (tree-sitter-md marks
        // fenced_code_block content as @text.literal → markup.literal)
        if !code_ranges.is_empty() {
            spans.retain(|s| {
                if s.theme_key != "markup.literal" {
                    return true;
                }
                !code_ranges
                    .iter()
                    .any(|(start, end)| s.byte_start < *end && s.byte_end > *start)
            });
        }

        // Per-block injection: same pattern as org's src block injection (line 1315)
        inject_fenced_code_blocks(source, &mut spans);

        spans.sort_by_key(|s| s.byte_start);
        return spans;
    }
    highlight_with_config(&config, source)
}

/// Compute syntax spans for a single language + source without caching.
/// Used by KB buffers and other contexts needing one-shot highlighting
/// of embedded code blocks.
pub fn compute_spans_standalone(language: Language, source: &str) -> Vec<HighlightSpan> {
    if language == Language::Org {
        return compute_org_spans(source);
    }
    let Some(config) = build_configuration(language) else {
        return Vec::new();
    };
    highlight_with_config(&config, source)
}

pub(crate) fn highlight_with_config(
    config: &HighlightConfiguration,
    source: &str,
) -> Vec<HighlightSpan> {
    let Some(regions) = mae_syntax::highlight(config, source) else {
        return Vec::new();
    };
    regions
        .into_iter()
        .map(|r| HighlightSpan {
            byte_start: r.start,
            byte_end: r.end,
            theme_key: highlight_name_to_theme_key(r.name),
        })
        .collect()
}

/// Inject per-block syntax highlighting for markdown fenced code blocks.
/// Same approach as org's src block injection: regex-find ` ```lang ` / ` ``` `
/// pairs, run `highlight_with_config()` on each block's content, and offset
/// the resulting spans into the full source.
fn inject_fenced_code_blocks(source: &str, spans: &mut Vec<HighlightSpan>) {
    use regex::Regex;
    use std::sync::OnceLock;

    static FENCED_BLOCK: OnceLock<Regex> = OnceLock::new();
    let fenced_block =
        FENCED_BLOCK.get_or_init(|| Regex::new(r"(?m)^```(\w+)\s*\n([\s\S]*?)^```\s*$").unwrap());

    for cap in fenced_block.captures_iter(source) {
        if let (Some(lang_m), Some(content_m)) = (cap.get(1), cap.get(2)) {
            if let Some(lang) = super::detection::language_from_id(lang_m.as_str()) {
                if let Some(config) = build_configuration(lang) {
                    let offset = content_m.start();
                    for mut span in highlight_with_config(&config, content_m.as_str()) {
                        span.byte_start += offset;
                        span.byte_end += offset;
                        spans.push(span);
                    }
                }
            }
        }
    }
}

/// Build a one-shot syntax tree (used primarily in tests).
pub fn parse_once(language: Language, source: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    let ts_lang = language.ts_language()?;
    parser.set_language(&ts_lang).ok()?;
    parser.parse(source, None)
}
