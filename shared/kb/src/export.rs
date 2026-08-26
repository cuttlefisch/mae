//! KB export — write KB nodes to standard formats.
//!
//! ## Supported Formats
//!
//! - **Org-mode** (v0.11.0): native format, full fidelity (ID, tags, links, properties)
//! - Markdown (roadmap): standard CommonMark with YAML frontmatter
//! - Obsidian (roadmap): Markdown + `[[wikilinks]]` + `#tags`
//! - Notion (roadmap): Markdown + block-level export via API
//!
//! ## Design
//!
//! Export writes to a specified directory. Each node becomes a file named by
//! its slug (`{id}.org` or `{id}.md`). Links are preserved as the target
//! format's native link syntax.

use std::path::Path;

use crate::{KnowledgeBase, Node};

/// Export format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// Org-mode (`.org`) — full fidelity, native format.
    Org,
    /// Markdown (`.md`) — CommonMark with YAML frontmatter.
    Markdown,
}

/// Export report.
#[derive(Debug, Clone, Default)]
pub struct ExportReport {
    pub files_written: usize,
    pub files_skipped: usize,
    pub errors: Vec<(String, String)>,
}

/// Export a KB to a directory in the specified format.
///
/// Each node becomes a separate file. Links are converted to the target
/// format's syntax. Returns a report of files written.
pub fn export_kb(
    kb: &KnowledgeBase,
    output_dir: &Path,
    format: ExportFormat,
    node_ids: Option<&[String]>,
) -> std::io::Result<ExportReport> {
    std::fs::create_dir_all(output_dir)?;
    let mut report = ExportReport::default();

    let ids: Vec<String> = match node_ids {
        Some(ids) => ids.to_vec(),
        None => kb.list_ids(None),
    };

    for id in &ids {
        let Some(node) = kb.get(id) else {
            report.files_skipped += 1;
            continue;
        };

        let content = match format {
            ExportFormat::Org => node_to_org(node),
            ExportFormat::Markdown => node_to_markdown(node),
        };

        let ext = match format {
            ExportFormat::Org => "org",
            ExportFormat::Markdown => "md",
        };
        let filename = format!("{}.{ext}", sanitize_filename(&node.id));
        let path = output_dir.join(&filename);

        match std::fs::write(&path, &content) {
            Ok(()) => report.files_written += 1,
            Err(e) => report.errors.push((node.id.clone(), e.to_string())),
        }
    }

    Ok(report)
}

/// Convert a single node to org-mode format.
pub fn node_to_org(node: &Node) -> String {
    let mut out = String::new();

    // Properties drawer
    out.push_str(":PROPERTIES:\n");
    out.push_str(&format!(":ID: {}\n", node.id));
    // `:KIND:` is what `parse_file_header` reads back. Without it, exporting a
    // `concept:` node and re-importing it made it a `Note` — the kind was parsed
    // IN and never written OUT.
    out.push_str(&format!(":KIND: {}\n", node.kind.as_str()));
    // **Sorted, because `properties` is a `HashMap`.** Unordered iteration made
    // the serialized form nondeterministic: two saves of an unmodified node
    // produced different text, which for the ADR-092 D3 edit surface means a
    // spurious diff and — since that text drives a character-level CRDT diff —
    // a spurious edit broadcast to every peer.
    let mut props: Vec<(&String, &String)> = node.properties.iter().collect();
    props.sort_by(|a, b| a.0.cmp(b.0));
    for (k, v) in props {
        out.push_str(&format!(":{}: {}\n", k.to_uppercase(), v));
    }
    out.push_str(":END:\n");

    // Title
    out.push_str(&format!("#+title: {}\n", node.title));

    // Tags as filetags
    if !node.tags.is_empty() {
        out.push_str(&format!("#+filetags: :{}: \n", node.tags.join(":")));
    }

    // Aliases. Emitted as `#+aliases:` because that is the form the parser
    // reads back (`parse_file_header`); until ADR-092 D3's round trip was
    // measured, they were emitted NOWHERE and simply vanished on export.
    if !node.aliases.is_empty() {
        out.push_str(&format!("#+aliases: {}\n", node.aliases.join(", ")));
    }

    // TODO state + priority would go on heading lines, but these are file-level nodes
    if let Some(ref state) = node.todo_state {
        out.push_str(&format!("#+todo_state: {state}\n"));
    }
    if let Some(pri) = node.priority {
        out.push_str(&format!("#+priority: {pri}\n"));
    }

    out.push('\n');

    // Body — convert [[id|display]] links to org format [[id][display]].
    //
    // `body_after_header` runs first, and it is a no-op for any node ingested
    // after #655. It exists for the ones ingested BEFORE: their bodies still
    // carry the `:PROPERTIES:` drawer and `#+title:` that `parse_org` used to
    // copy in wholesale, so exporting them would emit both a second time. Same
    // function the parser uses, so the two cannot drift (principle #8).
    let body = convert_links_to_org(&crate::org::body_after_header(&node.body));
    out.push_str(&body);
    if !body.ends_with('\n') {
        out.push('\n');
    }

    out
}

