//! `kb_export_subgraph_html` — export a KB subgraph (a seed/anchor node plus
//! its neighborhood) to ONE self-contained, standalone HTML file. Read-then-
//! write, same shape as `guidance_export.rs`'s `kb_export_guidance`: resolve
//! content from the KB, then a single `std::fs::write` — no partial-write/
//! merge concerns here (unlike guidance export, this always fully
//! overwrites `path`; there's no hand-written content to preserve in a
//! generated graph page).
//!
//! Extraction reuses `mae_kb::KnowledgeBase::extract_subgraph` (the same
//! BFS the native KB graph view uses, `crates/core/src/editor/
//! graph_view_ops.rs`); layout reuses `mae_canvas::kb_graph::build_kb_graph`
//! (sunflower seed positions + `ForceLayout::run`, the same pipeline that
//! backs the native graph view's Force layout mode) so this tool doesn't
//! reimplement either. HTML assembly lives entirely in
//! `mae_export::html_graph` — this file is the bridge from `mae-kb`/
//! `mae-canvas` types to that crate's leaf-crate `GraphExportNode`/
//! `GraphExportEdge` shapes (mirrors `crates/core/src/editor/
//! graph_view_ops.rs`'s own `to_kb_nodes`/`to_link_info` bridging for the
//! exact same reason: `mae-canvas` and `mae-export` are deliberately kept
//! free of a `mae-kb` dependency).

use std::collections::HashMap;
use std::path::PathBuf;

use mae_core::Editor;

/// Resolve a possibly-relative output/input path: `~` expansion (matching
/// `execute_create_file`'s convention, `crates/ai/src/tool_impls/file.rs`),
/// then relative-to-project-root if a project is open and the path isn't
/// already absolute, else relative-to-CWD (the plain `PathBuf` behavior) —
/// so this tool is usable both from an open project (the common case) and
/// as a standalone export against an explicit/absolute path.
fn resolve_path(editor: &Editor, raw: &str) -> PathBuf {
    let expanded = mae_core::file_picker::expand_tilde(raw);
    let path = PathBuf::from(&expanded);
    if path.is_absolute() {
        return path;
    }
    match editor.git_or_project_root() {
        Some(root) => root.join(path),
        None => path,
    }
}

/// Find which `KnowledgeBase` (primary, or a registered federated instance)
/// actually contains `id` — `extract_subgraph` is a method on a single
/// in-memory `KnowledgeBase` and never crosses instance boundaries (see
/// `mae_kb::KnowledgeBase::extract_subgraph`'s BFS: it only ever follows
/// `self.nodes`), so the whole exported subgraph is guaranteed to stay
/// within whichever KB `id` resolves to.
fn locate_seed_kb<'a>(editor: &'a Editor, id: &str) -> Result<&'a mae_kb::KnowledgeBase, String> {
    if editor.kb.primary.get(id).is_some() {
        return Ok(&editor.kb.primary);
    }
    for kb in editor.kb.instances.values() {
        if kb.get(id).is_some() {
            return Ok(kb);
        }
    }
    Err(format!(
        "kb_export_subgraph_html: seed node '{id}' was not found in the primary KB or any \
         registered federated instance. Check the id — try kb_search or kb_list first."
    ))
}

