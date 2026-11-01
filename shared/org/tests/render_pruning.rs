//! Review round 6 (F, H): Org deletes the COMMENT subtrees before it
//! parses, runs babel or selects (`org-export--delete-comment-trees`, then
//! `org-export--selected-trees` and `org-export--prune-tree` on the parse),
//! so a select tag, a footnote definition or a named `#+RESULTS:` found
//! only under a COMMENT headline counts for nothing. Each expectation is
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
    ("h1", &["QZdQZ"], &["Head"], true),
    ("h2", &["QZeQZ", "QZfQZ"], &["V"], false),
    ("h3", &["QZgQZ"], &["Head"], true),
    ("h4", &["QZhQZ"], &["Head"], true),
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

/// Every name table the page resolves against × every reason a subtree
/// is pruned (COMMENT, `:noexport:`, archived, outside a selection, a
/// COMMENT subtree inside an archived one): the one definition of a name
/// sits in the pruned subtree, a use of it outside. What Emacs 30.2 shows
/// of the markers (`QZvQZ` outside, `QZsQZ` in the definition), or the
/// error its export stops with: MAE shows no marker Emacs does not.
const MATRIX: &[(&str, &str)] = &[
    ("call--archive", "v"),
    ("call--comment-in-archive", "error"),
    ("call--comment", "error"),
    ("call--noexport", "v"),
    ("call--unselected", "v"),
    ("fn--archive", "sv"),
    ("fn--comment-in-archive", "footnote"),
    ("fn--comment", "footnote"),
    ("fn--noexport", "sv"),
    ("fn--unselected", "sv"),
    ("macro--archive", "sv"),
    ("macro--comment-in-archive", "error"),
    ("macro--comment", "error"),
    ("macro--noexport", "sv"),
    ("macro--unselected", "sv"),
    ("name--archive", "v"),
    ("name--comment-in-archive", "v"),
    ("name--comment", "v"),
    ("name--noexport", "v"),
    ("name--unselected", "v"),
    ("radio--archive", "v"),
    ("radio--comment-in-archive", "v"),
    ("radio--comment", "v"),
    ("radio--noexport", "v"),
    ("radio--unselected", "v"),
    ("results--archive", "v"),
    ("results--comment-in-archive", "v"),
    ("results--comment", "v"),
    ("results--noexport", "v"),
    ("results--unselected", "v"),
    ("target--archive", "v"),
    ("target--comment-in-archive", "v"),
    ("target--comment", "v"),
    ("target--noexport", "v"),
    ("target--unselected", "v"),
];

#[test]
fn every_name_table_reads_the_pruned_tree() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pruning/matrix");
    let mut failures = Vec::new();
    for (name, emacs) in MATRIX {
        let doc = std::fs::read_to_string(dir.join(format!("{name}.org"))).unwrap();
        let d = parse(&doc, &ParseOptions::default());
        let sem = semantics(&d);
        let r = render(&d, &sem, &RenderOptions::default(), &Resolver::default());
        let all = format!("{}{}", r.to_json(TreeFormat::Html), mae_org::text(&d, &sem));
        let shows = |m: &str| all.contains(&format!("QZ{m}QZ"));
        let ok = match *emacs {
            // Org's export stops: MAE renders nothing (a macro), or hides
            // the call's results (registered: call-unknown-reference).
            "error" => !shows("s"),
            // Registered (undefined-footnote): the rest, with the
            // diagnostic.
            "footnote" => {
                !shows("s")
                    && shows("v")
                    && r.diagnostics
                        .iter()
                        .any(|d| d.kind_name() == "footnote_undefined")
            }
            seen => ["s", "v"].iter().all(|m| shows(m) == seen.contains(m)),
        };
        if !ok {
            failures.push(format!("{name}: Emacs {emacs}, MAE {all}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