/// Convert a single node to Markdown format.
pub fn node_to_markdown(node: &Node) -> String {
    let mut out = String::new();

    // YAML frontmatter
    out.push_str("---\n");
    out.push_str(&format!("id: \"{}\"\n", node.id));
    out.push_str(&format!("title: \"{}\"\n", node.title));
    if !node.tags.is_empty() {
        out.push_str("tags:\n");
        for tag in &node.tags {
            out.push_str(&format!("  - \"{tag}\"\n"));
        }
    }
    if let Some(ref state) = node.todo_state {
        out.push_str(&format!("status: \"{state}\"\n"));
    }
    out.push_str("---\n\n");

    // Title as heading
    out.push_str(&format!("# {}\n\n", node.title));

    // Body — convert [[id|display]] to [display](id)
    let body = convert_links_to_markdown(&node.body);
    out.push_str(&body);
    if !body.ends_with('\n') {
        out.push('\n');
    }

    out
}

/// Convert `[[id|display]]` and `[[id]]` to org-mode `[[id][display]]`.
fn convert_links_to_org(body: &str) -> String {
    let mut result = String::with_capacity(body.len());
    let mut rest = body;

    while let Some(open) = rest.find("[[") {
        result.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find("]]") else {
            // An unterminated `[[` is ordinary text, not a link. Emit the
            // remainder verbatim rather than swallowing it — the previous
            // char-scanner consumed to EOF here and silently deleted it.
            result.push_str(&rest[open..]);
            return result;
        };
        let content = &after[..close];
        rest = &after[close + 2..];

        // **Idempotent.** A body is normally in MAE's internal
        // `[[id|display]]` form, because ingest rewrites org links into it. But
        // a node authored through `kb_create`/`kb_update` can carry a literal
        // org link, and running the internal-form converter over
        // `[[id:x][y]]` produced `[[id:id:x]]y]]` — silent corruption of the
        // user's own text.
        //
        // **The `id:` prefix is load-bearing, not decoration.** Without it the
        // export round trip DESTROYS every internal link: `parse_org` treats a
        // prefix-less `[[n:1][one]]` as an EXTERNAL link and flattens it to
        // plain text (`one (n:1)`), so re-importing an export left a corpus
        // with no graph at all. Measured, not theorised.
        if content.starts_with("id:") {
            result.push_str("[[");
            result.push_str(content);
            result.push_str("]]");
        } else if let Some(pipe) = content.find('|') {
            let id = &content[..pipe];
            let display = &content[pipe + 1..];
            result.push_str(&format!("[[id:{id}][{display}]]"));
        } else {
            result.push_str(&format!("[[id:{content}]]"));
        }
    }
    result.push_str(rest);
    result
}

