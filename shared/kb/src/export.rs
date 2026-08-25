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
    for (k, v) in &node.properties {
        out.push_str(&format!(":{}: {}\n", k.to_uppercase(), v));
    }
    out.push_str(":END:\n");

    // Title
    out.push_str(&format!("#+title: {}\n", node.title));

    // Tags as filetags
    if !node.tags.is_empty() {
        out.push_str(&format!("#+filetags: :{}: \n", node.tags.join(":")));
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
    let mut chars = body.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '[' && chars.peek() == Some(&'[') {
            chars.next(); // consume second '['
            let mut link_content = String::new();
            let mut depth = 0;
            for ch in chars.by_ref() {
                if ch == ']' {
                    if depth > 0 {
                        depth -= 1;
                        link_content.push(ch);
                    } else {
                        // Consume trailing ']'
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
            // Parse id|display or just id
            if let Some(pipe) = link_content.find('|') {
                let id = &link_content[..pipe];
                let display = &link_content[pipe + 1..];
                result.push_str(&format!("[[{id}][{display}]]"));
            } else {
                result.push_str(&format!("[[{link_content}]]"));
            }
        } else {
            result.push(ch);
        }
    }

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
        assert_eq!(
            convert_links_to_org("See [[concept:buffer|buffers]] for details."),
            "See [[concept:buffer][buffers]] for details."
        );
        assert_eq!(convert_links_to_org("[[simple-link]]"), "[[simple-link]]");
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
