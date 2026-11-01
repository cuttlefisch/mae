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

/// Every name table the page resolves against (footnote definitions,
/// standalone and inline; targets; radio targets; `#+NAME`; named
/// results; babel calls; macro templates) × every way Org leaves a region
/// out (a COMMENT subtree, alone or in an archived one; `:noexport:`; an
/// `#+EXCLUDE_TAGS` tag; archived; outside a selection; a LOGBOOK or other
/// drawer; a comment block; comment lines): the one definition of a name
/// sits in the region, a use of it outside. Measured in Emacs 30.2, the
/// markers its page shows (`v` outside, `s` in the definition), or the
/// error its export stops with: only what ox deletes before it parses (a
/// COMMENT subtree) or never parses as a definition (a comment) holds
/// none; a footnote definition anywhere else, standalone or inline, is
/// still used. MAE shows exactly the markers Emacs shows, but in a drawer
/// it does not render.
const MATRIX: &[(&str, &str)] = &[
    ("call--archive", "v"),
    ("call--comment-block", "error"),
    ("call--comment-in-archive", "error"),
    ("call--comment-lines", "error"),
    ("call--comment", "error"),
    ("call--drawer", "sv"),
    ("call--exclude", "v"),
    ("call--logbook", "v"),
    ("call--noexport", "v"),
    ("call--unselected", "v"),
    ("fn--archive", "sv"),
    ("fn--comment-block", "footnote"),
    ("fn--comment-in-archive", "footnote"),
    ("fn--comment-lines", "footnote"),
    ("fn--comment", "footnote"),
    ("fn--drawer", "sv"),
    ("fn--exclude", "sv"),
    ("fninline--archive", "sv"),
    ("fninline--comment-block", "footnote"),
    ("fninline--comment-in-archive", "footnote"),
    ("fninline--comment-lines", "footnote"),
    ("fninline--comment", "footnote"),
    ("fninline--drawer", "sv"),
    ("fninline--exclude", "sv"),
    ("fninline--logbook", "sv"),
    ("fninline--noexport", "sv"),
    ("fninline--unselected", "sv"),
    ("fn--logbook", "sv"),
    ("fn--noexport", "sv"),
    ("fn--unselected", "sv"),
    ("macro--archive", "sv"),
    ("macro--comment-block", "error"),
    ("macro--comment-in-archive", "error"),
    ("macro--comment-lines", "error"),
    ("macro--comment", "error"),
    ("macro--drawer", "sv"),
    ("macro--exclude", "sv"),
    ("macro--logbook", "sv"),
    ("macro--noexport", "sv"),
    ("macro--unselected", "sv"),
    ("name--archive", "v"),
    ("name--comment-block", "v"),
    ("name--comment-in-archive", "v"),
    ("name--comment-lines", "v"),
    ("name--comment", "v"),
    ("name--drawer", "sv"),
    ("name--exclude", "v"),
    ("name--logbook", "v"),
    ("name--noexport", "v"),
    ("name--unselected", "v"),
    ("radio--archive", "v"),
    ("radio--comment-block", "v"),
    ("radio--comment-in-archive", "v"),
    ("radio--comment-lines", "v"),
    ("radio--comment", "v"),
    ("radio--drawer", "sv"),
    ("radio--exclude", "v"),
    ("radio--logbook", "v"),
    ("radio--noexport", "v"),
    ("radio--unselected", "v"),
    ("results--archive", "v"),
    ("results--comment-block", "v"),
    ("results--comment-in-archive", "v"),
    ("results--comment-lines", "v"),
    ("results--comment", "v"),
    ("results--drawer", "v"),
    ("results--exclude", "v"),
    ("results--logbook", "v"),
    ("results--noexport", "v"),
    ("results--unselected", "v"),
    ("target--archive", "v"),
    ("target--comment-block", "v"),
    ("target--comment-in-archive", "v"),
    ("target--comment-lines", "v"),
    ("target--comment", "v"),
    ("target--drawer", "sv"),
    ("target--exclude", "v"),
    ("target--logbook", "v"),
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
            // A drawer other than LOGBOOK: Org exports it, MAE renders
            // none (registered: drawers), so hides what it holds.
            seen if name.ends_with("--drawer") => shows("v") && (!shows("s") || seen.contains('s')),
            seen => ["s", "v"].iter().all(|m| shows(m) == seen.contains(m)),
        };
        if !ok {
            failures.push(format!("{name}: Emacs {emacs}, MAE {all}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Leak oracle (seed 2, round 6): Org deletes the source's COMMENT
/// subtrees from the text before it expands macros; a COMMENT headline an
/// expansion makes stays, is parsed (its tags select, its footnote
/// definitions are used), and is pruned with what it holds. Babel skips
/// what sits under a COMMENT or archived heading, so a results macro there
/// stays for the second expansion, where an undefined macro it completes
/// fails the export. Emacs 30.2 on each (`tests/pruning/expanded/`):
/// (file, what the page shows, what no output shows, whether the export
/// fails).
#[test]
fn an_expansions_comment_headline_is_parsed_then_pruned() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pruning/expanded");
    let cases: &[(&str, &[&str], &[&str], bool)] = &[
        ("e1", &[], &["QZaQZ", "QZbQZ"], false),
        ("e2", &[], &["QZcQZ", "QZdQZ", "QZeQZ", "QZfQZ"], false),
        ("e3", &["V", "QZhQZ", "QZiQZ"], &["QZgQZ"], false),
        ("e4", &["V", "QZkQZ"], &["QZjQZ"], false),
        ("e5", &[], &["QZlQZ", "QZmQZ"], false),
        ("e6", &[], &["qz20qz", "qz22qz"], true),
        ("e7", &["V", "ok"], &["QZnQZ"], false),
    ];
    let mut failures = Vec::new();
    for (name, shown, hidden, fails) in cases {
        let doc = std::fs::read_to_string(dir.join(format!("{name}.org"))).unwrap();
        let d = parse(&doc, &ParseOptions::default());
        let sem = semantics(&d);
        let r = render(&d, &sem, &RenderOptions::default(), &Resolver::default());
        let all = format!(
            "{}{}",
            r.to_json(TreeFormat::Html),
            r.to_json(TreeFormat::Hast)
        );
        let page = mae_org::to_html(&r.body);
        for s in *shown {
            if !page.contains(s) {
                failures.push(format!("{name}: {s} not shown: {page}"));
            }
        }
        for h in *hidden {
            if all.contains(h) {
                failures.push(format!("{name}: {h} shown: {all}"));
            }
        }
        if shown.is_empty() && !page.is_empty() {
            failures.push(format!("{name}: Emacs's page is empty: {page}"));
        }
        let failed = r
            .diagnostics
            .iter()
            .any(|d| d.kind_name() == "macro_undefined");
        if failed != *fails {
            failures.push(format!("{name}: {:?}", r.diagnostics));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