/// Convert `[[id|display]]` and `[[id]]` to Markdown `[display](id)`.
fn convert_links_to_markdown(body: &str) -> String {
    let mut result = String::with_capacity(body.len());
    let mut chars = body.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '[' && chars.peek() == Some(&'[') {
            chars.next();
            let mut link_content = String::new();
            let mut depth = 0;
            for ch in chars.by_ref() {
                if ch == ']' {
                    if depth > 0 {
                        depth -= 1;
                        link_content.push(ch);
                    } else {
                        let _ = chars.next();
                        break;
                    }
                } else if ch == '[' {
                    depth += 1;
                    link_content.push(ch);
                } else {
                    link_content.push(ch);
                }
            }
            if let Some(pipe) = link_content.find('|') {
                let id = &link_content[..pipe];
                let display = &link_content[pipe + 1..];
                result.push_str(&format!("[{display}]({id})"));
            } else {
                result.push_str(&format!("[{link_content}]({link_content})"));
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Sanitize a node ID for use as a filename (replace `:` with `-`, etc.).
fn sanitize_filename(id: &str) -> String {
    id.chars()
        .map(|c| match c {
            ':' | '/' | '\\' | ' ' => '-',
            c if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' => c,
            _ => '-',
        })
        .collect()
}

/// Compute a FNV-1a hash for KB identity.
pub fn fnv1a_kb_id(name: &str, creator: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in name.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    for &b in creator.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:012x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Node, NodeKind};

    #[test]
    fn node_to_org_basic() {
        let node = Node::new(
            "concept:test",
            "Test Node",
            NodeKind::Concept,
            "Some body text.",
        )
        .with_tags(["core", "design"]);
        let org = node_to_org(&node);
        assert!(org.contains(":ID: concept:test"));
        assert!(org.contains("#+title: Test Node"));
        assert!(org.contains("#+filetags: :core:design:"));
        assert!(org.contains("Some body text."));
    }

    #[test]
    fn node_to_markdown_basic() {
        let node =
            Node::new("concept:test", "Test Node", NodeKind::Concept, "Body.").with_tags(["tag1"]);
        let md = node_to_markdown(&node);
        assert!(md.contains("id: \"concept:test\""));
        assert!(md.contains("title: \"Test Node\""));
        assert!(md.contains("# Test Node"));
        assert!(md.contains("Body."));
    }

    #[test]
    fn convert_links_org() {
        // The `id:` scheme is REQUIRED on the way out. This test previously
        // asserted its absence -- pinning the defect as expected behaviour --
        // and that is exactly why the round trip destroyed links: `parse_org`
        // reads a prefix-less `[[x][d]]` as an EXTERNAL link and flattens it to
        // `d (x)`. See `round_trip_identity_tests`.
        assert_eq!(
            convert_links_to_org("See [[concept:buffer|buffers]] for details."),
            "See [[id:concept:buffer][buffers]] for details."
        );
        assert_eq!(
            convert_links_to_org("[[simple-link]]"),
            "[[id:simple-link]]"
        );
    }

    #[test]
    fn convert_links_markdown() {
        assert_eq!(
            convert_links_to_markdown("See [[concept:buffer|buffers]] for details."),
            "See [buffers](concept:buffer) for details."
        );
        assert_eq!(
            convert_links_to_markdown("[[simple-link]]"),
            "[simple-link](simple-link)"
        );
    }

    #[test]
    fn export_kb_org() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("n1", "Node One", NodeKind::Note, "Body one.").with_tags(["tag1"]));
        kb.insert(Node::new("n2", "Node Two", NodeKind::Note, "Body two."));

        let report = export_kb(&kb, tmp.path(), ExportFormat::Org, None).unwrap();
        assert_eq!(report.files_written, 2);
        assert!(tmp.path().join("n1.org").exists());
        assert!(tmp.path().join("n2.org").exists());
    }

    #[test]
    fn export_kb_markdown() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("n1", "Node One", NodeKind::Note, "Body."));

        let report = export_kb(&kb, tmp.path(), ExportFormat::Markdown, None).unwrap();
        assert_eq!(report.files_written, 1);
        assert!(tmp.path().join("n1.md").exists());
    }

    #[test]
    fn export_subset() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("a", "A", NodeKind::Note, ""));
        kb.insert(Node::new("b", "B", NodeKind::Note, ""));
        kb.insert(Node::new("c", "C", NodeKind::Note, ""));

        let ids = vec!["a".to_string(), "c".to_string()];
        let report = export_kb(&kb, tmp.path(), ExportFormat::Org, Some(&ids)).unwrap();
        assert_eq!(report.files_written, 2);
    }

    #[test]
    fn sanitize_filename_colon() {
        assert_eq!(sanitize_filename("concept:buffer"), "concept-buffer");
        assert_eq!(sanitize_filename("cmd:delete-line"), "cmd-delete-line");
    }

    #[test]
    fn fnv1a_kb_id_deterministic() {
        let id1 = fnv1a_kb_id("research", "alice");
        let id2 = fnv1a_kb_id("research", "alice");
        assert_eq!(id1, id2);

        let id3 = fnv1a_kb_id("research", "bob");
        assert_ne!(id1, id3);
    }
}

/// The org round-trip: `parse_org` → `node_to_org` → `parse_org`.
///
/// **No test in this crate exercised the round trip before #655.** The existing
/// export tests hand-build a clean `Node`, so they structurally cannot see what
/// an *ingested* node carries — which is exactly where the defect lives.
#[cfg(test)]
mod round_trip_tests {
    use crate::org::parse_org;

