//! Generates a small, deliberately-mixed-translation-completeness fixture
//! export for the Layer 2 (real-browser) test suite under `tests/browser/`.
//! This fixture exists specifically to exercise the translation-fallback UX
//! bug class this feature was built around: some nodes have a full EN/ES
//! translation, some none, some only a title or only a body, and one an
//! explicit empty-string translation (a real, distinct case from "no
//! translation at all"). See `docs/adr/` for the ported design rationale
//! this feature's own doc comments reference elsewhere.
//!
//! Usage: `cargo run --example fixture_export -p mae-export -- <output-path>`

use mae_export::html_graph::{
    build_export_node, build_guidance_node, ChordDiagramConfig, GraphExportEdge, GruvboxPalette,
    HtmlGraphExporter, NodeTranslation,
};
use std::f64::consts::PI;

fn main() {
    // Usage: `fixture_export <output-path> [hover-growth-factor]` -- the
    // optional second arg drives a ChordDiagramConfig override, used by the
    // Layer 2 suite to confirm a config override changes REAL runtime
    // hover-growth behavior, not just generated source text (see
    // hover_growth_factor_override_changes_generated_js in html_graph.rs
    // for the Layer 1 half of that coverage).
    let out_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "fixture.html".to_string());
    let hover_growth_factor_override = std::env::args().nth(2).map(|s| {
        s.parse::<f64>()
            .expect("hover-growth-factor must be a number")
    });
    let palette = GruvboxPalette::dark();

    // A small ring: "home" (anchor) plus five spokes, each exercising a
    // distinct translation-completeness case the Layer 2 suite drives.
    let specs: Vec<(&str, &str, &str, Option<NodeTranslation>)> = vec![
        (
            "home",
            "Fixture Home",
            "This is the anchor node. It has a real Spanish translation.",
            Some(NodeTranslation {
                title_es: Some("Inicio de la Prueba".to_string()),
                body_es: Some("Este es el nodo ancla. Tiene una traducción real al español.".to_string()),
            }),
        ),
        (
            "translated",
            "Fully Translated Node",
            "Both the title and body of this node are translated.",
            Some(NodeTranslation {
                title_es: Some("Nodo Completamente Traducido".to_string()),
                body_es: Some("Tanto el título como el cuerpo de este nodo están traducidos.".to_string()),
            }),
        ),
        (
            "untranslated",
            "Untranslated Node",
            "This node has no Spanish translation at all -- title_es and body_es both mirror English.",
            None,
        ),
        (
            "partial-title-only",
            "Partial: Title Only",
            "Only the title of this node is translated; the body falls back to English.",
            Some(NodeTranslation {
                title_es: Some("Parcial: Solo el Título".to_string()),
                body_es: None,
            }),
        ),
        (
            "partial-body-only",
            "Partial: Body Only",
            "Only the body of this node is translated; the title falls back to English.",
            Some(NodeTranslation {
                title_es: None,
                body_es: Some(
                    "Solo el cuerpo de este nodo está traducido; el título usa el original en inglés."
                        .to_string(),
                ),
            }),
        ),
        (
            "empty-string",
            "Empty String Translation",
            "This node has an explicit, real, empty-string Spanish translation -- not the same as no translation at all.",
            Some(NodeTranslation {
                title_es: Some(String::new()),
                body_es: Some(String::new()),
            }),
        ),
        // Syntax-highlighting fixture: a Terraform src block (keyword,
        // string, comment, number, all real token shapes highlightSource
        // recognizes) plus an example block with a "$ " shell-prompt line
        // -- exercises the Layer 2 highlighting suite. Deliberately not in
        // reading_order_extra below (falls back to the BFS-appended group,
        // same as partial-body-only/empty-string).
        (
            "code-sample",
            "Code Sample Node",
            "This node has a Terraform code block and a command-output example block.\n\n\
             #+begin_src tf\n\
             # provisions the example instance\n\
             resource \"aws_instance\" \"example\" {\n\
             \x20\x20count = 2\n\
             }\n\
             #+end_src\n\n\
             #+begin_example\n\
             $ terraform plan\n\
             No changes. Your infrastructure matches the configuration.\n\
             #+end_example\n\n\
             - [ ] Not done yet\n\
             - [X] Already done\n",
            None,
        ),
    ];

    // kb (gitlab-migration scaling work): an explicit authored reading-order
    // chain across 4 of the 6 spokes, deliberately in an order that visibly
    // differs from pure BFS-distance-from-anchor (every spoke is BFS
    // distance 1 from "home" -- the BFS/alphabetical fallback alone would
    // never produce this sequence). "home" (the anchor) sits at chain
    // position 1, not 0 -- mirrors the real gitlab-migration bug this
    // feature fixes, where "Start Here" (the anchor) sits mid-chain, not at
    // position 0 -- but still reachable FORWARD via Next to "untranslated"/
    // "translated" (only "partial-title-only" precedes it, via Previous),
    // so the existing title-search-via-Next tests (which start from the
    // anchor and only ever walk forward) and the new reading-order-specific
    // tests (which need the anchor NOT at position 0) are both satisfied by
    // the same fixture. "partial-body-only" and "empty-string" deliberately
    // have no Reading Order section, so the Layer 2 suite can also confirm
    // they still fall back to the BFS heuristic and get appended after the
    // chain.
    let reading_order_extra: &[(&str, &str, &str)] = &[
        ("partial-title-only", "none", "home"),
        ("home", "partial-title-only", "untranslated"),
        ("untranslated", "home", "translated"),
        ("translated", "untranslated", "none"),
    ];
    let reading_order_body = |id: &str| -> Option<String> {
        reading_order_extra
            .iter()
            .find(|(nid, ..)| *nid == id)
            .map(|(_, prev, next)| {
                let prev_line = if *prev == "none" {
                    "- Previous :: none.".to_string()
                } else {
                    format!("- Previous :: [[{prev}][{prev}]].")
                };
                let next_line = if *next == "none" {
                    "- Next :: none.".to_string()
                } else {
                    format!("- Next :: [[{next}][{next}]].")
                };
                // "untranslated" is the only chain node carrying a Part ::
                // line -- lets the Layer 2 suite confirm the breadcrumb
                // appears for it and is absent for every other node
                // (chain and non-chain alike).
                let part_line = if id == "untranslated" {
                    "- Part :: Fixture Chain Walkthrough.\n"
                } else {
                    ""
                };
                format!("\n\n* Reading Order\n{prev_line}\n{next_line}\n{part_line}")
            })
    };

    // A couple of distinct tags per node -- "empty-string" deliberately
    // stays untagged, so the Layer 2 suite can confirm an untagged node
    // also gets dimmed once ANY tag filter is active (OR-within-one-facet
    // semantics: no active filters -> everything matches; some active ->
    // an untagged node matches none of them).
    let tags_for = |id: &str| -> Vec<String> {
        match id {
            "home" => vec!["core".to_string()],
            "translated" => vec!["core".to_string(), "i18n".to_string()],
            "untranslated" => vec!["i18n".to_string()],
            "partial-title-only" => vec!["i18n".to_string(), "docs".to_string()],
            "partial-body-only" => vec!["docs".to_string()],
            _ => vec![],
        }
    };

    let n = specs.len();
    let radius = 200.0;
    let nodes: Vec<_> = specs
        .iter()
        .enumerate()
        .map(|(i, (id, title, body, translation))| {
            let angle = 2.0 * PI * (i as f64) / (n as f64);
            let (x, y) = if *id == "home" {
                (0.0, 0.0)
            } else {
                (radius * angle.cos(), radius * angle.sin())
            };
            let full_body = match reading_order_body(id) {
                Some(extra) => format!("{body}{extra}"),
                None => body.to_string(),
            };
            let mut node = build_export_node(
                *id,
                "note",
                x,
                y,
                *id == "home",
                *id == "home",
                title,
                &full_body,
                translation.as_ref(),
                &palette,
            );
            node.tags = tags_for(id);
            node
        })
        .collect();

    let edges: Vec<GraphExportEdge> = specs
        .iter()
        .filter(|(id, ..)| *id != "home")
        .map(|(id, ..)| GraphExportEdge {
            source: "home".to_string(),
            target: id.to_string(),
            rel_type: "related_to".to_string(),
            weight: 1.0,
        })
        .collect();

    // kb/adrs/0004: a guidance/colophon node, deliberately with no
    // translation and no edge to the rest of the ring -- exercises the
    // colophon click-to-open path, the guidance-note in the detail panel,
    // AND the ADR-0003 fallback notice all firing together on the same
    // node, plus confirms it's really excluded from the chord graph +
    // reading-order walk (no edges to it, no BFS-reachable position).
    let guidance = build_guidance_node(
        "style-guide",
        "practice",
        "Fixture Style Guide",
        "Guidance/meta content this fixture guide is written against.",
        None,
        &palette,
    );
    let mut nodes = nodes;
    nodes.push(guidance);

    let html = match hover_growth_factor_override {
        Some(hover_growth_factor) => {
            let config = ChordDiagramConfig {
                hover_growth_factor,
                ..ChordDiagramConfig::default()
            };
            HtmlGraphExporter.export_with_config(
                &nodes,
                &edges,
                "home",
                "Bilingual Export Fixture",
                &config,
            )
        }
        None => HtmlGraphExporter.export(&nodes, &edges, "home", "Bilingual Export Fixture"),
    };
    std::fs::write(&out_path, &html).unwrap_or_else(|e| {
        eprintln!("couldn't write {out_path}: {e}");
        std::process::exit(1);
    });
    println!(
        "Wrote {} nodes ({} edges) to {out_path} ({} bytes)",
        nodes.len(),
        edges.len(),
        html.len()
    );
}
