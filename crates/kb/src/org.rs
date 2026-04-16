//! Minimal org-roam parser — extracts the subset MAE's KB needs:
//!
//! - `:PROPERTIES: :ID: <uuid> :END:` — node id (required; files without
//!   an ID are skipped so random org files don't pollute the KB).
//! - `#+title: ...` — node title.
//! - `#+filetags: :foo:bar:` — file-level tags.
//! - `[[id:UUID][display]]` / `[[id:UUID]]` — rewritten to the KB's
//!   internal `[[UUID|display]]` / `[[UUID]]` convention so existing
//!   renderer/link code works unchanged.
//!
//! The source `.org` file is **never modified** — this is a read-only
//! derivation. Disk is authoritative; the KB is a derived index.
//!
//! Deliberately small and hand-rolled. When we need richer org support
//! (headings with sub-node `:ID:` drawers, block types, etc.) we can
//! swap in tree-sitter-org without breaking the API.

use crate::{KnowledgeBase, Node, NodeKind};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Result of ingesting a directory: how many files were parsed as nodes
/// and how many were skipped (no `:ID:`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IngestReport {
    pub indexed: usize,
    pub skipped_no_id: usize,
    pub read_errors: Vec<PathBuf>,
}

/// Parse a single org file's text into a `Node` from the *file-level*
/// `:ID:` drawer. Returns `None` if the file has no file-level id.
/// Heading-level ids are parsed by `parse_org_multi`.
pub fn parse_org(content: &str) -> Option<Node> {
    let header = parse_file_header(content);
    let id = header.file_id?;
    let title = header.file_title.unwrap_or_else(|| id.clone());
    let body = rewrite_links(content);
    Some(Node::new(id, title, NodeKind::Note, body).with_tags(header.file_tags))
}

/// Parse an org file into zero or more nodes: the file itself (if it
/// has a file-level `:ID:`) and one node per heading with its own
/// `:PROPERTIES: :ID:` drawer. This is the org-roam model — one big
/// .org file can host many roam nodes.
///
/// Heading nodes have:
/// - `id` from the heading's drawer.
/// - `title` from the heading text (stars + TODO keywords stripped).
/// - `body` = everything between the heading line and the next heading
///   at the same or shallower level (drawers preserved, links rewritten).
/// - `tags` from any trailing `:tag1:tag2:` on the heading line, merged
///   with file-level `#+filetags:`.
pub fn parse_org_multi(content: &str) -> Vec<Node> {
    let header = parse_file_header(content);
    let lines: Vec<&str> = content.lines().collect();
    let mut out: Vec<Node> = Vec::new();

    if let Some(id) = header.file_id.clone() {
        let title = header.file_title.clone().unwrap_or_else(|| id.clone());
        let body = rewrite_links(content);
        out.push(Node::new(id, title, NodeKind::Note, body).with_tags(header.file_tags.clone()));
    }

    // Heading nodes. Find heading boundaries; for each heading with an
    // ID drawer, its body runs up to the next heading of equal-or-shallower
    // level (or EOF).
    let mut headings: Vec<(usize, usize, String, Vec<String>)> = Vec::new();
    for (i, line) in lines.iter().enumerate().skip(header.file_header_end) {
        if let Some(level) = heading_level(line) {
            let (title, inline_tags) = split_heading(line, level);
            headings.push((i, level, title, inline_tags));
        }
    }

    for hi in 0..headings.len() {
        let (start, level, title, inline_tags) = headings[hi].clone();
        let end = headings[(hi + 1)..]
            .iter()
            .find(|(_, l, _, _)| *l <= level)
            .map(|(idx, _, _, _)| *idx)
            .unwrap_or(lines.len());
        let Some(id) = scan_heading_id(&lines[start + 1..end]) else {
            continue;
        };
        let body_raw = lines[start..end].join("\n");
        let body = rewrite_links(&body_raw);
        let mut tags = header.file_tags.clone();
        tags.extend(inline_tags);
        out.push(Node::new(id, title, NodeKind::Note, body).with_tags(tags));
    }

    out
}