    const FIXTURE: &str = ":PROPERTIES:\n\
                          :ID: note:alpha\n\
                          :ROLE: reference\n\
                          :END:\n\
                          #+title: Alpha\n\
                          \n\
                          Some prose about alpha.\n";

    /// **#655 falsified.** An ingest → export round trip emits the properties
    /// drawer **twice**: once from `node.properties`, and again inside
    /// `node.body`, because `parse_org` sets `body` to the whole file text.
    ///
    /// This is the three-line reproduction the plan called for, and it decides
    /// the field-authority question empirically rather than by argument: the two
    /// stores are not merely redundant, they are *observably* divergent the
    /// moment anything writes back.
    #[test]
    fn an_ingest_export_round_trip_emits_exactly_one_properties_drawer() {
        let node = parse_org(FIXTURE).expect("fixture has a file-level :ID:");
        let exported = super::node_to_org(&node);
        assert_eq!(
            exported.matches(":PROPERTIES:").count(),
            1,
            "a node must serialize to ONE drawer -- properties are stored twice \
             (#655), so the round trip duplicates them:\n{exported}"
        );
        assert_eq!(
            exported.matches(":END:").count(),
            1,
            "...and one :END: to match:\n{exported}"
        );
    }

    /// The round trip must be **idempotent**: re-parsing the export yields the
    /// same node. Without this, every ingest→export cycle grows the body by one
    /// more drawer.
    #[test]
    fn the_round_trip_is_idempotent() {
        let once = parse_org(FIXTURE).expect("parses");
        let exported = super::node_to_org(&once);
        let twice = parse_org(&exported).expect("the export must itself be parseable");

        assert_eq!(twice.id, once.id);
        assert_eq!(twice.title, once.title);
        assert_eq!(
            twice.properties, once.properties,
            "properties must survive the round trip unchanged"
        );
        assert_eq!(
            twice.body, once.body,
            "the body must be a fixed point -- if it grows, every export cycle \
             accretes another drawer"
        );
    }

    /// The drawer is a *rendering* of `properties`, so a property that exists
    /// only in the structured field must still appear in the export.
    #[test]
    fn a_property_set_only_on_the_struct_is_rendered_into_the_drawer() {
        let mut node = parse_org(FIXTURE).expect("parses");
        node.properties
            .insert("assignee".to_string(), "hayden".to_string());
        let exported = super::node_to_org(&node);
        assert!(
            exported.contains(":ASSIGNEE: hayden"),
            "a structured-only property must be rendered:\n{exported}"
        );
        assert_eq!(exported.matches(":PROPERTIES:").count(), 1);
    }

    /// A node ingested BEFORE #655 still has the drawer inside its body. Its
    /// export must not emit two drawers either — otherwise the fix only helps
    /// content written after it, and every existing KB keeps the defect.
    #[test]
    fn a_legacy_body_that_still_contains_a_drawer_exports_one_drawer() {
        use crate::{Node, NodeKind};
        let mut node = Node::new("note:legacy", "Legacy", NodeKind::Note, FIXTURE);
        node.properties
            .insert("role".to_string(), "reference".to_string());

        let exported = super::node_to_org(&node);
        assert_eq!(
            exported.matches(":PROPERTIES:").count(),
            1,
            "a pre-#655 body carries its own drawer; the export must not add a \
             second:\n{exported}"
        );
        assert!(
            exported.contains("Some prose about alpha."),
            "the actual prose must survive:\n{exported}"
        );
    }
}

/// ADR-092 D3: is the org round-trip **identity**?
///
/// The ADR requires the serialize/parse pair to be *"identity on whichever
/// in-text link grammar it was handed rather than a normaliser"*. R1 sharpened
/// the stakes: whether the export can ever be a **recovery path** (rather than
/// migration-and-rendering only) turns on whether every field survives a round
/// trip — *verified by a test rather than assumed*.
///
/// These tests establish where that stands today, honestly. A property that does
/// not hold is recorded as not holding, not quietly narrowed until it passes.
#[cfg(test)]
mod round_trip_identity_tests {
    use crate::org::parse_org;

