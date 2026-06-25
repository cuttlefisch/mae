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
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A parsed link from org content with typed relationship info.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedLink {
    /// Full node ID (e.g., "concept:buffer").
    pub target: String,
    /// Display text.
    pub display: String,
    /// Relationship type (e.g., "teaches", "references").
    pub rel_type: String,
    /// Optional block index or heading slug (from `#fragment`).
    pub fragment: Option<String>,
    /// Relationship weight 0–1 (ADR-030 in-text grammar, default 1.0).
    pub weight: f64,
    /// Relationship confidence 0–1 (default 1.0; lower for AI-inferred links).
    pub confidence: f64,
    /// Unrecognized query attributes, preserved verbatim + in source order
    /// (ADR-030 extensibility: a future `?key=val` needs no grammar change —
    /// parsed here today, read by tomorrow's code). Recognized keys
    /// (rel/w/weight/c/conf/confidence) are NOT duplicated here.
    pub attrs: Vec<(String, String)>,
}

/// Result of parsing an org file, including structured metadata
/// beyond what's stored directly on `Node`.
#[derive(Debug, Clone, Default)]
pub struct OrgParseResult {
    pub nodes: Vec<Node>,
    /// Typed links extracted from node bodies: (source_node_id, ParsedLink).
    pub typed_links: Vec<(String, ParsedLink)>,
    /// Transclusion directives: (meta_node_id, member_id, role).
    pub transclusions: Vec<(String, String, String)>,
}

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
    let mut node = Node::new(id, title, NodeKind::Note, body).with_tags(header.file_tags);
    if !header.file_properties.is_empty() {
        node = node.with_properties(header.file_properties);
    }
    Some(node)
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
        let mut node =
            Node::new(id, title, NodeKind::Note, body).with_tags(header.file_tags.clone());
        if !header.file_properties.is_empty() {
            node = node.with_properties(header.file_properties.clone());
        }
        out.push(node);
    }

    // Heading nodes. Find heading boundaries; for each heading with an
    // ID drawer, its body runs up to the next heading of equal-or-shallower
    // level (or EOF).
    let mut headings: Vec<(usize, usize, HeadingMeta)> = Vec::new();
    for (i, line) in lines.iter().enumerate().skip(header.file_header_end) {
        if let Some(level) = heading_level(line) {
            let meta = split_heading_meta(line, level);
            headings.push((i, level, meta));
        }
    }

    for hi in 0..headings.len() {
        let start = headings[hi].0;
        let level = headings[hi].1;
        let end = headings[(hi + 1)..]
            .iter()
            .find(|(_, l, _)| *l <= level)
            .map(|(idx, _, _)| *idx)
            .unwrap_or(lines.len());
        let (heading_id, heading_props) = scan_heading_properties(&lines[start + 1..end]);
        let Some(id) = heading_id else {
            continue;
        };
        let body_raw = lines[start..end].join("\n");
        let body = rewrite_links(&body_raw);
        let mut tags = header.file_tags.clone();
        tags.extend(headings[hi].2.tags.clone());
        let kind = heading_props
            .get("kind")
            .map(|k| NodeKind::from_str_lossy(k))
            .unwrap_or(NodeKind::Note);
        let mut node = Node::new(id, headings[hi].2.title.clone(), kind, body).with_tags(tags);
        node.todo_state = headings[hi].2.todo_state.clone();
        node.priority = headings[hi].2.priority;
        if !heading_props.is_empty() {
            node.properties = heading_props;
        }
        out.push(node);
    }

    out
}

struct FileHeader {
    file_id: Option<String>,
    file_title: Option<String>,
    file_tags: Vec<String>,
    file_header_end: usize,
    /// All property drawer key-value pairs (lowercased keys, excluding ID).
    file_properties: HashMap<String, String>,
    /// NodeKind from `:KIND:` or `#+KIND:` property.
    kind: Option<NodeKind>,
    /// Aliases from `:ALIASES:` or `#+ALIASES:` property.
    aliases: Vec<String>,
}

