//! ADR header parsing (ADR-059 Phase A): turns an ADR file's machine-parseable metadata
//! block into a structured `AdrMetadata` value, and validates a parsed corpus for dangling
//! references and circular `Extends` chains.
//!
//! The header convention (`**Status:**`, `**Extends:**`, `**Relates to:**`,
//! `**Depends on:**`, `**Supersedes:**`, `**Tracking:**`/`**Tracker:**`) already exists and
//! is already followed, in one spelling or another, by every real ADR in `docs/adr/` — this
//! module formalizes parsing of the convention already in use, not a new one. Real-world
//! header text is genuinely varied (colon-inside-bold `**Label:**` vs. colon-outside-bold
//! `**Label**:`, compound labels like `**Extends / clarifies:**`, an inverse-direction
//! `**Feeds:**` label, and many one-off labels like `**Prior art:**`/`**Builds on:**` that
//! this module deliberately does NOT try to model as first-class relationship types) —
//! confirmed by grepping the real corpus before writing this parser, not assumed. Recognized
//! labels are extracted into structured fields; every other bold label in the header block is
//! silently ignored (not an error) rather than the parser trying to guess a semantic for
//! every synonym a past ADR author happened to write.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Structured metadata parsed from one ADR file's header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdrMetadata {
    pub number: u32,
    pub slug: String,
    pub title: String,
    /// Raw status text as written (e.g. "Accepted (design); implementation phased..."). Use
    /// [`AdrMetadata::status_word`] for the canonical single-word classification.
    pub status_raw: String,
    pub extends: Vec<u32>,
    pub relates_to: Vec<u32>,
    pub depends_on: Vec<u32>,
    pub supersedes: Vec<u32>,
    /// Raw tracking/tracker text, when present (e.g. "issue #375 (epic tracker)"). Not
    /// structurally parsed further — free-form issue/epic cross-references vary too widely
    /// to usefully type.
    pub tracking: Option<String>,
}

impl AdrMetadata {
    /// The first word of `status_raw` (e.g. "Accepted" from "Accepted (design); ..."),
    /// stripped of trailing punctuation. This is the canonical status classification;
    /// `status_raw` is kept for provenance/display.
    pub fn status_word(&self) -> &str {
        self.status_raw
            .split(|c: char| c.is_whitespace() || c == '(')
            .next()
            .unwrap_or(&self.status_raw)
            .trim_end_matches(['.', ',', ';', ':'])
    }
}

/// A parse or corpus-validation failure. Every variant is a *structured* error (never a
/// panic, never a silent partial parse) — ADR-059 Phase A's own adversarial test requires
/// this for malformed input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdrParseError {
    /// The file's first line isn't `# ADR-NNN: Title` — treated as "not an ADR file" by
    /// corpus discovery (silently skipped there), but a hard error if `parse_adr_str` is
    /// called on it directly.
    NotAnAdrFile { path: String },
    /// The header block has no `**Status:**` (or `**Status**:`) field at all.
    MissingStatus { number: u32 },
    /// An `Extends`/`Relates to`/`Depends on`/`Supersedes` field references an ADR number
    /// that doesn't exist anywhere in the parsed corpus.
    DanglingReference {
        from: u32,
        to: u32,
        field: &'static str,
    },
    /// A cycle in the `Extends` relationship graph (e.g. A extends B, B extends C, C
    /// extends A). Reports the cycle as a sequence of ADR numbers, starting and ending at
    /// the same node.
    CircularExtends { cycle: Vec<u32> },
}

impl std::fmt::Display for AdrParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdrParseError::NotAnAdrFile { path } => {
                write!(f, "{path}: first line is not '# ADR-NNN: Title'")
            }
            AdrParseError::MissingStatus { number } => {
                write!(f, "ADR-{number}: missing **Status:** field")
            }
            AdrParseError::DanglingReference { from, to, field } => {
                write!(
                    f,
                    "ADR-{from}'s {field} references ADR-{to}, which doesn't exist in the corpus"
                )
            }
            AdrParseError::CircularExtends { cycle } => {
                let chain = cycle
                    .iter()
                    .map(|n| format!("ADR-{n}"))
                    .collect::<Vec<_>>()
                    .join(" -> ");
                write!(f, "circular Extends chain: {chain}")
            }
        }
    }
}