    /// Parse → export → parse must be a **fixed point** on every field the
    /// parser reads. This is weaker than byte identity and is the property that
    /// actually matters: a second cycle must not change anything.
    #[test]
    fn the_round_trip_is_a_fixed_point_on_every_parsed_field() {
        for fixture in [
            ":PROPERTIES:\n:ID: n:1\n:END:\n#+title: Plain\n\nJust prose.\n",
            ":PROPERTIES:\n:ID: n:2\n:ROLE: owner\n:END:\n#+title: With props\n\nBody.\n",
            ":PROPERTIES:\n:ID: n:3\n:END:\n#+title: Tagged\n#+filetags: :alpha:beta:\n\nBody.\n",
            ":PROPERTIES:\n:ID: n:4\n:END:\n#+title: Linked\n\nSee [[id:n:1][one]] and [[id:n:2]].\n",
            ":PROPERTIES:\n:ID: n:5\n:END:\n#+title: Unicode — ✎ 日本語\n\nBödy wíth ñon-ASCII.\n",
        ] {
            let once = parse_org(fixture).expect("fixture parses");
            let exported = super::node_to_org(&once);
            let twice = parse_org(&exported).expect("the export must itself parse");

            assert_eq!(twice.id, once.id, "id drifted:\n{exported}");
            assert_eq!(twice.title, once.title, "title drifted:\n{exported}");
            assert_eq!(twice.body, once.body, "body drifted:\n{exported}");
            assert_eq!(twice.tags, once.tags, "tags drifted:\n{exported}");
            assert_eq!(
                twice.properties, once.properties,
                "properties drifted:\n{exported}"
            );

            // ...and a THIRD cycle changes nothing, which is what "fixed point"
            // means. A round trip that converges only after two passes would
            // still corrupt the first export.
            let thrice = super::node_to_org(&twice);
            assert_eq!(
                thrice, exported,
                "the export is not stable across cycles:\n{exported}\n---\n{thrice}"
            );
        }
    }

    /// **The link grammar must survive as authored, not be normalised.**
    ///
    /// ADR-030 makes the in-text grammar the human's edit surface; a serializer
    /// that rewrites `[[id:x][disp]]` into some canonical form is editing the
    /// user's prose behind their back. ADR-092 D3 says so explicitly.
    #[test]
    fn a_link_survives_the_round_trip_in_a_resolvable_form() {
        let src = ":PROPERTIES:\n:ID: n:1\n:END:\n#+title: T\n\nSee [[id:n:2][two]].\n";
        let node = parse_org(src).expect("parses");
        let exported = super::node_to_org(&node);
        let reparsed = parse_org(&exported).expect("re-parses");

        // The parser rewrites `[[id:x][d]]` to its internal `[[x|d]]` form, so
        // byte identity does not hold here -- but the LINK must still resolve to
        // the same target after a round trip, which is the property that matters
        // for a recovery path.
        let links_of = |n: &crate::Node| crate::org::parse_typed_links(&n.body, &n.id);
        let before: Vec<String> = links_of(&node).into_iter().map(|l| l.target).collect();
        let after: Vec<String> = links_of(&reparsed).into_iter().map(|l| l.target).collect();
        assert_eq!(
            before, after,
            "a link's TARGET must survive the round trip:\n{exported}"
        );
        assert_eq!(
            before,
            vec!["n:2".to_string()],
            "and resolve to the real id"
        );
    }
}

/// ADR-092 D3 — the serialize/parse round trip the node edit surface rests on.
///
/// **The human edit surface for a KB node is its normalized org source text**,
/// so `parse_org(node_to_org(n))` must return `n`. Anything it drops is a field
/// a user loses by opening a node and saving it unchanged — the worst failure
/// available, because it looks like nothing happened.
///
/// The round trip is stated as **`parse(serialize(parse(x))) == parse(x)`**,
/// the standard formulation: a hand-built `Node` can hold shapes ingest never
/// produces (uppercase property keys, org-form links in `body`), and asserting
/// against those tests the fixture rather than the code. Anchoring on a *parsed*
/// node fixes the canonical form as the one the corpus actually contains.
#[cfg(test)]
mod org_round_trip_tests {
    use crate::org::{parse_org, parse_org_multi};
    use crate::{Node, NodeKind};

    /// A file whose header exercises every field the serializer emits.
    fn source() -> String {
        "\
:PROPERTIES:
:ID: note:round-trip
:KIND: concept
:ASSIGNEE: hayden
:ROLE: reference
:END:
#+title: A title with: a colon
#+filetags: :alpha:beta:
#+aliases: nickname, other name
#+todo_state: TODO
#+priority: A

Body line one.

Body line two with a [[id:note:other][link]].
"
        .to_string()
    }

