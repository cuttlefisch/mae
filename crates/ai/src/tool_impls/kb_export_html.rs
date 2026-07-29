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
//! graph_view_ops.rs`); layout reuses `mae_canvas::kb_graph::
//! build_kb_graph_chord_positions` (nodes at even angular positions around
//! a ring, the same chord/Circos-style placement the native graph view's
//! Chord layout mode uses — see that function's doc comment) so this tool
//! doesn't reimplement either. The exported nav widget is a small,
//! secondary chord diagram (edges rendered as arcs through the interior
//! client-side, see `bilingual_kb_export::html_graph::GRAPH_JS`), not the primary
//! force-directed graph view — `build_kb_graph` (Fruchterman-Reingold via
//! `ForceLayout::run`) is available in the same module if a future caller
//! wants that instead; swapping is a one-line change, not a new
//! dependency. HTML assembly lives entirely in `bilingual_kb_export::html_graph` —
//! this file is the bridge from `mae-kb`/`mae-canvas` types to that
//! crate's leaf-crate `GraphExportNode`/`GraphExportEdge` shapes (mirrors
//! `crates/core/src/editor/graph_view_ops.rs`'s own `to_kb_nodes`/
//! `to_link_info` bridging for the exact same reason: `mae-canvas` and
//! `mae-export` are deliberately kept free of a `mae-kb` dependency).

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
/// `depth` (optional, BFS hop radius, default 2, clamped to 4 — this tool
/// exports one flat chord ring with no drill-down, not built for a long
/// narrow chain; prefer multiple shallower exports over one deep one),
/// `node_cap` (optional, safety net on the reachable-set size, default 60,
/// clamped to 200 — past 200 the chord ring's per-node hit target starts
/// shrinking to fit the fixed-size widget, which needs layout work this
/// tool doesn't do yet), `translations` (optional, path to a `{id:
/// {title_es, body_es}}` JSON overlay — see `bilingual_kb_export::html_graph` module
/// docs), `title` (optional, page `<title>`/`<h1>`, default derived from
/// the seed node's own title). If `node_cap` truncates the reachable set,
/// the returned status string says so explicitly (`"N more node(s) hidden
/// by node_cap"`) rather than reporting a plain success.
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
    // Clamped, not just defaulted -- 4 is a deliberate ceiling, not an
    // arbitrary one: this tool renders every node client-side in one flat
    // chord ring with no drill-down/pagination, so an unbounded depth on a
    // well-connected KB can reach well past what `node_cap` below would
    // keep anyway. A caller that legitimately needs a longer linear chain
    // should prefer multiple shallower exports over one deep one -- this
    // tool's UI (chord ring + reading-order walk) isn't built for a long,
    // narrow shape.
    let depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(2)
        .min(4) as usize;
    // A generous but real safety net -- this tool targets small (~15-20
    // node) curated exports; a much larger reachable set is almost
    // certainly not what a "subgraph export" call intended. Overridable,
    // still capped at 200: past that the chord ring's per-node hit target
    // shrinks as the ring grows to fit a fixed-size widget (see
    // `build_kb_graph_chord_positions`'s sqrt-area layout), so a much
    // larger export needs widget-sizing work this tool doesn't do yet, not
    // just a bigger number here.
    let node_cap = args
        .get("node_cap")
        .and_then(|v| v.as_u64())
        .unwrap_or(60)
        .min(200) as usize;

    let kb = locate_seed_kb(editor, id)?;

    let spec = mae_kb::SubgraphSpec {
        starter_nodes: vec![id.to_string()],
        max_depth: depth,
        include_backlinks: true,
        node_cap: Some(node_cap),
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

    let translations: bilingual_kb_export::html_graph::TranslationMap =
        match args.get("translations").and_then(|v| v.as_str()) {
            Some(raw) => {
                let resolved = resolve_path(editor, raw);
                bilingual_kb_export::html_graph::load_translations(&resolved)?
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
    // Boundary links are dropped for this export (see bilingual_kb_export::html_graph
    // module docs: v1 is single-KB, read-only, no "... (+N)" stub concept)
    // -- the chord layout doesn't consult them at all, so `&[]`.
    //
    // Chord ring positions (`build_kb_graph_chord_positions`), not
    // force-directed (`build_kb_graph`): the exported nav widget is a
    // small, secondary chord diagram (nodes at even angular positions,
    // edges as arcs through the interior -- rendered client-side in
    // `bilingual_kb_export::html_graph::GRAPH_JS`), not the primary force-directed
    // graph view. Both functions live in the same module and share the
    // same `SceneGraph`/positions-only contract, so this is a one-line
    // swap, not a new dependency.
    let scene =
        mae_canvas::kb_graph::build_kb_graph_chord_positions(&kb_nodes, &kb_links, &[], 1.0);
    let positions: HashMap<String, (f64, f64)> = scene
        .nodes
        .iter()
        .map(|n| (n.id.clone(), (n.x, n.y)))
        .collect();

    // --- Bridge to mae-export for HTML rendering ---
    let palette = bilingual_kb_export::html_graph::GruvboxPalette::dark();
    let export_nodes: Vec<bilingual_kb_export::html_graph::GraphExportNode> = result
        .nodes
        .iter()
        .map(|n| {
            let (x, y) = positions.get(&n.id).copied().unwrap_or((0.0, 0.0));
            let mut export_node = bilingual_kb_export::html_graph::build_export_node(
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
            );
            // Org `#+filetags:`, verbatim -- drives the exported page's
            // header tag-filter UI. `build_export_node` defaults this to
            // empty (it has no access to `mae_kb::Node` itself, only the
            // raw title/body strings), so this is the one place that has
            // the real tag data and sets it after construction.
            export_node.tags = n.tags.clone();
            export_node
        })
        .collect();
    let export_edges: Vec<bilingual_kb_export::html_graph::GraphExportEdge> = result
        .links
        .iter()
        .map(|l| bilingual_kb_export::html_graph::GraphExportEdge {
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

    let html = bilingual_kb_export::html_graph::HtmlGraphExporter.export(
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
        "Exported {} node{} ({} edges) rooted at '{id}' to {} ({} bytes){}{}",
        export_nodes.len(),
        if export_nodes.len() == 1 { "" } else { "s" },
        export_edges.len(),
        out_path.display(),
        html.len(),
        if translations.is_empty() {
            String::new()
        } else {
            format!(", {} translation(s) applied", translations.len())
        },
        // Never truncate silently -- house rule elsewhere in this codebase
        // (see graph_view_ops.rs's identical hidden_node_count reporting).
        // Without this, a caller hitting `node_cap` got a success message
        // that read as complete ("Exported 60 nodes...") with no signal
        // that the true reachable set was actually larger.
        if result.hidden_node_count > 0 {
            format!(
                ", {} more node(s) hidden by node_cap",
                result.hidden_node_count
            )
        } else {
            String::new()
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

    /// A star KB: `root` plus `spoke_count` directly-linked spokes -- for
    /// exercising `node_cap`/`hidden_node_count` behavior, which
    /// `editor_with_linked_notes`'s 2-node fixture can't reach.
    fn editor_with_star_kb(spoke_count: usize) -> Editor {
        let mut editor = Editor::new();
        let mut root_body = "The hub.".to_string();
        for i in 0..spoke_count {
            root_body.push_str(&format!(" [[spoke{i}][Spoke {i}]]"));
        }
        editor.kb.primary.insert(mae_kb::Node::new(
            "root",
            "Hub",
            mae_kb::NodeKind::Note,
            &root_body,
        ));
        for i in 0..spoke_count {
            editor.kb.primary.insert(mae_kb::Node::new(
                format!("spoke{i}"),
                format!("Spoke {i}"),
                mae_kb::NodeKind::Note,
                "A spoke note.",
            ));
        }
        editor
    }

    #[test]
    fn node_cap_truncation_is_reported_not_silent() {
        let editor = editor_with_star_kb(20);
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.html");
        let msg = execute_kb_export_subgraph_html(
            &editor,
            &serde_json::json!({
                "id": "root",
                "path": out.to_str().unwrap(),
                "node_cap": 5,
            }),
        )
        .unwrap();
        // 5 nodes fit (root + 4 spokes out of 20 reachable) -> 16 hidden.
        assert!(msg.contains("Exported 5 nodes"), "{msg}");
        assert!(msg.contains("16 more node(s) hidden by node_cap"), "{msg}");
    }

    #[test]
    fn node_cap_override_is_honored_and_default_still_60() {
        let editor = editor_with_star_kb(3);
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.html");
        // Well under both the override and the default -- no truncation,
        // no "hidden" mention either way.
        let msg = execute_kb_export_subgraph_html(
            &editor,
            &serde_json::json!({"id": "root", "path": out.to_str().unwrap(), "node_cap": 10}),
        )
        .unwrap();
        assert!(msg.contains("Exported 4 nodes"), "{msg}");
        assert!(!msg.contains("hidden"), "{msg}");

        let out2 = dir.path().join("out2.html");
        let msg2 = execute_kb_export_subgraph_html(
            &editor,
            &serde_json::json!({"id": "root", "path": out2.to_str().unwrap()}),
        )
        .unwrap();
        assert!(msg2.contains("Exported 4 nodes"), "{msg2}");
        assert!(!msg2.contains("hidden"), "{msg2}");
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

    // --- Chord-ring layout bridge (mae_canvas::kb_graph::
    // build_kb_graph_chord_positions), the widget's actual layout call ---

    #[test]
    fn chord_layout_places_three_nodes_at_distinct_positions() {
        let kb_nodes = vec![
            mae_canvas::kb_graph::KbNodeInfo {
                id: "a".into(),
                title: "A".into(),
                kind: mae_canvas::scene::NodeKind::Note,
                is_seed: false,
            },
            mae_canvas::kb_graph::KbNodeInfo {
                id: "b".into(),
                title: "B".into(),
                kind: mae_canvas::scene::NodeKind::Note,
                is_seed: false,
            },
            mae_canvas::kb_graph::KbNodeInfo {
                id: "c".into(),
                title: "C".into(),
                kind: mae_canvas::scene::NodeKind::Note,
                is_seed: false,
            },
        ];
        let scene = mae_canvas::kb_graph::build_kb_graph_chord_positions(&kb_nodes, &[], &[], 1.0);
        assert_eq!(scene.nodes.len(), 3);
        let unique: std::collections::HashSet<(i64, i64)> = scene
            .nodes
            .iter()
            .map(|n| ((n.x * 1000.0) as i64, (n.y * 1000.0) as i64))
            .collect();
        assert_eq!(
            unique.len(),
            3,
            "chord ring nodes must be at distinct positions: {:?}",
            scene.nodes.iter().map(|n| (n.x, n.y)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn exported_subgraph_has_distinct_coordinates_per_node_end_to_end() {
        let mut editor = editor_with_linked_notes();
        editor.kb.primary.insert(mae_kb::Node::new(
            "grandchild",
            "Grandchild Note",
            mae_kb::NodeKind::Note,
            "A grandchild note.",
        ));
        editor
            .kb
            .primary
            .get_mut("child")
            .unwrap()
            .body
            .push_str(" See [[grandchild][Grandchild]].");

        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.html");
        let result = execute_kb_export_subgraph_html(
            &editor,
            &serde_json::json!({"id": "root", "path": out.to_str().unwrap(), "depth": 2}),
        );
        assert!(result.is_ok(), "{result:?}");
        let html = std::fs::read_to_string(&out).unwrap();
        assert!(html.contains("\"id\":\"grandchild\""));

        // Extract the embedded JSON payload and confirm every node got a
        // distinct (x, y) from the chord layout, not all collapsed to the
        // same point.
        let marker = "<script id=\"graph-data\" type=\"application/json\">";
        let start = html.find(marker).unwrap() + marker.len();
        let end = html[start..].find("</script>").unwrap() + start;
        let raw = html[start..end].replace("<\\/", "</");
        let payload: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let nodes = payload["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 3);
        let unique: std::collections::HashSet<(i64, i64)> = nodes
            .iter()
            .map(|n| {
                (
                    (n["x"].as_f64().unwrap() * 1000.0) as i64,
                    (n["y"].as_f64().unwrap() * 1000.0) as i64,
                )
            })
            .collect();
        assert_eq!(
            unique.len(),
            3,
            "expected 3 distinct chord positions: {nodes:?}"
        );
    }
}