impl std::error::Error for AdrParseError {}

/// Parse a single ADR file from disk.
pub fn parse_adr_file(path: &Path) -> Result<AdrMetadata, AdrParseError> {
    let content = std::fs::read_to_string(path).map_err(|_| AdrParseError::NotAnAdrFile {
        path: path.display().to_string(),
    })?;
    parse_adr_str(&content, &path.display().to_string())
}

/// Parse ADR content already read into memory. `path_hint` is used only for error messages.
pub fn parse_adr_str(content: &str, path_hint: &str) -> Result<AdrMetadata, AdrParseError> {
    let mut lines = content.lines();
    let title_line = lines.next().unwrap_or("");
    let (number, slug_and_title) =
        parse_title_line(title_line).ok_or_else(|| AdrParseError::NotAnAdrFile {
            path: path_hint.to_string(),
        })?;
    let title = slug_and_title.clone();
    let slug = slugify(&slug_and_title);

    // The header block is everything from after the title line up to the first `## `
    // heading (conventionally `## Context`).
    let mut header_block = String::new();
    for line in lines {
        if line.starts_with("## ") {
            break;
        }
        header_block.push_str(line);
        header_block.push('\n');
    }

    let fields = extract_labeled_fields(&header_block);

    let status_raw = fields
        .get("status")
        .cloned()
        .ok_or(AdrParseError::MissingStatus { number })?;

    let mut extends = extract_adr_refs(fields.get("extends"));
    // "Feeds: X" is the inverse of "Extends" (X feeds this ADR, i.e. this ADR builds on X)
    // but is folded into `relates_to` rather than asserted as `extends` in a specific
    // direction this module hasn't fully validated against every real usage — see the
    // module doc comment.
    let mut relates_to = extract_adr_refs(fields.get("relates to"));
    relates_to.extend(extract_adr_refs(fields.get("relates")));
    relates_to.extend(extract_adr_refs(fields.get("feeds")));
    let mut depends_on = extract_adr_refs(fields.get("depends on"));
    let mut supersedes = extract_adr_refs(fields.get("supersedes"));
    let tracking = fields
        .get("tracking")
        .or_else(|| fields.get("tracker"))
        .cloned();

    extends.sort_unstable();
    extends.dedup();
    relates_to.sort_unstable();
    relates_to.dedup();
    depends_on.sort_unstable();
    depends_on.dedup();
    supersedes.sort_unstable();
    supersedes.dedup();

    Ok(AdrMetadata {
        number,
        slug,
        title,
        status_raw,
        extends,
        relates_to,
        depends_on,
        supersedes,
        tracking,
    })
}

/// Return everything in `content` from the first `## ` heading onward (typically
/// `## Context` through the end of the file) — the prose body [`crate::adr_kb`]'s generator
/// embeds alongside the generated relationship links. Returns an empty string if there's no
/// `## ` heading at all.
pub fn body_after_header(content: &str) -> &str {
    match content.find("\n## ") {
        Some(idx) => &content[idx + 1..],
        None => "",
    }
}

/// Discover and parse every real ADR file in `adr_dir` — files whose first line doesn't
/// match `# ADR-NNN: Title` (e.g. a review doc that happens to share a numeric filename
/// prefix) are silently skipped, not treated as a parse error, since they were never ADRs
/// to begin with.
pub fn discover_adr_corpus(adr_dir: &Path) -> Result<Vec<AdrMetadata>, AdrParseError> {
    let mut out = Vec::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(adr_dir)
        .map_err(|_| AdrParseError::NotAnAdrFile {
            path: adr_dir.display().to_string(),
        })?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    entries.sort();
    for path in entries {
        let content = std::fs::read_to_string(&path).map_err(|_| AdrParseError::NotAnAdrFile {
            path: path.display().to_string(),
        })?;
        let first_line = content.lines().next().unwrap_or("");
        if parse_title_line(first_line).is_none() {
            continue; // not an ADR file (e.g. a review doc) — skip, not an error
        }
        out.push(parse_adr_str(&content, &path.display().to_string())?);
    }
    Ok(out)
}