fn parse_file_header(content: &str) -> FileHeader {
    let lines: Vec<&str> = content.lines().collect();
    let mut file_id = None;
    let mut file_title = None;
    let mut file_tags = Vec::new();
    let mut file_properties = HashMap::new();
    let mut in_properties = false;
    let mut file_header_end = 0;
    let mut kind = None;
    let mut aliases = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if heading_level(line).is_some() {
            file_header_end = i;
            return FileHeader {
                file_id,
                file_title,
                file_tags,
                file_header_end,
                file_properties,
                kind,
                aliases,
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
                    let v = value.trim();
                    if !v.is_empty() {
                        if key.eq_ignore_ascii_case("ID") {
                            file_id = Some(v.to_string());
                        } else if key.eq_ignore_ascii_case("KIND") {
                            kind = Some(NodeKind::from_str_lossy(v));
                        } else if key.eq_ignore_ascii_case("ALIASES") {
                            aliases = v
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                        } else {
                            file_properties.insert(key.to_ascii_lowercase(), v.to_string());
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
                    "kind" => {
                        kind = Some(NodeKind::from_str_lossy(value.trim()));
                    }
                    "aliases" => {
                        aliases = value
                            .trim()
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
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
        file_properties,
        kind,
        aliases,
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

/// Structured heading metadata extracted from an org heading line.
pub struct HeadingMeta {
    pub title: String,
    pub tags: Vec<String>,
    pub todo_state: Option<String>,
    pub priority: Option<char>,
}

const TODO_KEYWORDS: &[&str] = &["TODO", "DONE", "NEXT", "WAIT", "CANCELLED", "DEFERRED"];

/// Split a heading line into structured metadata.
fn split_heading_meta(line: &str, level: usize) -> HeadingMeta {
    let (title, tags) = split_heading(line, level);

    // Extract TODO keyword and priority from title.
    let mut rest = title.as_str();
    let mut todo_state = None;
    let mut priority = None;

    // Check for TODO keyword at start.
    for kw in TODO_KEYWORDS {
        if rest.starts_with(kw) && rest[kw.len()..].starts_with(' ') {
            todo_state = Some(kw.to_string());
            rest = rest[kw.len() + 1..].trim_start();
            break;
        }
    }

    // Check for priority [#A] / [#B] / [#C].
    if rest.starts_with("[#") && rest.len() >= 4 && rest.as_bytes()[3] == b']' {
        let ch = rest.as_bytes()[2] as char;
        if ch.is_ascii_uppercase() {
            priority = Some(ch);
            rest = rest[4..].trim_start();
        }
    }

    // The clean title is the rest after stripping keyword + priority.
    let clean_title = if todo_state.is_some() || priority.is_some() {
        rest.to_string()
    } else {
        title
    };

    HeadingMeta {
        title: clean_title,
        tags,
        todo_state,
        priority,
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
/// :END:` drawer. Returns the ID and all other properties if present.
/// Only looks at contiguous lines starting right after the heading —
/// if a blank line precedes the drawer it's still considered valid
/// (org tolerates that).
fn scan_heading_properties(lines: &[&str]) -> (Option<String>, HashMap<String, String>) {
    let mut in_props = false;
    let mut id = None;
    let mut props = HashMap::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let upper = trimmed.to_ascii_uppercase();
        if i == 0 && !in_props && !upper.starts_with(":PROPERTIES:") && !trimmed.is_empty() {
            return (None, props);
        }
        if upper.starts_with(":PROPERTIES:") {
            in_props = true;
            continue;
        }
        if in_props && upper.starts_with(":END:") {
            return (id, props);
        }
        if in_props {
            if let Some(rest) = trimmed.strip_prefix(':') {
                if let Some((key, value)) = rest.split_once(':') {
                    let v = value.trim();
                    if !v.is_empty() {
                        if key.eq_ignore_ascii_case("ID") {
                            id = Some(v.to_string());
                        } else {
                            props.insert(key.to_ascii_lowercase(), v.to_string());
                        }
                    }
                }
            }
        }
    }
    (id, props)
}

/// Extract typed links from org content.
///
/// Recognizes org-roam `[[id:UUID][display]]` and the ADR-030 in-text grammar
/// `[[NODE_ID[#FRAGMENT][?rel=X&w=Y&c=Z&…]][display]]`, where all relationship
/// metadata lives as orderless key-value attributes in the target's query (see
/// [`classify_link`]). Absent a query, links default to `rel_type=references`,
/// `weight=confidence=1.0`.
///
/// The `source_id` is the ID of the node containing these links.
pub fn parse_typed_links(body: &str, source_id: &str) -> Vec<ParsedLink> {
    let code_ranges = crate::compute_code_block_ranges(body);
    let in_code = |pos: usize| -> bool { code_ranges.iter().any(|&(s, e)| pos >= s && pos < e) };

    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    let _ = source_id; // used for context, not filtering

    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' && !in_code(i)
            // Skip links inside org verbatim =...= or code ~...~ spans.
            // Emacs does not parse org markup inside these inline spans.
            && !(i > 0 && (bytes[i - 1] == b'=' || bytes[i - 1] == b'~'))
        {
            let link_start = if i + 2 < bytes.len() && bytes[i + 2] == b'[' {
                i + 1
            } else {
                i
            };
            if let Some(rel_end) = body[link_start + 2..].find("]]") {
                let inner = &body[link_start + 2..link_start + 2 + rel_end];
                if inner.contains("[[") {
                    i += 1;
                    continue;
                }
                let (target_raw, display) = match inner.find("][") {
                    Some(sep) => (&inner[..sep], Some(&inner[sep + 2..])),
                    None => (inner, None),
                };

                // Strip id: prefix if present
                let target_str = target_raw.strip_prefix("id:").unwrap_or(target_raw).trim();

                let link_end = link_start + 2 + rel_end + 2; // past the closing ]]

                if !target_str.is_empty() {
                    out.push(classify_link(target_str, display));
                }

                i = link_end;
                continue;
            }
        }
        let ch = body[i..].chars().next().expect("i < body.len()");
        i += ch.len_utf8();
    }
    out
}

/// Split a link target `NODE_ID[#FRAGMENT][?QUERY]` (ADR-030) into its parts.
/// Query is delimited by the first `?`; fragment by the first `#` in the
/// remaining path. Node IDs never contain `?`/`#`, so this is unambiguous.
fn split_link_target(target: &str) -> (&str, Option<String>, Option<&str>) {
    let (path, query) = match target.find('?') {
        Some(p) => (&target[..p], Some(&target[p + 1..])),
        None => (target, None),
    };
    let (node_id, fragment) = match path.find('#') {
        Some(p) => (&path[..p], Some(path[p + 1..].to_string())),
        None => (path, None),
    };
    (node_id, fragment, query)
}

/// Parse a link target's query string `rel=X&w=Y&c=Z&custom=…` (ADR-030) into
/// `(rel_type, weight, confidence, attrs)`. Orderless and extensible:
/// - **Recognized** keys are `rel`, `w`/`weight`, `c`/`conf`/`confidence`
///   (numerics clamped to 0–1; malformed/non-finite values fall back to the
///   default rather than dropping the link).
/// - **Unrecognized** keys are collected verbatim into `attrs` (source order),
///   so custom/future attributes round-trip without a grammar change.
///
/// `rel_type` is returned as `None` when unset (caller defaults to `references`).
fn parse_link_query(query: &str) -> (Option<String>, f64, f64, Vec<(String, String)>) {
    let mut rel_type = None;
    let mut weight = 1.0;
    let mut confidence = 1.0;
    let mut attrs = Vec::new();
    let parse_unit = |v: &str, slot: &mut f64| {
        if let Ok(f) = v.parse::<f64>() {
            if f.is_finite() {
                *slot = f.clamp(0.0, 1.0);
            }
        }
    };
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.split_once('=') {
            Some((k, v)) => (k.trim(), v.trim()),
            None => (pair.trim(), ""),
        };
        match key {
            "rel" => {
                if !value.is_empty() {
                    rel_type = Some(value.to_string());
                }
            }
            "w" | "weight" => parse_unit(value, &mut weight),
            "c" | "conf" | "confidence" => parse_unit(value, &mut confidence),
            "" => {}
            _ => attrs.push((key.to_string(), value.to_string())),
        }
    }
    (rel_type, weight, confidence, attrs)
}

/// Classify a link target into a [`ParsedLink`] per the ADR-030 grammar
/// `NODE_ID[#FRAGMENT][?rel=X&w=Y&c=Z&…]`. Relationship metadata is read from the
/// orderless query (see [`parse_link_query`]); absent it, `rel_type` defaults to
/// `references` and weight/confidence to 1.0. `display` defaults to the bare
/// NODE_ID when the link has no `[display]` part.
fn classify_link(target: &str, display: Option<&str>) -> ParsedLink {
    let (node_id, fragment, query) = split_link_target(target);
    let (rel_type, weight, confidence, attrs) = match query {
        Some(q) => parse_link_query(q),
        None => (None, 1.0, 1.0, Vec::new()),
    };
    ParsedLink {
        target: node_id.to_string(),
        display: display.unwrap_or(node_id).to_string(),
        rel_type: rel_type.unwrap_or_else(|| "references".to_string()),
        fragment,
        weight,
        confidence,
        attrs,
    }
}

/// Rewrite `[[id:UUID][display]]` / `[[id:UUID]]` → `[[UUID|display]]` /
/// `[[UUID]]` so the KB's existing link scanner sees them as regular
/// internal links. Non-id links (`[[file:…]]`, `[[http://…]]`) are
/// rewritten to `display (url)` form so they don't collide with our
/// internal link scanner (which uses `[[…]]`). If a `[[` has no matching
/// `]]` before a nested `[[`, it's treated as literal text.
///
/// Also strips ADR-030 relationship metadata from the rendered link:
/// `[[concept:buffer?rel=teaches&w=0.8][text]]` → `[[concept:buffer|text]]`
/// (the `?query` is projector input, not display — the fragment is kept for
/// node resolution).
pub fn rewrite_links(body: &str) -> String {
    rewrite_links_with_types(body)
}

/// Rewrite links for display, stripping any ADR-030 `?query` metadata from the
/// target (kept only in the canonical source text for the projector).
pub fn rewrite_links_with_types(body: &str) -> String {
    let code_ranges = crate::compute_code_block_ranges(body);
    let in_code_block =
        |pos: usize| -> bool { code_ranges.iter().any(|&(s, e)| pos >= s && pos < e) };

    let mut out = String::with_capacity(body.len());
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < body.len() {
        // `[[` is pure ASCII so byte-indexed lookahead is UTF-8-safe.
        if i + 1 < bytes.len()
            && bytes[i] == b'['
            && bytes[i + 1] == b'['
            && !in_code_block(i)
            // Skip links inside org verbatim =...= or code ~...~ spans
            && !(i > 0 && (bytes[i - 1] == b'=' || bytes[i - 1] == b'~'))
        {
            // Triple-bracket `[[[id:...]]` — skip the stray leading `[`
            // so the inner `[[id:...]]` is parsed as a normal link.
            let link_start = if i + 2 < bytes.len() && bytes[i + 2] == b'[' {
                i + 1
            } else {
                i
            };
            if let Some(rel_end) = body[link_start + 2..].find("]]") {
                let inner = &body[link_start + 2..link_start + 2 + rel_end];
                // If `inner` contains a nested `[[`, the outer brackets
                // are stray. Skip just ONE `[` so the inner link can be
                // parsed on the next iteration.
                if inner.contains("[[") {
                    out.push('[');
                    i += 1;
                    continue;
                }
                {
                    let (target_raw, display) = match inner.find("][") {
                        Some(sep) => (&inner[..sep], Some(&inner[sep + 2..])),
                        None => (inner, None),
                    };
                    // Strip ADR-030 `?query` metadata — display/resolution use the
                    // bare NODE_ID[#FRAGMENT]; the query lives only in source text.
                    let target_clean = match target_raw.find('?') {
                        Some(p) => &target_raw[..p],
                        None => target_raw,
                    };
                    if let Some(uuid) = target_clean.strip_prefix("id:") {
                        let uuid = uuid.trim();
                        out.push_str("[[");
                        // Keep fragment for node resolution.
                        out.push_str(uuid);
                        if let Some(d) = display {
                            out.push('|');
                            out.push_str(d);
                        }
                        out.push_str("]]");
                    } else if is_kb_node_id(target_clean) {
                        // Internal KB link (concept:buffer, key:normal-mode, etc.)
                        // — preserve as [[target|display]] for the help renderer.
                        out.push_str("[[");
                        out.push_str(target_clean);
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
                    i = link_start + 2 + rel_end + 2;
                    continue;
                }
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

/// Known KB node ID namespace prefixes. A link target matching one of these
/// is an internal KB link and should be preserved as `[[target|display]]`.
const KB_NAMESPACES: &[&str] = &[
    "concept:",
    "cmd:",
    "key:",
    "category:",
    "lesson:",
    "module:",
    "option:",
    "scheme:",
    "term:",
    "tutor:",
    "tutorial:",
    "guide:",
    "view:",
    "task:",
    "meta:",
    "index",
    "daily:",
];

/// Check if a link target looks like an internal KB node ID.
fn is_kb_node_id(target: &str) -> bool {
    if target == "index" {
        return true;
    }
    KB_NAMESPACES.iter().any(|ns| target.starts_with(ns))
}

/// Parse an org file into an `OrgParseResult` with typed links and transclusions.
///
/// This is the rich version of `parse_org_multi()` that extracts relationship
/// types from the ADR-030 in-text link grammar and TRANSCLUDE directives.
pub fn parse_org_multi_result(content: &str) -> OrgParseResult {
    let nodes = parse_org_multi_with_types(content);
    let mut typed_links = Vec::new();
    let mut transclusions = Vec::new();

    // Extract typed links from ORIGINAL content (before rewriting strips prefixes).
    // We match links to nodes by parsing the original content per-node.
    let header = parse_file_header(content);
    let lines: Vec<&str> = content.lines().collect();

    // File-level node links: scan the entire file header area
    if let Some(ref file_id) = header.file_id {
        let links = parse_typed_links(content, file_id);
        for link in links {
            typed_links.push((file_id.clone(), link));
        }
    }

    // Heading-level node links: scan each heading's original body
    let mut headings: Vec<(usize, usize, Option<String>)> = Vec::new();
    for (i, line) in lines.iter().enumerate().skip(header.file_header_end) {
        if let Some(level) = heading_level(line) {
            let end_search = &lines[i + 1..];
            let (heading_id, _) = scan_heading_properties(end_search);
            headings.push((i, level, heading_id));
        }
    }
    for hi in 0..headings.len() {
        let Some(ref hid) = headings[hi].2 else {
            continue;
        };
        let start = headings[hi].0;
        let level = headings[hi].1;
        let end = headings[(hi + 1)..]
            .iter()
            .find(|(_, l, _)| *l <= level)
            .map(|(idx, _, _)| *idx)
            .unwrap_or(lines.len());
        let body_raw = lines[start..end].join("\n");
        let links = parse_typed_links(&body_raw, hid);
        for link in links {
            typed_links.push((hid.clone(), link));
        }
    }

    // Deduplicate: file-level links may overlap with heading-level links
    // if the file node body includes heading content. Keep heading-level
    // ones (more specific source) and remove file-level duplicates.
    if let Some(ref file_id) = header.file_id {
        if !headings.is_empty() {
            let heading_ids: HashSet<String> = headings
                .iter()
                .filter_map(|(_, _, id)| id.clone())
                .collect();
            let heading_targets: HashSet<(String, String)> = typed_links
                .iter()
                .filter(|(src, _)| heading_ids.contains(src))
                .map(|(_, link)| (link.target.clone(), link.rel_type.clone()))
                .collect();
            typed_links.retain(|(src, link)| {
                src != file_id
                    || !heading_targets.contains(&(link.target.clone(), link.rel_type.clone()))
            });
        }
    }

    // Parse TRANSCLUDE directives from content (file-level)
    if let Some(ref file_id) = header.file_id {
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed
                .strip_prefix("#+TRANSCLUDE:")
                .or_else(|| trimmed.strip_prefix("#+transclude:"))
            {
                let parts: Vec<&str> = rest.trim().splitn(2, ' ').collect();
                if !parts.is_empty() && !parts[0].is_empty() {
                    let member_id = parts[0].to_string();
                    let role = parts.get(1).unwrap_or(&"content").to_string();
                    transclusions.push((file_id.clone(), member_id, role));
                }
            }
        }
    }

    OrgParseResult {
        nodes,
        typed_links,
        transclusions,
    }
}

/// Like `parse_org_multi` but strips ADR-030 `?query` link metadata from
/// rewritten bodies.
fn parse_org_multi_with_types(content: &str) -> Vec<Node> {
    let header = parse_file_header(content);
    let lines: Vec<&str> = content.lines().collect();
    let mut out: Vec<Node> = Vec::new();

    if let Some(id) = header.file_id.clone() {
        let title = header.file_title.clone().unwrap_or_else(|| id.clone());
        let body = rewrite_links_with_types(content);
        let kind = header.kind.unwrap_or(NodeKind::Note);
        let mut node = Node::new(id, title, kind, body).with_tags(header.file_tags.clone());
        if !header.aliases.is_empty() {
            node = node.with_aliases(header.aliases.iter().map(|s| s.as_str()));
        }
        if !header.file_properties.is_empty() {
            node = node.with_properties(header.file_properties.clone());
        }
        out.push(node);
    }

    // Heading nodes (same logic as parse_org_multi)
    let mut headings: Vec<(usize, usize, HeadingMeta)> = Vec::new();
    for (i, line) in lines.iter().enumerate().skip(header.file_header_end) {
        if let Some(level) = heading_level(line) {
            let meta = split_heading_meta(line, level);
            headings.push((i, level, meta));
        }
    }

    for hi in 0..headings.len() {
        let start = headings[hi].0;
        let level = headings[hi].1;
        let end = headings[(hi + 1)..]
            .iter()
            .find(|(_, l, _)| *l <= level)
            .map(|(idx, _, _)| *idx)
            .unwrap_or(lines.len());
        let (heading_id, heading_props) = scan_heading_properties(&lines[start + 1..end]);
        let Some(id) = heading_id else {
            continue;
        };
        let body_raw = lines[start..end].join("\n");
        let body = rewrite_links_with_types(&body_raw);
        let mut tags = header.file_tags.clone();
        tags.extend(headings[hi].2.tags.clone());
        // Extract :KIND: from heading properties if present
        let kind = heading_props
            .get("kind")
            .map(|k| NodeKind::from_str_lossy(k))
            .unwrap_or(NodeKind::Note);
        let mut node = Node::new(id, headings[hi].2.title.clone(), kind, body).with_tags(tags);
        // Extract :ALIASES: from heading properties if present
        if let Some(aliases_str) = heading_props.get("aliases") {
            let aliases: Vec<&str> = aliases_str
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            if !aliases.is_empty() {
                node = node.with_aliases(aliases);
            }
        }
        node.todo_state = headings[hi].2.todo_state.clone();
        node.priority = headings[hi].2.priority;
        if !heading_props.is_empty() {
            node.properties = heading_props;
        }
        out.push(node);
    }

    out
}

/// Rewrite a single property in an org file's PROPERTIES drawer.
/// If the key exists, update its value. If not, insert before :END:.
/// Returns the modified content string, or None if no PROPERTIES drawer found.
pub fn update_property(content: &str, key: &str, value: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut in_props = false;
    let key_lower = key.to_ascii_lowercase();
    let mut found_key_line = None;
    let mut end_line = None;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let upper = trimmed.to_ascii_uppercase();
        if upper.starts_with(":PROPERTIES:") {
            in_props = true;
            continue;
        }
        if in_props && upper.starts_with(":END:") {
            end_line = Some(i);
            break;
        }
        if in_props {
            if let Some(rest) = trimmed.strip_prefix(':') {
                if let Some((k, _)) = rest.split_once(':') {
                    if k.eq_ignore_ascii_case(&key_lower) {
                        found_key_line = Some(i);
                    }
                }
            }
        }
    }

    let end_line = end_line?; // No valid PROPERTIES drawer → bail

    let mut result = Vec::with_capacity(lines.len() + 1);
    for (i, line) in lines.iter().enumerate() {
        if Some(i) == found_key_line {
            // Replace the existing key line, preserving indentation
            let indent = &line[..line.len() - line.trim_start().len()];
            result.push(format!("{}:{}: {}", indent, key, value));
        } else if found_key_line.is_none() && i == end_line {
            // Key not found — insert before :END:
            let indent = &line[..line.len() - line.trim_start().len()];
            result.push(format!("{}:{}: {}", indent, key, value));
            result.push(line.to_string());
        } else {
            result.push(line.to_string());
        }
    }

    // Preserve trailing newline if original had one
    let mut out = result.join("\n");
    if content.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
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
        for mut n in nodes {
            n.source_file = Some(path.as_ref().to_path_buf());
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
a regular [[https://mae.invalid][link]] that we keep.
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
        assert!(node.body.contains("link (https://mae.invalid)"));
        assert!(!node.body.contains("[[https://mae.invalid"));
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
    fn ingest_org_file_populates_source_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("note.org");
        std::fs::write(&path, SAMPLE).unwrap();
        let mut kb = KnowledgeBase::new();
        let ids = kb.ingest_org_file(&path);
        assert!(!ids.is_empty());
        let node = kb.get(&ids[0]).unwrap();
        assert_eq!(node.source_file.as_deref(), Some(path.as_path()));
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

    #[test]
    fn rewrite_links_triple_bracket() {
        // org-roam sometimes produces `[[[id:UUID][display]]` (extra leading bracket).
        // Should be parsed as a normal id: link.
        let body = "see [[[id:abc-def-123][my note]]].";
        let out = rewrite_links(body);
        assert!(
            out.contains("[[abc-def-123|my note]]"),
            "triple bracket should parse as link, got: {}",
            out
        );
    }

    #[test]
    fn rewrite_links_triple_bracket_bare() {
        let body = "link: [[[id:xyz-123]]] end";
        let out = rewrite_links(body);
        assert!(
            out.contains("[[xyz-123]]"),
            "bare triple bracket should parse, got: {}",
            out
        );
    }

    #[test]
    fn rewrite_links_skips_code_blocks() {
        let body = "\
Text with [[id:abc][real link]].
#+begin_src elisp
(format \"[[id:%s][%s]]\" prev-id prev-title)
#+end_src
After code [[id:def][another link]].";
        let out = rewrite_links(body);
        // Real links outside code blocks should be rewritten.
        assert!(
            out.contains("[[abc|real link]]"),
            "real link missing: {out}"
        );
        assert!(
            out.contains("[[def|another link]]"),
            "post-code link missing: {out}"
        );
        // The code block content should be preserved verbatim.
        assert!(
            out.contains("[[id:%s][%s]]"),
            "code block link should NOT be rewritten: {out}"
        );
    }

    #[test]
    fn rewrite_links_code_block_case_insensitive() {
        let body = "\
#+BEGIN_SRC python
x = \"[[id:fake][link]]\"
#+END_SRC";
        let out = rewrite_links(body);
        assert!(
            out.contains("[[id:fake][link]]"),
            "case-insensitive code block not detected: {out}"
        );
    }

    #[test]
    fn rewrite_links_skips_example_blocks() {
        let body = "\
Text with [[id:abc][real link]].
#+begin_example
:PROPERTIES:
:ID: concept:example
:END:
See [[concept:fake-node]] inside example.
#+end_example
After example [[id:def][another link]].";
        let out = rewrite_links(body);
        assert!(
            out.contains("[[abc|real link]]"),
            "real link before example should be rewritten: {out}"
        );
        assert!(
            out.contains("[[def|another link]]"),
            "link after example should be rewritten: {out}"
        );
        // Content inside #+begin_example should be preserved verbatim
        assert!(
            out.contains("[[concept:fake-node]]"),
            "link inside example block should NOT be rewritten: {out}"
        );
    }

    #[test]
    fn rewrite_links_skips_verbatim_spans() {
        let body = "Real [[id:abc][link]] and =[[id:fake][verbatim]]= end.";
        let out = rewrite_links(body);
        assert!(
            out.contains("[[abc|link]]"),
            "real link should be rewritten: {out}"
        );
        assert!(
            out.contains("=[[id:fake][verbatim]]="),
            "verbatim span link should NOT be rewritten: {out}"
        );
    }

    #[test]
    fn parse_typed_links_skips_example_blocks() {
        let body = "\
[[concept:buffer?rel=teaches][real typed link]]
#+begin_example
[[concept:fake?rel=teaches][inside example]]
#+end_example";
        let links = parse_typed_links(body, "test");
        assert_eq!(
            links.len(),
            1,
            "should only find link outside example block"
        );
        assert_eq!(links[0].target, "concept:buffer");
    }

    #[test]
    fn parse_typed_links_skips_verbatim_spans() {
        let body = "See [[concept:buffer?rel=teaches]] and =[[concept:fake?rel=teaches]]=.";
        let links = parse_typed_links(body, "test");
        assert_eq!(links.len(), 1, "should skip link in verbatim span");
        assert_eq!(links[0].target, "concept:buffer");
    }

    #[test]
    fn parse_captures_all_properties() {
        let content = "\
:PROPERTIES:
:ID:       abc-123
:hash:     deadbeef
:last-modified: 2026-01-15
:last-accessed: 2026-01-14
:END:
#+title: My Note

Body text.
";
        let node = parse_org(content).unwrap();
        assert_eq!(node.id, "abc-123");
        assert_eq!(node.properties.get("hash").unwrap(), "deadbeef");
        assert_eq!(node.properties.get("last-modified").unwrap(), "2026-01-15");
        assert_eq!(node.properties.get("last-accessed").unwrap(), "2026-01-14");
        // ID should NOT be in properties (it's the node id).
        assert!(!node.properties.contains_key("id"));
    }

    #[test]
    fn multi_heading_captures_properties() {
        let content = "\
:PROPERTIES:
:ID: file-id
:hash: filehash
:END:
#+title: Daily

* Entry
:PROPERTIES:
:ID: heading-id
:custom-prop: hello
:END:

Body.
";
        let nodes = parse_org_multi(content);
        let file_node = nodes.iter().find(|n| n.id == "file-id").unwrap();
        assert_eq!(file_node.properties.get("hash").unwrap(), "filehash");
        let heading_node = nodes.iter().find(|n| n.id == "heading-id").unwrap();
        assert_eq!(heading_node.properties.get("custom-prop").unwrap(), "hello");
    }

    #[test]
    fn update_property_inserts_new() {
        let content = "\
:PROPERTIES:
:ID: abc
:END:
#+title: Test
";
        let result = update_property(content, "hash", "deadbeef").unwrap();
        assert!(result.contains(":hash: deadbeef"));
        assert!(result.contains(":END:"));
        // hash should appear before :END:
        let hash_pos = result.find(":hash:").unwrap();
        let end_pos = result.find(":END:").unwrap();
        assert!(hash_pos < end_pos);
    }

    #[test]
    fn update_property_replaces_existing() {
        let content = "\
:PROPERTIES:
:ID: abc
:hash: oldhash
:END:
#+title: Test
";
        let result = update_property(content, "hash", "newhash").unwrap();
        assert!(result.contains(":hash: newhash"));
        assert!(!result.contains("oldhash"));
    }

    #[test]
    fn update_property_returns_none_for_malformed() {
        let content = "#+title: No drawer\nBody text.\n";
        assert!(update_property(content, "hash", "value").is_none());
    }

    #[test]
    fn typed_link_parsing() {
        // ADR-030: rel lives in the target query `?rel=…`.
        let body = "See [[concept:buffer?rel=teaches][Buffer Management]] for details.";
        let links = parse_typed_links(body, "test-node");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].rel_type, "teaches");
        assert_eq!(links[0].target, "concept:buffer");
        assert_eq!(links[0].display, "Buffer Management");
        assert_eq!(links[0].fragment, None);
        // No w/c attributes → weight/confidence default to 1.0.
        assert_eq!(links[0].weight, 1.0);
        assert_eq!(links[0].confidence, 1.0);
    }

    #[test]
    fn link_weight_and_confidence_attributes() {
        // ADR-030: `?rel=…&w=…&c=…` carries the relationship metadata in the target.
        let body =
            "see [[concept:buffer?rel=teaches&w=0.8&c=0.95][the buffer]] then [[concept:plain]]";
        let links = parse_typed_links(body, "src");
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].rel_type, "teaches");
        assert_eq!(links[0].target, "concept:buffer");
        assert_eq!(links[0].display, "the buffer");
        assert_eq!(links[0].weight, 0.8);
        assert_eq!(links[0].confidence, 0.95);
        // The second link has no query → defaults.
        assert_eq!(links[1].target, "concept:plain");
        assert_eq!(links[1].rel_type, "references");
        assert_eq!(links[1].weight, 1.0);
        assert_eq!(links[1].confidence, 1.0);
    }

    #[test]
    fn link_attrs_orderless_clamped_and_extensible() {
        // Keys are orderless (c before w before rel); out-of-range numerics clamp;
        // a malformed numeric falls back to default; unknown keys are preserved
        // verbatim in `attrs` (ADR-030 extensibility) without dropping the link.
        let links = parse_typed_links(
            "[[concept:x?c=0.95&w=0.8&rel=cites]] \
             [[concept:y?w=2.0&c=-0.5&w=bogus]] \
             [[concept:z?rel=cites&since=2026-06&by=ai]]",
            "src",
        );
        assert_eq!(links.len(), 3);
        // Orderless: c/w/rel parsed regardless of position.
        assert_eq!(links[0].rel_type, "cites");
        assert_eq!((links[0].weight, links[0].confidence), (0.8, 0.95));
        // Clamp 2.0→1.0, -0.5→0.0; the later malformed `w=bogus` leaves w at 1.0.
        assert_eq!(links[1].target, "concept:y");
        assert_eq!((links[1].weight, links[1].confidence), (1.0, 0.0));
        // Custom keys round-trip in attrs (source order), link kept.
        assert_eq!(links[2].rel_type, "cites");
        assert_eq!(
            links[2].attrs,
            vec![
                ("since".to_string(), "2026-06".to_string()),
                ("by".to_string(), "ai".to_string()),
            ]
        );
        // Recognized keys are NOT duplicated into attrs.
        assert!(links[2].attrs.iter().all(|(k, _)| k != "rel"));
    }

    #[test]
    fn typed_link_with_fragment() {
        // Grammar order: NODE_ID `#`FRAGMENT then `?`QUERY.
        let body = "See [[concept:rope#architecture?rel=implements][Rope Internals]].";
        let links = parse_typed_links(body, "src");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].rel_type, "implements");
        assert_eq!(links[0].target, "concept:rope");
        assert_eq!(links[0].fragment, Some("architecture".to_string()));
    }

    #[test]
    fn untyped_link_defaults_to_references() {
        let body = "See [[concept:buffer][Buffer Management]].";
        let links = parse_typed_links(body, "src");
        assert_eq!(links.len(), 1);
        // No `?rel=` → defaults to "references"; the colon namespace is part of the id.
        assert_eq!(links[0].rel_type, "references");
        assert_eq!(links[0].target, "concept:buffer");
    }

    #[test]
    fn link_without_display_defaults_to_node_id() {
        // No `[display]` part → display falls back to the bare NODE_ID (query stripped).
        let body = "See [[concept:buffer?rel=teaches&w=0.5]].";
        let links = parse_typed_links(body, "src");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "concept:buffer");
        assert_eq!(links[0].display, "concept:buffer");
        assert_eq!(links[0].weight, 0.5);
    }

    #[test]
    fn rewrite_links_strips_query_metadata() {
        let body = "See [[concept:buffer?rel=teaches&w=0.8][Buffer Management]].";
        let out = rewrite_links(body);
        assert!(
            out.contains("[[concept:buffer|Buffer Management]]"),
            "query metadata should be stripped for display: {out}"
        );
        assert!(
            !out.contains('?'),
            "no query should leak into display: {out}"
        );
    }

    #[test]
    fn rewrite_links_preserves_kb_node_ids() {
        let body = "See [[concept:buffer][Buffer]] and [[cmd:save][Save]] and [[tutorial:getting-started][Getting Started]].";
        let out = rewrite_links(body);
        assert!(
            out.contains("[[concept:buffer|Buffer]]"),
            "concept: link should be preserved: {out}"
        );
        assert!(
            out.contains("[[cmd:save|Save]]"),
            "cmd: link should be preserved: {out}"
        );
        assert!(
            out.contains("[[tutorial:getting-started|Getting Started]]"),
            "tutorial: link should be preserved: {out}"
        );
    }

    #[test]
    fn rewrite_links_preserves_bare_kb_node_ids() {
        let body = "See [[concept:buffer]] and [[index]].";
        let out = rewrite_links(body);
        assert!(
            out.contains("[[concept:buffer]]"),
            "bare concept link should be preserved: {out}"
        );
        assert!(
            out.contains("[[index]]"),
            "bare index link should be preserved: {out}"
        );
    }

    #[test]
    fn kind_property_extraction() {
        let content = "\
:PROPERTIES:
:ID: test-concept
:KIND: concept
:END:
#+title: Test Concept

Body text.
";
        let result = parse_org_multi_result(content);
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].kind, NodeKind::Concept);
    }

    #[test]
    fn aliases_property_extraction() {
        let content = "\
:PROPERTIES:
:ID: test-node
:ALIASES: buffer management, text buffer
:END:
#+title: Buffer

Body text.
";
        let result = parse_org_multi_result(content);
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(
            result.nodes[0].aliases,
            vec!["buffer management", "text buffer"]
        );
    }

    #[test]
    fn keyword_kind_extraction() {
        let content = "\
:PROPERTIES:
:ID: test-lesson
:END:
#+title: Basic Editing
#+KIND: lesson

Body text.
";
        let result = parse_org_multi_result(content);
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].kind, NodeKind::Lesson);
    }

    #[test]
    fn transclude_directive_extraction() {
        let content = "\
:PROPERTIES:
:ID: meta:editor
:END:
#+title: Editor Architecture
#+TRANSCLUDE: concept:buffer content
#+TRANSCLUDE: concept:mode reference

Body text.
";
        let result = parse_org_multi_result(content);
        assert_eq!(result.transclusions.len(), 2);
        assert_eq!(
            result.transclusions[0],
            (
                "meta:editor".to_string(),
                "concept:buffer".to_string(),
                "content".to_string()
            )
        );
        assert_eq!(
            result.transclusions[1],
            (
                "meta:editor".to_string(),
                "concept:mode".to_string(),
                "reference".to_string()
            )
        );
    }

    #[test]
    fn parse_org_multi_result_typed_links() {
        let content = "\
:PROPERTIES:
:ID: lesson:navigation
:KIND: lesson
:END:
#+title: Navigation

Learn about [[concept:buffer?rel=teaches][buffers]] and [[concept:rope?rel=implements][ropes]].
Also see [[concept:window][windows]].
";
        let result = parse_org_multi_result(content);
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.typed_links.len(), 3);

        let teaches: Vec<_> = result
            .typed_links
            .iter()
            .filter(|(_, l)| l.rel_type == "teaches")
            .collect();
        assert_eq!(teaches.len(), 1);
        assert_eq!(teaches[0].1.target, "concept:buffer");

        let implements: Vec<_> = result
            .typed_links
            .iter()
            .filter(|(_, l)| l.rel_type == "implements")
            .collect();
        assert_eq!(implements.len(), 1);
        assert_eq!(implements[0].1.target, "concept:rope");

        let refs: Vec<_> = result
            .typed_links
            .iter()
            .filter(|(_, l)| l.rel_type == "references")
            .collect();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].1.target, "concept:window");
    }
}
