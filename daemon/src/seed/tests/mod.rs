//! Tests for the seed planner. Every fixture is synthetic text written here.

mod git_tests;
mod glob_tests;
mod plan_tests;
mod prop_tests;
mod split_tests;

use super::*;

/// A page in the canonical form `regen` writes.
pub(super) fn page(
    props: &[(&str, &str)],
    title: Option<&str>,
    tags: Option<&[&str]>,
    body: &str,
) -> String {
    let f = SplitFile {
        id: String::new(),
        properties: props
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        title: title.map(str::to_string),
        filetags: tags.map(|t| t.iter().map(|s| s.to_string()).collect()),
        body: body.to_string(),
        anchors: Vec::new(),
    };
    regen(&f)
}

/// The kinds of the problems `split` reports for `text` (empty when it splits).
pub(super) fn problem_kinds(text: &str) -> Vec<ProblemKind> {
    match split(text) {
        Ok(_) => Vec::new(),
        Err(ps) => ps.into_iter().map(|p| p.kind).collect(),
    }
}