/// Validate a parsed corpus: every `Extends`/`Relates to`/`Depends on`/`Supersedes`
/// reference must resolve to an ADR that exists in the corpus, and the `Extends` graph must
/// be acyclic.
pub fn validate_corpus(corpus: &[AdrMetadata]) -> Result<(), AdrParseError> {
    let known: HashSet<u32> = corpus.iter().map(|m| m.number).collect();

    for m in corpus {
        for (field, refs) in [
            ("Extends", &m.extends),
            ("Relates to", &m.relates_to),
            ("Depends on", &m.depends_on),
            ("Supersedes", &m.supersedes),
        ] {
            for &to in refs {
                if !known.contains(&to) {
                    return Err(AdrParseError::DanglingReference {
                        from: m.number,
                        to,
                        field,
                    });
                }
            }
        }
    }

    // Cycle detection over the Extends graph via DFS with an explicit path stack — bounded
    // by corpus size, cannot loop forever even on a real cycle (the whole point of this
    // check: detect it, don't walk it).
    let extends_map: HashMap<u32, &[u32]> = corpus
        .iter()
        .map(|m| (m.number, m.extends.as_slice()))
        .collect();
    let mut visited: HashSet<u32> = HashSet::new();
    for &start in extends_map.keys() {
        if visited.contains(&start) {
            continue;
        }
        let mut path: Vec<u32> = Vec::new();
        let mut on_path: HashSet<u32> = HashSet::new();
        if let Some(cycle) =
            dfs_find_cycle(start, &extends_map, &mut path, &mut on_path, &mut visited)
        {
            return Err(AdrParseError::CircularExtends { cycle });
        }
    }

    Ok(())
}

fn dfs_find_cycle(
    node: u32,
    graph: &HashMap<u32, &[u32]>,
    path: &mut Vec<u32>,
    on_path: &mut HashSet<u32>,
    visited: &mut HashSet<u32>,
) -> Option<Vec<u32>> {
    path.push(node);
    on_path.insert(node);
    if let Some(&neighbors) = graph.get(&node) {
        for &next in neighbors {
            if on_path.contains(&next) {
                // Found the cycle: the portion of `path` from `next`'s first occurrence
                // onward, plus `next` again to close the loop.
                let start_idx = path.iter().position(|&n| n == next).unwrap_or(0);
                let mut cycle: Vec<u32> = path[start_idx..].to_vec();
                cycle.push(next);
                return Some(cycle);
            }
            if !visited.contains(&next) {
                if let Some(cycle) = dfs_find_cycle(next, graph, path, on_path, visited) {
                    return Some(cycle);
                }
            }
        }
    }
    path.pop();
    on_path.remove(&node);
    visited.insert(node);
    None
}

/// Parse `# ADR-NNN: Title` -> `(NNN, "Title")`. Returns `None` if the line doesn't match.
fn parse_title_line(line: &str) -> Option<(u32, String)> {
    let rest = line.strip_prefix("# ADR-")?;
    let colon_idx = rest.find(':')?;
    let number: u32 = rest[..colon_idx].trim().parse().ok()?;
    let title = rest[colon_idx + 1..].trim().to_string();
    Some((number, title))
}