/// Export a KB subgraph rooted at `id` to one self-contained HTML file.
///
/// Args: `id` (required, seed/anchor node), `path` (required, output file),
/// `depth` (optional, BFS hop radius, default 2, clamped to 4),
/// `translations` (optional, path to a `{id: {title_es, body_es}}` JSON
/// overlay — see `mae_export::html_graph` module docs), `title` (optional,
/// page `<title>`/`<h1>`, default derived from the seed node's own title).
///
/// Fails with a clear, specific error (never a panic/generic error) when:
/// the seed id doesn't exist anywhere in the KB; an explicitly-given
/// `translations` path can't be read or isn't valid JSON in the expected
/// shape (an OMITTED `translations` arg is never an error — ES fields
/// mirror EN and the page hides the toggle, see module docs); or the output
/// path can't be written. `translations` omitted entirely is always fine.
pub fn execute_kb_export_subgraph_html(
    editor: &Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: id".to_string())?;
    let path_arg = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: path".to_string())?;
    let depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(2)
        .min(4) as usize;

    let kb = locate_seed_kb(editor, id)?;

    let spec = mae_kb::SubgraphSpec {
        starter_nodes: vec![id.to_string()],
        max_depth: depth,
        include_backlinks: true,
        // A generous but real safety net -- this tool targets small (~15-20
        // node) curated exports; a much larger reachable set is almost
        // certainly not what a "subgraph export" call intended.
        node_cap: Some(60),
        include_body: true,
    };
    let result = kb.extract_subgraph(&spec);
    if result.nodes.is_empty() {
        // Shouldn't happen (locate_seed_kb already confirmed `id` resolves
        // in this exact KB, and extract_subgraph always includes a
        // resolving starter node at depth 0) -- kept as a defensive,
        // clearly-worded guard rather than silently emitting an empty page.
        return Err(format!(
            "kb_export_subgraph_html: seed node '{id}' resolved, but subgraph extraction \
             produced zero nodes -- this shouldn't happen; please file an issue."
        ));
    }

    let translations: mae_export::html_graph::TranslationMap =
        match args.get("translations").and_then(|v| v.as_str()) {
            Some(raw) => {
                let resolved = resolve_path(editor, raw);
                mae_export::html_graph::load_translations(&resolved)?
            }
            None => HashMap::new(),
        };

    // --- Bridge to mae-canvas for layout (mirrors graph_view_ops.rs's
    // to_kb_nodes/to_link_info) ---
    let kb_nodes: Vec<mae_canvas::kb_graph::KbNodeInfo> = result
        .nodes
        .iter()
        .map(|n| mae_canvas::kb_graph::KbNodeInfo {
            id: n.id.clone(),
            title: n.title.clone(),
            kind: mae_core::graph_view_support::shared_kind_to_canvas_kind(n.kind),
            is_seed: n.source == Some(mae_kb::NodeSource::Seed),
        })
        .collect();
    let kb_links: Vec<mae_canvas::kb_graph::KbLinkInfo> = result
        .links
        .iter()
        .map(|l| mae_canvas::kb_graph::KbLinkInfo {
            source: l.source.clone(),
            target: l.target.clone(),
            rel_type: l.rel_type.clone(),
            weight: l.weight,
        })
        .collect();
    // Boundary links are dropped for this export (see mae_export::html_graph
    // module docs: v1 is single-KB, read-only, no "... (+N)" stub concept)
    // -- `build_kb_graph` only needs them to seed initial spacing/repulsion
    // around a boundary stub, which doesn't apply here, so `&[]`.
    let scene = mae_canvas::kb_graph::build_kb_graph(&kb_nodes, &kb_links, &[], 1.0);
    let positions: HashMap<String, (f64, f64)> = scene
        .nodes
        .iter()
        .map(|n| (n.id.clone(), (n.x, n.y)))
        .collect();

    // --- Bridge to mae-export for HTML rendering ---
    let palette = mae_export::html_graph::GruvboxPalette::dark();
    let export_nodes: Vec<mae_export::html_graph::GraphExportNode> = result
        .nodes
        .iter()
        .map(|n| {
            let (x, y) = positions.get(&n.id).copied().unwrap_or((0.0, 0.0));
            mae_export::html_graph::build_export_node(
                n.id.clone(),
                n.kind.as_str().to_string(),
                x,
                y,
                n.source == Some(mae_kb::NodeSource::Seed),
                n.id == id,
                &n.title,
                &n.body,
                translations.get(&n.id),
                &palette,
            )
        })
        .collect();
    let export_edges: Vec<mae_export::html_graph::GraphExportEdge> = result
        .links
        .iter()
        .map(|l| mae_export::html_graph::GraphExportEdge {
            source: l.source.clone(),
            target: l.target.clone(),
            rel_type: l.rel_type.clone(),
            weight: l.weight,
        })
        .collect();

    let seed_title = result
        .nodes
        .iter()
        .find(|n| n.id == id)
        .map(|n| n.title.clone())
        .unwrap_or_else(|| id.to_string());
    let page_title = args
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{seed_title} \u{2014} KB Subgraph"));

    let html = mae_export::html_graph::HtmlGraphExporter.export(
        &export_nodes,
        &export_edges,
        id,
        &page_title,
    );

    let out_path = resolve_path(editor, path_arg);
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "kb_export_subgraph_html: couldn't create directory {}: {e}",
                    parent.display()
                )
            })?;
        }
    }
    std::fs::write(&out_path, &html).map_err(|e| {
        format!(
            "kb_export_subgraph_html: couldn't write {}: {e}",
            out_path.display()
        )
    })?;

    Ok(format!(
        "Exported {} node{} ({} edges) rooted at '{id}' to {} ({} bytes){}",
        export_nodes.len(),
        if export_nodes.len() == 1 { "" } else { "s" },
        export_edges.len(),
        out_path.display(),
        html.len(),
        if translations.is_empty() {
            String::new()
        } else {
            format!(", {} translation(s) applied", translations.len())
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor_with_linked_notes() -> Editor {
        let mut editor = Editor::new();
        editor.kb.primary.insert(mae_kb::Node::new(
            "root",
            "Root Note",
            mae_kb::NodeKind::Note,
            "The root. See [[child][Child]].",
        ));
        editor.kb.primary.insert(mae_kb::Node::new(
            "child",
            "Child Note",
            mae_kb::NodeKind::Note,
            "A child note.",
        ));
        editor
    }

    #[test]
    fn errors_clearly_when_seed_node_does_not_exist() {
        let editor = editor_with_linked_notes();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.html");
        let result = execute_kb_export_subgraph_html(
            &editor,
            &serde_json::json!({"id": "does-not-exist", "path": out.to_str().unwrap()}),
        );
        let err = result.unwrap_err();
        assert!(err.contains("does-not-exist"), "{err}");
        assert!(err.contains("not found"), "{err}");
        assert!(!out.exists(), "must not write a file on a failed lookup");
    }

    #[test]
    fn errors_clearly_when_translations_path_is_bad() {
        let editor = editor_with_linked_notes();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.html");
        let result = execute_kb_export_subgraph_html(
            &editor,
            &serde_json::json!({
                "id": "root",
                "path": out.to_str().unwrap(),
                "translations": dir.path().join("nope.json").to_str().unwrap(),
            }),
        );
        let err = result.unwrap_err();
        assert!(err.contains("translations file"), "{err}");
    }

    #[test]
    fn missing_translations_arg_is_not_an_error() {
        let editor = editor_with_linked_notes();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.html");
        let result = execute_kb_export_subgraph_html(
            &editor,
            &serde_json::json!({"id": "root", "path": out.to_str().unwrap()}),
        );
        assert!(result.is_ok(), "{result:?}");
        assert!(out.exists());
    }

    #[test]
    fn exports_seed_plus_neighborhood_to_a_real_file() {
        let editor = editor_with_linked_notes();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("nested").join("out.html");
        let result = execute_kb_export_subgraph_html(
            &editor,
            &serde_json::json!({"id": "root", "path": out.to_str().unwrap(), "depth": 1}),
        );
        assert!(result.is_ok(), "{result:?}");
        let html = std::fs::read_to_string(&out).unwrap();
        assert!(html.contains("\"id\":\"root\""));
        assert!(html.contains("\"id\":\"child\""));
        assert!(html.starts_with("<!DOCTYPE html>"));
    }

    #[test]
    fn does_not_write_output_file_when_seed_lookup_fails_even_with_nested_dir() {
        let editor = editor_with_linked_notes();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("a").join("b").join("out.html");
        let result = execute_kb_export_subgraph_html(
            &editor,
            &serde_json::json!({"id": "nope", "path": out.to_str().unwrap()}),
        );
        assert!(result.is_err());
        assert!(!out.exists());
    }
}