    fn assert_same(a: &Node, b: &Node, what: &str) {
        assert_eq!(a.id, b.id, "{what}: id");
        assert_eq!(a.title, b.title, "{what}: title");
        assert_eq!(a.body.trim(), b.body.trim(), "{what}: body");
        assert_eq!(a.tags, b.tags, "{what}: tags");
        assert_eq!(a.kind, b.kind, "{what}: kind");
        assert_eq!(a.todo_state, b.todo_state, "{what}: todo_state");
        assert_eq!(a.priority, b.priority, "{what}: priority");
        assert_eq!(a.aliases, b.aliases, "{what}: aliases");
        assert_eq!(a.properties, b.properties, "{what}: properties");
    }

    /// **The property.** Serializing a parsed node and re-parsing returns it.
    #[test]
    fn serialize_then_parse_is_identity_on_a_parsed_node() {
        let once = parse_org(&source()).expect("the fixture has a file-level :ID:");
        let text = super::node_to_org(&once);
        let twice = parse_org(&text).expect("serialized org must parse back");

        assert_same(&once, &twice, "round trip");
    }

    /// And it is **stable**, not merely equal once — a converter that mangles on
    /// each pass can still look right after one.
    #[test]
    fn a_second_round_trip_changes_nothing_further() {
        let once = parse_org(&source()).unwrap();
        let text_a = super::node_to_org(&once);
        let text_b = super::node_to_org(&parse_org(&text_a).unwrap());

        assert_eq!(
            text_a, text_b,
            "the serialized form must reach a fixed point on the first pass"
        );
    }

    /// The fields the round trip used to lose, named individually so a
    /// regression says *which* one went.
    #[test]
    fn the_previously_lost_fields_survive() {
        let node = parse_org(&super::node_to_org(&parse_org(&source()).unwrap())).unwrap();

        assert_eq!(node.todo_state.as_deref(), Some("TODO"), "todo_state");
        assert_eq!(node.priority, Some('A'), "priority");
        assert_eq!(node.aliases, vec!["nickname", "other name"], "aliases");
        assert_eq!(node.kind, NodeKind::Concept, "kind");
        assert_eq!(
            node.properties.get("assignee").map(String::as_str),
            Some("hayden"),
            "drawer properties"
        );
    }

    /// **The ingest path is the one that matters**, and it was the one dropping
    /// `:KIND:` and `:ALIASES:` — three copies of the file-level construction had
    /// drifted, and the two used by import were the poorer pair.
    #[test]
    fn the_ingest_parser_keeps_what_the_typed_parser_kept() {
        let multi = parse_org_multi(&source());
        let file_level = multi.first().expect("the file-level node");

        assert_eq!(file_level.kind, NodeKind::Concept, "kind reached ingest");
        assert_eq!(file_level.aliases, vec!["nickname", "other name"]);
        assert_eq!(file_level.todo_state.as_deref(), Some("TODO"));
        assert_eq!(file_level.priority, Some('A'));
    }

    /// **The link converter must be idempotent.** A node authored through
    /// `kb_create`/`kb_update` can carry a literal org link, and running the
    /// internal-form converter over it produced `[[id:id:x]]y]]` — silent
    /// corruption of the user's own text.
    #[test]
    fn an_already_org_form_link_is_not_converted_twice() {
        let mut node = Node::new(
            "note:literal",
            "Literal",
            NodeKind::Note,
            "See [[id:note:other][the other]] and [[id:note:bare]].\n",
        );
        node.tags = vec![];

        let once = super::node_to_org(&node);
        assert!(
            once.contains("[[id:note:other][the other]]"),
            "an org-form link must survive verbatim: {once}"
        );
        assert!(
            once.contains("[[id:note:bare]]"),
            "including the display-less form: {once}"
        );
        assert!(
            !once.contains("id:id:"),
            "and must not be double-prefixed: {once}"
        );
    }

    /// The internal form still converts — without this, the idempotence fix
    /// could have been "convert nothing" and every test above would pass.
    #[test]
    fn the_internal_link_form_is_still_converted() {
        let node = Node::new(
            "note:internal",
            "Internal",
            NodeKind::Note,
            "See [[note:other|the other]] and [[note:bare]].\n",
        );

        let out = super::node_to_org(&node);
        assert!(out.contains("[[id:note:other][the other]]"), "{out}");
        assert!(out.contains("[[id:note:bare]]"), "{out}");
    }
}