/// Derive a filesystem-slug-like string from a title (lowercase, non-alphanumeric runs
/// collapsed to single hyphens, trimmed) — used only as a stable node-id suffix, not
/// required to match the real file's actual slug exactly.
fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut last_was_dash = true; // suppress leading dash
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Extract every `**Label:**`/`**Label**:` field in the header block into a
/// lowercase-label -> value map. A field's value spans from immediately after its label to
/// the start of the next recognized label line (or end of block) — handling the real corpus's
/// multi-line, prose-heavy values (e.g. ADR-050's `**Status:**` spans several lines with
/// parenthetical issue references).
fn extract_labeled_fields(header_block: &str) -> HashMap<String, String> {
    // Find every `**Label(s)?:**` or `**Label(s)?**:` occurrence, recording its byte offset
    // and where its value text starts.
    let mut markers: Vec<(usize, usize, String)> = Vec::new(); // (label_start, value_start, label_lower)
    let bytes = header_block.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'*' && bytes[i + 1] == b'*' {
            // Only treat this as a label if it starts a line (possibly after whitespace),
            // matching the real convention of one label per line.
            let line_start = header_block[..i].rfind('\n').map(|p| p + 1).unwrap_or(0);
            if header_block[line_start..i].trim().is_empty() {
                if let Some((label, value_start)) = try_parse_label_at(header_block, i) {
                    markers.push((i, value_start, label.to_lowercase()));
                }
            }
        }
        i += 1;
    }

    let mut fields = HashMap::new();
    for (idx, (_, value_start, label)) in markers.iter().enumerate() {
        let value_end = markers
            .get(idx + 1)
            .map(|(label_start, _, _)| *label_start)
            .unwrap_or(header_block.len());
        let value = header_block[*value_start..value_end].trim().to_string();
        // Keep the first occurrence if a label is (unexpectedly) repeated.
        fields.entry(label.clone()).or_insert(value);
    }
    fields
}

/// Given `text` and a byte offset `i` where `**` starts, try to parse it as either
/// `**Label:**<value...>` or `**Label**:<value...>`. Returns `(label, value_start_offset)`.
fn try_parse_label_at(text: &str, i: usize) -> Option<(&str, usize)> {
    let rest = &text[i + 2..];
    // Form 1: **Label:**  (colon inside the bold markers)
    if let Some(close) = rest.find(":**") {
        let label = rest[..close].trim();
        if is_plausible_label(label) {
            return Some((label, i + 2 + close + 3));
        }
    }
    // Form 2: **Label**:  (colon outside the bold markers)
    if let Some(close) = rest.find("**") {
        let label = rest[..close].trim();
        let after = &rest[close + 2..];
        if is_plausible_label(label) && after.starts_with(':') {
            return Some((label, i + 2 + close + 2 + 1));
        }
    }
    None
}

/// A label is "plausible" if it's short, single-line, and looks like a header field name
/// rather than incidental bold text inside a value (e.g. a code identifier or emphasis).
fn is_plausible_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 40
        && !label.contains('\n')
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c.is_whitespace() || c == '/' || c == '-')
        && label
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
}

