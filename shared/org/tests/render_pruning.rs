//! Review round 6 (F): Org deletes the COMMENT subtrees before it parses
//! or selects (`org-export--delete-comment-trees`, then
//! `org-export--selected-trees` and `org-export--prune-tree` on the parse),
//! so a select tag found only under a COMMENT headline counts for nothing. Each expectation is
//! Emacs 30.2's export of the file in `tests/pruning/`.

use mae_org::{parse, render, semantics, ParseOptions, RenderOptions, Resolver, TreeFormat};

/// (file, what every output hides, what the page shows, whether a
/// footnote reference is left undefined: Emacs's export fails there,
/// registered as `undefined-footnote`).
const CASES: &[(&str, &[&str], &[&str], bool)] = &[
    ("f1", &["QZaQZ", "QZbQZ", "Parent"], &["Other"], false),
    ("f2", &["QZaQZ", "QZbQZ", "Parent"], &["Other"], false),
    ("f3", &["QZaQZ", "QZbQZ", "Parent"], &["Other"], false),
    (
        "f4",
        &["QZcQZ"],
        &["Parent QZaQZ", "Archived", "Other"],
        false,
    ),
];

#[test]
fn a_comment_subtree_counts_for_nothing_in_any_table() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pruning");
    let mut failures = Vec::new();
    for (name, hidden, shown, undefined) in CASES {
        let doc = std::fs::read_to_string(dir.join(format!("{name}.org"))).unwrap();
        let d = parse(&doc, &ParseOptions::default());
        let sem = semantics(&d);
        for pd in [false, true] {
            let opts = RenderOptions::default().with_property_drawers(pd);
            let r = render(&d, &sem, &opts, &Resolver::default());
            let all = format!(
                "{}{}{}",
                r.to_json(TreeFormat::Html),
                r.to_json(TreeFormat::Hast),
                mae_org::text(&d, &sem)
            );
            for h in *hidden {
                if all.contains(h) {
                    failures.push(format!("{name}: {h} shown:\n{all}"));
                }
            }
            let page = mae_org::to_html(&r.body);
            for s in *shown {
                if !page.contains(s) {
                    failures.push(format!("{name}: {s} not shown:\n{page}"));
                }
            }
            let flagged = r
                .diagnostics
                .iter()
                .any(|d| d.kind_name() == "footnote_undefined");
            if flagged != *undefined {
                failures.push(format!("{name}: diagnostics {:?}", r.diagnostics));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
