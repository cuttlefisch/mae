//! Synthetic smoke test for `kb_export_subgraph_html` — builds a tiny
//! in-memory KB (no real personal paths, no dependency on any specific
//! machine's KB content) and runs it through the exact same
//! `execute_kb_export_subgraph_html` call path the MCP tool and the
//! `kb-export-subgraph-html` Scheme primitive use. For real invocation
//! recipes against actual KB data, see `docs/KB_SUBGRAPH_EXPORT.md`.
//!
//! Usage: `cargo run --example export_demo -p mae-ai -- <output-path>`

fn main() {
    let out_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/export_demo.html".to_string());

    let mut editor = mae_core::Editor::new();
    editor.kb.primary.insert(mae_kb::Node::new(
        "hub",
        "Demo Hub",
        mae_kb::NodeKind::Note,
        "The anchor node. See [[spoke-a][Spoke A]] and [[spoke-b][Spoke B]].",
    ));
    editor.kb.primary.insert(mae_kb::Node::new(
        "spoke-a",
        "Spoke A",
        mae_kb::NodeKind::Note,
        "A neighbor of the hub.",
    ));
    editor.kb.primary.insert(mae_kb::Node::new(
        "spoke-b",
        "Spoke B",
        mae_kb::NodeKind::Note,
        "Another neighbor of the hub.",
    ));

    let args = serde_json::json!({
        "id": "hub",
        "path": out_path,
        "depth": 1,
        "title": "Export Demo (synthetic fixture)",
    });

    match mae_ai::execute_kb_export_subgraph_html(&editor, &args, None) {
        Ok(msg) => eprintln!("OK: {msg}"),
        Err(e) => {
            eprintln!("ERROR: {e}");
            std::process::exit(1);
        }
    }
}