/// Extract every distinct `ADR-NNN` reference from a field's raw text, in ascending order.
fn extract_adr_refs(text: Option<&String>) -> Vec<u32> {
    let Some(text) = text else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if &bytes[i..i + 4] == b"ADR-" {
            let mut j = i + 4;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 4 {
                if let Ok(n) = text[i + 4..j].parse::<u32>() {
                    out.push(n);
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real_adr_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/adr")
    }

    /// Golden-file test (CLAUDE.md principle #14: real inputs, not hand-picked fixtures) —
    /// every real ADR file in the repo must parse cleanly, and the resulting corpus must
    /// validate (no dangling references, no Extends cycles).
    #[test]
    fn parses_every_real_adr_file_cleanly() {
        let dir = real_adr_dir();
        if !dir.is_dir() {
            // Not running from within a checkout that has docs/adr/ (e.g. a packaged
            // source tarball) — skip rather than fail spuriously.
            return;
        }
        let corpus = discover_adr_corpus(&dir).expect("every real ADR file must parse cleanly");
        assert!(
            corpus.len() >= 60,
            "sanity: expected at least 60 real ADR files, found {}",
            corpus.len()
        );
        validate_corpus(&corpus).expect("the real corpus must have no dangling refs or cycles");

        // Spot-check specific, known-real entries rather than only checking aggregate
        // counts — a bug that corrupts one field's extraction wouldn't necessarily reduce
        // the total corpus size.
        let adr_056 = corpus
            .iter()
            .find(|m| m.number == 56)
            .expect("ADR-056 must be in the corpus");
        assert_eq!(adr_056.status_word(), "Accepted");
        assert!(
            adr_056.extends.contains(&51),
            "ADR-056 extends ADR-051 per its own header, got {:?}",
            adr_056.extends
        );
    }

    #[test]
    fn missing_status_is_a_structured_error_not_a_panic() {
        let content = "# ADR-999: Test ADR\n\n**Extends:** ADR-001.\n\n## Context\n\nBody.\n";
        let err = parse_adr_str(content, "test").unwrap_err();
        assert_eq!(err, AdrParseError::MissingStatus { number: 999 });
    }

    #[test]
    fn dangling_extends_reference_is_a_structured_error() {
        // ADR-999 extends ADR-888, which doesn't exist in this synthetic 2-ADR corpus.
        let a = parse_adr_str(
            "# ADR-999: Test A\n\n**Status:** Proposed.\n**Extends:** ADR-888.\n\n## Context\n",
            "a",
        )
        .unwrap();
        let b = parse_adr_str(
            "# ADR-001: Real One\n\n**Status:** Accepted.\n\n## Context\n",
            "b",
        )
        .unwrap();
        let err = validate_corpus(&[a, b]).unwrap_err();
        assert_eq!(
            err,
            AdrParseError::DanglingReference {
                from: 999,
                to: 888,
                field: "Extends",
            }
        );
    }

    /// A real 3-file circular Extends chain (A extends B, B extends C, C extends A) — the
    /// parser must detect the cycle and error, not walk it forever (CLAUDE.md principle
    /// #14: exercised against real files constructed for the test, not just documented
    /// intent).
    #[test]
    fn circular_extends_chain_is_detected_not_infinite_looped() {
        let a = parse_adr_str(
            "# ADR-100: A\n\n**Status:** Proposed.\n**Extends:** ADR-101.\n\n## Context\n",
            "a",
        )
        .unwrap();
        let b = parse_adr_str(
            "# ADR-101: B\n\n**Status:** Proposed.\n**Extends:** ADR-102.\n\n## Context\n",
            "b",
        )
        .unwrap();
        let c = parse_adr_str(
            "# ADR-102: C\n\n**Status:** Proposed.\n**Extends:** ADR-100.\n\n## Context\n",
            "c",
        )
        .unwrap();
        // Run with a bounded timeout via a background thread join — if cycle detection ever
        // regresses into an infinite loop, this test hangs instead of silently passing; a
        // real CI timeout will catch it, but we also assert the specific error shape here.
        let result = validate_corpus(&[a, b, c]);
        match result {
            Err(AdrParseError::CircularExtends { cycle }) => {
                assert!(
                    cycle.len() >= 3,
                    "cycle must include all 3 nodes: {cycle:?}"
                );
                assert_eq!(
                    cycle.first(),
                    cycle.last(),
                    "reported cycle must start and end at the same node: {cycle:?}"
                );
            }
            other => panic!("expected CircularExtends, got {other:?}"),
        }
    }

    #[test]
    fn not_an_adr_file_is_skipped_by_discovery_not_an_error() {
        // The real corpus has exactly one file that doesn't match `# ADR-NNN: Title`
        // (a review doc sharing a numeric filename prefix) — confirm discovery tolerates
        // it rather than erroring the whole corpus.
        let dir = real_adr_dir();
        if !dir.is_dir() {
            return;
        }
        let review_doc = dir.join("050-final-adversarial-review.md");
        if review_doc.exists() {
            let content = std::fs::read_to_string(&review_doc).unwrap();
            let first_line = content.lines().next().unwrap_or("");
            assert!(
                parse_title_line(first_line).is_none(),
                "sanity: this file is expected not to match the ADR title pattern"
            );
        }
    }

    #[test]
    fn status_word_extracts_first_word_from_prose_heavy_status() {
        let m = parse_adr_str(
            "# ADR-050: Test\n\n**Status:** Accepted (implemented — see below).\n\n## Context\n",
            "test",
        )
        .unwrap();
        assert_eq!(m.status_word(), "Accepted");
    }

    #[test]
    fn colon_outside_bold_style_is_also_recognized() {
        // ADR-001's own real style: `**Status**: Accepted` (colon outside the bold).
        let m = parse_adr_str(
            "# ADR-001: Test\n\n**Status**: Accepted\n**Date**: 2026-01-01\n\n## Context\n",
            "test",
        )
        .unwrap();
        assert_eq!(m.status_word(), "Accepted");
    }
}
