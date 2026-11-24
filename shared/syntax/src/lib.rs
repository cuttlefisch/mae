//! mae-syntax: the tree-sitter grammars MAE highlights with, the query each
//! is highlighted by, and the configuration builder (ADR-119 D12).
//! @stability: experimental
//!
//! One registry, shared (principle #8): the editor (mae-core's `syntax`
//! module) and the daemon's KB site build their highlighting from it, so a
//! language the editor colours one way the site cannot colour another. The
//! editor's own notions (its `Language`, which also covers Org's regex
//! highlighter, buffer options, theme keys) stay in mae-core; this crate
//! knows grammars only.
//!
//! - [`Grammar`]: every grammar, and the names a source block or a modeline
//!   may give it ([`Grammar::from_name`]).
//! - [`build_configuration`]: a grammar's compiled highlight queries.
//! - [`highlight`]: one text's highlighted regions, each named by the
//!   innermost capture from [`HIGHLIGHT_NAMES`].
//! - [`role`]: the KB site palette's colour role for each of those names.
//!
//! Feature `site-grammars` adds the grammars only the KB site highlights
//! with (PowerShell, INI, SQL, nginx).
//!
//! Adding a language is three steps: the grammar crate in Cargo.toml, a
//! [`Grammar`] variant with its names in `grammar.rs`, and its queries in
//! `queries.rs`.

mod grammar;
mod highlight;
mod queries;
mod role;

pub use grammar::Grammar;
pub use highlight::{highlight, Region};
pub use queries::{build_configuration, SCHEME_SEXP_COMMENT_DEPTH};
pub use role::{role, Role, ROLES};
/// What [`build_configuration`] returns, for a caller that caches it.
pub use tree_sitter_highlight::HighlightConfiguration;

/// Highlight groups we recognize, in the order passed to
/// `HighlightConfiguration::configure`: a [`Region`]'s name is one of these.
///
/// The names here mirror the capture names used by the Helix/tree-sitter
/// highlight queries -- `@keyword`, `@string.special`, etc. -- with the
/// dotted suffixes mostly flattened since our themes currently only
/// style the top-level name. tree-sitter-highlight matches a capture to the
/// longest of these that prefixes it (`@keyword.conditional` is `keyword`).
pub const HIGHLIGHT_NAMES: &[&str] = &[
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
