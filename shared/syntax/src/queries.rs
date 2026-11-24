//! Each grammar's highlight queries, and the configuration built from them.

use tree_sitter_highlight::HighlightConfiguration;

use crate::{Grammar, HIGHLIGHT_NAMES};

/// How many levels below a `#;` comment are still painted as comment. A query
/// has no descendant operator, so each level is its own pattern; past this
/// depth a commented-out datum's innermost forms highlight as code again.
/// 0.24's query stopped at four; Scheme config forms nest deeper than that
/// (a `define` holding a `let` holding a `cond` clause holding a call).
pub const SCHEME_SEXP_COMMENT_DEPTH: usize = 8;

/// `(sexp_comment _ @comment)`, `(sexp_comment (_ _* @comment))`, and so on
/// down to `depth` levels: the same shape tree-sitter-scheme's own query uses
/// for `#'` syntax quotes.
fn scheme_sexp_comment_query(depth: usize) -> String {
    let mut query = String::from("(sexp_comment _ @comment)\n");
    for level in 1..depth {
        let open = "(_ ".repeat(level);
        let close = ")".repeat(level);
        query.push_str(&format!("(sexp_comment {open}_* @comment{close})\n"));
    }
    query
}

/// tree-sitter-scheme 0.25's query paints `#;` itself as a comment but lets
/// every datum under it highlight as live code, because the inner symbol/list
/// captures nest inside the outer @comment. 0.24 re-captured the datum's
/// descendants as @comment four levels deep; 0.25 dropped that and kept the
/// same shape only for `#'`. Append it back for `sexp_comment` (last pattern
/// wins), deeper than 0.24 went.
fn scheme_highlights_query() -> &'static str {
    static SCHEME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SCHEME.get_or_init(|| {
        format!(
            "{}\n{}",
            tree_sitter_scheme::HIGHLIGHTS_QUERY,
            scheme_sexp_comment_query(SCHEME_SEXP_COMMENT_DEPTH)
        )
    })
}

/// TypeScript reuses JavaScript highlights as a fallback, appending its own
/// typescript-specific captures on top: the TS crate's query assumes the JS
/// captures are in scope.
fn typescript_highlights_query() -> &'static str {
    static TS_COMBINED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TS_COMBINED.get_or_init(|| {
        format!(
            "{}\n{}",
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_typescript::HIGHLIGHTS_QUERY
        )
    })
}

/// The tree-sitter-cpp highlights query only carries C++-specific captures
/// (templates, qualified ids, …) and assumes the C captures are in scope —
/// the C++ grammar is a superset of C. Concatenate C + C++ highlights, the
/// same combine pattern TypeScript uses over JavaScript.
fn cpp_highlights_query() -> &'static str {
    static CPP_COMBINED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CPP_COMBINED.get_or_init(|| {
        format!(
            "{}\n{}",
            tree_sitter_c::HIGHLIGHT_QUERY,
            tree_sitter_cpp::HIGHLIGHT_QUERY
        )
    })
}

/// A grammar's `(highlights, injections, locals)` queries.
fn queries(grammar: Grammar) -> (&'static str, &'static str, &'static str) {
    match grammar {
        Grammar::Rust => (
            tree_sitter_rust::HIGHLIGHTS_QUERY,
            tree_sitter_rust::INJECTIONS_QUERY,
            "",
        ),
        Grammar::Toml => (tree_sitter_toml_ng::HIGHLIGHTS_QUERY, "", ""),
        Grammar::Markdown => (
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
            tree_sitter_md::INJECTION_QUERY_BLOCK,
            "",
        ),
        Grammar::Python => (tree_sitter_python::HIGHLIGHTS_QUERY, "", ""),
        Grammar::JavaScript => (
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_javascript::INJECTIONS_QUERY,
            tree_sitter_javascript::LOCALS_QUERY,
        ),
        Grammar::TypeScript | Grammar::Tsx => (
            typescript_highlights_query(),
            tree_sitter_javascript::INJECTIONS_QUERY,
            tree_sitter_typescript::LOCALS_QUERY,
        ),
        Grammar::Go => (tree_sitter_go::HIGHLIGHTS_QUERY, "", ""),
        Grammar::C => (tree_sitter_c::HIGHLIGHT_QUERY, "", ""),
        Grammar::Cpp => (cpp_highlights_query(), "", ""),
        Grammar::Ruby => (tree_sitter_ruby::HIGHLIGHTS_QUERY, "", ""),
        Grammar::Json => (tree_sitter_json::HIGHLIGHTS_QUERY, "", ""),
        Grammar::Bash => (tree_sitter_bash::HIGHLIGHT_QUERY, "", ""),
        Grammar::Scheme => (scheme_highlights_query(), "", ""),
        Grammar::Yaml => (tree_sitter_yaml::HIGHLIGHTS_QUERY, "", ""),
        // tree-sitter-hcl ships no highlights query at all (crate or
        // upstream repo) -- see queries/hcl_highlights.scm's own doc
        // comment for the full explanation of this deliberate exception to
        // "no local .scm files."
        Grammar::Terraform => (include_str!("../queries/hcl_highlights.scm"), "", ""),
        Grammar::Dockerfile => (
            tree_sitter_containerfile::HIGHLIGHTS_QUERY,
            tree_sitter_containerfile::INJECTIONS_QUERY,
            "",
        ),
    }
}

/// The highlight configuration for `grammar`, configured with
/// [`HIGHLIGHT_NAMES`]. Building one compiles its queries, which is the
/// expensive part: callers cache the result. `None` when a query does not
/// compile against its grammar.
pub fn build_configuration(grammar: Grammar) -> Option<HighlightConfiguration> {
    let (highlights, injections, locals) = queries(grammar);
    let mut config = HighlightConfiguration::new(
        grammar.language(),
        grammar.name(),
        highlights,
        injections,
        locals,
    )
    .ok()?;
    config.configure(HIGHLIGHT_NAMES);
    Some(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every grammar's queries compile against it: a query that does not
    /// would silently leave its language unhighlighted (`build_configuration`
    /// answers `None`), in the editor and the site alike.
    #[test]
    fn every_grammar_builds_a_configuration() {
        for &g in Grammar::ALL {
            assert!(build_configuration(g).is_some(), "{g:?}");
        }
    }
}