struct FileHeader {
    file_id: Option<String>,
    file_title: Option<String>,
    file_tags: Vec<String>,
    file_header_end: usize,
}

fn parse_file_header(content: &str) -> FileHeader {
    let lines: Vec<&str> = content.lines().collect();
    let mut file_id = None;
    let mut file_title = None;
    let mut file_tags = Vec::new();
    let mut in_properties = false;
    let mut file_header_end = 0;

    for (i, line) in lines.iter().enumerate() {
        if heading_level(line).is_some() {
            file_header_end = i;
            return FileHeader {
                file_id,
                file_title,
                file_tags,
                file_header_end,
            };
        }
        file_header_end = i + 1;
        let trimmed = line.trim_start();
        let upper = trimmed.to_ascii_uppercase();
        if upper.starts_with(":PROPERTIES:") {
            in_properties = true;
            continue;
        }
        if in_properties && upper.starts_with(":END:") {
            in_properties = false;
            continue;
        }
        if in_properties {
            if let Some(rest) = trimmed.strip_prefix(':') {
                if let Some((key, value)) = rest.split_once(':') {
                    if key.eq_ignore_ascii_case("ID") {
                        let v = value.trim();
                        if !v.is_empty() {
                            file_id = Some(v.to_string());
                        }
                    }
                }
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("#+") {
            if let Some((key, value)) = rest.split_once(':') {
                match key.to_ascii_lowercase().as_str() {
                    "title" => file_title = Some(value.trim().to_string()),
                    "filetags" | "tags" => {
                        for t in value.split(':') {
                            let t = t.trim();
                            if !t.is_empty() {
                                file_tags.push(t.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    FileHeader {
        file_id,
        file_title,
        file_tags,
        file_header_end,
    }
}

/// Return the level (number of leading stars) of a heading line, or
/// `None` if the line isn't a heading.
fn heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('*') {
        return None;
    }
    let stars = trimmed.chars().take_while(|c| *c == '*').count();
    // Org headings require a space after the stars.
    if trimmed.chars().nth(stars) == Some(' ') && stars > 0 {
        Some(stars)
    } else {
        None
    }
}

/// Split a heading line into (title, trailing-tag-list). Inline tags look
/// like `* Heading text :tag1:tag2:` at end of line.
fn split_heading(line: &str, level: usize) -> (String, Vec<String>) {
    let trimmed = line.trim_start();
    // Skip stars + following space.
    let after_stars = &trimmed[level + 1..];
    // Detect trailing `:tag1:tag2:` by stripping whitespace and matching.
    let s = after_stars.trim_end();
    // Find the last run of `:word:…:` attached to end.
    if let Some(last_space) = s.rfind(char::is_whitespace) {
        let tail = &s[last_space + 1..];
        if is_org_tag_run(tail) {
            let tags: Vec<String> = tail
                .split(':')
                .filter(|t| !t.is_empty())
                .map(|t| t.to_string())
                .collect();
            return (s[..last_space].trim_end().to_string(), tags);
        }
    }
    (s.to_string(), Vec::new())
}

/// Return true if `s` matches `:t1:t2:…:` (alphanumeric + `_-@`).
fn is_org_tag_run(s: &str) -> bool {
    if s.len() < 3 || !s.starts_with(':') || !s.ends_with(':') {
        return false;
    }
    let inner = &s[1..s.len() - 1];
    if inner.is_empty() {
        return false;
    }
    inner.split(':').all(|t| {
        !t.is_empty()
            && t.chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '@')
    })
}

/// Scan the lines immediately after a heading for a `:PROPERTIES: :ID: …
/// :END:` drawer. Returns the ID if present. Only looks at contiguous
/// lines starting right after the heading — if a blank line precedes
/// the drawer it's still considered valid (org tolerates that).
fn scan_heading_id(lines: &[&str]) -> Option<String> {
    let mut in_props = false;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let upper = trimmed.to_ascii_uppercase();
        if i == 0 && !in_props && !upper.starts_with(":PROPERTIES:") && !trimmed.is_empty() {
            // Drawer must be the very first content after the heading.
            return None;
        }
        if upper.starts_with(":PROPERTIES:") {
            in_props = true;
            continue;
        }
        if in_props && upper.starts_with(":END:") {
            return None;
        }
        if in_props {
            if let Some(rest) = trimmed.strip_prefix(':') {
                if let Some((key, value)) = rest.split_once(':') {
                    if key.eq_ignore_ascii_case("ID") {
                        let v = value.trim();
                        if !v.is_empty() {
                            return Some(v.to_string());
                        }
                    }
                }
            }
        }
    }
    None
}

/// Rewrite `[[id:UUID][display]]` / `[[id:UUID]]` → `[[UUID|display]]` /
/// `[[UUID]]` so the KB's existing link scanner sees them as regular
/// internal links. Non-id links (`[[file:…]]`, `[[http://…]]`) are
/// rewritten to `display (url)` form so they don't collide with our
/// internal link scanner (which uses `[[…]]`). If a `[[` has no matching
/// `]]` before a nested `[[`, it's treated as literal text.
pub fn rewrite_links(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < body.len() {
        // `[[` is pure ASCII so byte-indexed lookahead is UTF-8-safe.
        if i + 1 < bytes.len() && bytes[i] == b'[' && bytes[i + 1] == b'[' {
            if let Some(rel_end) = body[i + 2..].find("]]") {
                let inner = &body[i + 2..i + 2 + rel_end];
                // Reject candidates with a nested `[[` — the outer
                // open is almost certainly a stray `[[` in prose.
                if !inner.contains("[[") {
                    let (target_raw, display) = match inner.find("][") {
                        Some(sep) => (&inner[..sep], Some(&inner[sep + 2..])),
                        None => (inner, None),
                    };
                    if let Some(uuid) = target_raw.strip_prefix("id:") {
                        let uuid = uuid.trim();
                        out.push_str("[[");
                        out.push_str(uuid);
                        if let Some(d) = display {
                            out.push('|');
                            out.push_str(d);
                        }
                        out.push_str("]]");
                    } else if let Some(d) = display {
                        // External link — emit "display (target)" so
                        // the brackets don't collide with our scanner.
                        out.push_str(d);
                        out.push_str(" (");
                        out.push_str(target_raw);
                        out.push(')');
                    } else {
                        // Bare external link — emit the URL in parens.
                        out.push('(');
                        out.push_str(target_raw);
                        out.push(')');
                    }
                    i += 2 + rel_end + 2;
                    continue;
                }
                // fallthrough: treat outer `[` as literal.
            }
        }
        // Emit one full UTF-8 char. This keeps multibyte bodies
        // (non-English titles, emoji, etc.) intact.
        let ch = body[i..].chars().next().expect("i < body.len()");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

impl KnowledgeBase {
    /// Walk `dir` recursively, parse every `.org` file, and insert both
    /// the file-level node (if it has `:ID:`) and any heading-level
    /// nodes (headings with their own `:PROPERTIES: :ID:` drawer).
    /// Existing nodes with the same id are overwritten. Returns counts
    /// for reporting to the user.
    pub fn ingest_org_dir(&mut self, dir: impl AsRef<Path>) -> IngestReport {
        let mut report = IngestReport::default();
        let mut seen_ids = HashSet::new();
        for entry in walkdir::WalkDir::new(dir.as_ref())
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("org") {
                continue;
            }
            match std::fs::read_to_string(path) {
                Ok(content) => {
                    let nodes = parse_org_multi(&content);
                    if nodes.is_empty() {
                        report.skipped_no_id += 1;
                        continue;
                    }
                    for node in nodes {
                        if seen_ids.insert(node.id.clone()) {
                            self.insert(node);
                            report.indexed += 1;
                        } else {
                            report.read_errors.push(path.to_path_buf());
                        }
                    }
                }
                Err(_) => report.read_errors.push(path.to_path_buf()),
            }
        }
        report
    }

    /// Re-ingest a single file (for use by the watcher). Inserts every
    /// node found in the file (file-level + heading-level) and returns
    /// the list of upserted ids. Returns an empty vec if nothing parsed.
    pub fn ingest_org_file(&mut self, path: impl AsRef<Path>) -> Vec<String> {
        let Ok(content) = std::fs::read_to_string(path.as_ref()) else {
            return Vec::new();
        };
        let nodes = parse_org_multi(&content);
        let ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
        for n in nodes {
            self.insert(n);
        }
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const SAMPLE: &str = "\
:PROPERTIES:
:ID:       abc-123
:END:
#+title: My Note
#+filetags: :foo:bar:

Body with [[id:def-456][Another]] and [[id:ghi-789]] and
a regular [[https://example.com][link]] that we keep.
";

    #[test]
    fn parse_extracts_id_title_tags() {
        let node = parse_org(SAMPLE).unwrap();
        assert_eq!(node.id, "abc-123");
        assert_eq!(node.title, "My Note");
        assert_eq!(node.tags, vec!["foo", "bar"]);
        assert_eq!(node.kind, NodeKind::Note);
    }

    #[test]
    fn parse_rewrites_id_links_and_flattens_external() {
        let node = parse_org(SAMPLE).unwrap();
        assert!(node.body.contains("[[def-456|Another]]"));
        assert!(node.body.contains("[[ghi-789]]"));
        // External links are rewritten to "display (url)" so they don't
        // collide with our internal [[target]] scanner.
        assert!(node.body.contains("link (https://example.com)"));
        assert!(!node.body.contains("[[https://example.com"));
        // Outgoing links should be only the id-refs.
        assert_eq!(node.links(), vec!["def-456", "ghi-789"]);
    }

    #[test]
    fn parse_returns_none_without_id() {
        assert!(parse_org("#+title: no id\nbody").is_none());
    }

    #[test]
    fn parse_title_defaults_to_id_when_missing() {
        let content = ":PROPERTIES:\n:ID: xyz\n:END:\nbody\n";
        let node = parse_org(content).unwrap();
        assert_eq!(node.id, "xyz");
        assert_eq!(node.title, "xyz");
    }

    #[test]
    fn ingest_dir_walks_org_files() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.org"), SAMPLE).unwrap();
        std::fs::write(
            tmp.path().join("b.org"),
            ":PROPERTIES:\n:ID: x-1\n:END:\n#+title: B\n[[id:abc-123][to-a]]\n",
        )
        .unwrap();
        // A non-org file should be ignored.
        std::fs::write(tmp.path().join("notes.txt"), "not org").unwrap();
        // A file without :ID: should be skipped.
        std::fs::write(tmp.path().join("raw.org"), "#+title: raw\nno id").unwrap();

        let mut kb = KnowledgeBase::new();
        let report = kb.ingest_org_dir(tmp.path());
        assert_eq!(report.indexed, 2);
        assert_eq!(report.skipped_no_id, 1);
        assert!(kb.contains("abc-123"));
        assert!(kb.contains("x-1"));
        // The b.org node links to abc-123 → reverse index must reflect it.
        assert_eq!(kb.links_to("abc-123"), vec!["x-1".to_string()]);
    }

    #[test]
    fn ingest_file_upserts_node() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("a.org");
        std::fs::write(&p, SAMPLE).unwrap();
        let mut kb = KnowledgeBase::new();
        let ids = kb.ingest_org_file(&p);
        assert_eq!(ids, vec!["abc-123"]);
        assert!(kb.contains("abc-123"));
        // Re-ingest must overwrite cleanly.
        std::fs::write(&p, ":PROPERTIES:\n:ID: abc-123\n:END:\n#+title: Renamed\n").unwrap();
        kb.ingest_org_file(&p);
        assert_eq!(kb.get("abc-123").unwrap().title, "Renamed");
    }

    const MULTI: &str = "\
:PROPERTIES:
:ID: file-id
:END:
#+title: Daily notes
#+filetags: :daily:

Intro paragraph.

* First entry :work:
:PROPERTIES:
:ID: first-heading-id
:END:

Body of first entry with [[id:file-id][self]].

** Sub of first
:PROPERTIES:
:ID: sub-id
:END:

Nested body.

* Second entry
:PROPERTIES:
:ID: second-heading-id
:END:

Second body.

* No id here
Just a heading without a drawer.
";

    #[test]
    fn multi_parses_file_and_heading_nodes() {
        let nodes = parse_org_multi(MULTI);
        let ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
        // File node first, then headings in document order.
        assert_eq!(
            ids,
            vec!["file-id", "first-heading-id", "sub-id", "second-heading-id"]
        );
    }

    #[test]
    fn multi_heading_title_strips_inline_tags() {
        let nodes = parse_org_multi(MULTI);
        let first = nodes.iter().find(|n| n.id == "first-heading-id").unwrap();
        assert_eq!(first.title, "First entry");
        // Inline tag "work" should be captured, file-level "daily" inherited.
        assert!(first.tags.contains(&"daily".to_string()));
        assert!(first.tags.contains(&"work".to_string()));
    }

    #[test]
    fn multi_heading_body_scope_ends_at_next_sibling() {
        let nodes = parse_org_multi(MULTI);
        let first = nodes.iter().find(|n| n.id == "first-heading-id").unwrap();
        // "Nested body." is inside the sub-heading, but since sub is a
        // *child* heading its text is included in parent's body slice.
        // (We don't split at deeper levels — that matches org-roam's
        // "heading is a node" scoping.)
        assert!(first.body.contains("Body of first entry"));
        assert!(first.body.contains("Nested body"));
        assert!(!first.body.contains("Second body"));
    }

    #[test]
    fn multi_heading_without_drawer_is_skipped() {
        let nodes = parse_org_multi(MULTI);
        assert!(nodes.iter().all(|n| n.id != "No id here"));
    }

    #[test]
    fn ingest_dir_indexes_heading_nodes() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("daily.org"), MULTI).unwrap();
        let mut kb = KnowledgeBase::new();
        let report = kb.ingest_org_dir(tmp.path());
        assert_eq!(report.indexed, 4, "file + 3 heading ids");
        assert!(kb.contains("file-id"));
        assert!(kb.contains("first-heading-id"));
        assert!(kb.contains("sub-id"));
        assert!(kb.contains("second-heading-id"));
        // And cross-file links still resolve — heading body references file.
        assert!(kb
            .links_to("file-id")
            .contains(&"first-heading-id".to_string()));
    }

    #[test]
    fn rewrite_links_handles_unclosed_brackets() {
        let body = "Here is an [[unclosed bracket and a [[id:xyz][fine]] link.";
        let out = rewrite_links(body);
        assert!(out.contains("[[xyz|fine]]"));
    }

    #[test]
    fn rewrite_links_preserves_multibyte_chars() {
        // Regression: earlier versions used `bytes[i] as char` which
        // mangled non-ASCII characters. Both the body text AND link
        // display text must round-trip cleanly.
        let body = "日本語 body with [[id:xyz][émoji 🎉 link]] and café.";
        let out = rewrite_links(body);
        assert!(out.contains("日本語"));
        assert!(out.contains("café"));
        // External-style brackets become "display (target)".
        assert!(out.contains("émoji 🎉 link (id:xyz)") || out.contains("[[xyz|émoji 🎉 link]]"));
    }
}
