//! Issue #951: the org export honours `#+OPTIONS: H:N` as Org's does.
//! Headlines whose relative level is above N (3 when the file says nothing,
//! `org-export-headline-levels`) export as list items, in both formats; a
//! subtree export counts from the subtree's root, as Org narrows to it.

use mae_export::document::{export_document, export_subtree, PageSettings};
use mae_export::html::to_html_page;
use mae_export::markdown::to_markdown;

/// The titles the HTML page renders as headings, in order.
fn headings(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in html.split("<h").skip(1) {
        if !part.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        let open = part.find('>').unwrap() + 1;
        let close = part.find("</h").unwrap();
        out.push(part[open..close].to_string());
    }
    out
}

/// The titles the HTML page renders as low-level headline items, in order.
fn items(html: &str) -> Vec<String> {
    html.split("class=\"org-headline-title\"")
        .skip(1)
        .map(|part| {
            let open = part.find('>').unwrap() + 1;
            part[open..part.find("</div>").unwrap()].to_string()
        })
        .collect()
}

fn page(src: &str) -> String {
    to_html_page(&export_document(src))
}

const DEEP: &str = "* One\n** Two\n*** Three\n**** Four\nfour body\n***** Five\n";

#[test]
fn without_h_three_levels_are_headings_as_in_org() {
    assert_eq!(
        PageSettings::default().headline_levels,
        Some(3),
        "org-export-headline-levels"
    );
    let html = page(DEEP);
    // The table of contents lists headings only: read the body.
    let body = &html[html.find("</nav>").unwrap()..];
    assert_eq!(headings(body), ["One", "Two", "Three"]);
    assert_eq!(items(body), ["Four", "Five"]);
    assert!(body.contains("four body"), "{body}");
}

#[test]
fn h_in_options_sets_the_cap_in_both_formats() {
    let src = format!("#+OPTIONS: toc:nil H:1\n{DEEP}");
    let e = export_document(&src);
    let html = to_html_page(&e);
    assert_eq!(headings(&html), ["One"]);
    assert_eq!(items(&html), ["Two", "Three", "Four", "Five"]);
    let md = to_markdown(&e);
    assert!(md.contains("# One"), "{md}");
    for title in ["Two", "Three", "Four", "Five"] {
        assert!(
            !md.contains(&format!("# {title}")),
            "{title} a heading:\n{md}"
        );
        assert!(
            md.contains(&format!("- {title}")),
            "{title} not an item:\n{md}"
        );
    }
    assert!(md.contains("four body"), "{md}");
}

#[test]
fn h_values_read_as_org_reads_them() {
    // (H value, headings of DEEP). Measured in Emacs 30.2: nil, a negative
    // or a fraction caps nothing (not `wholenump`); a sign or leading zero
    // reads as the number; of several, the last wins.
    let cases: &[(&str, &[&str])] = &[
        ("H:0", &[]),
        ("H:2", &["One", "Two"]),
        ("H:+1", &["One"]),
        ("H:01", &["One"]),
        ("H:nil", &["One", "Two", "Three", "Four", "Five"]),
        ("H:-1", &["One", "Two", "Three", "Four", "Five"]),
        ("H:1.5", &["One", "Two", "Three", "Four", "Five"]),
        ("H:4 H:1", &["One"]),
        ("H:1\n#+OPTIONS: H:4", &["One", "Two", "Three", "Four"]),
    ];
    for (h, want) in cases {
        let html = page(&format!("#+OPTIONS: toc:nil {h}\n{DEEP}"));
        assert_eq!(headings(&html), *want, "{h}");
        assert_eq!(
            headings(&html).len() + items(&html).len(),
            5,
            "{h}: every headline once"
        );
    }
}

#[test]
fn a_subtree_export_counts_levels_from_its_root() {
    let src = format!("#+OPTIONS: toc:nil H:1\n{DEEP}");
    // `Two` is an item of the whole page, but the root of its own export.
    let e = export_subtree(&src, src.find("** Two").unwrap()).expect("exported");
    let html = to_html_page(&e);
    assert_eq!(headings(&html), ["Two"]);
    assert_eq!(items(&html), ["Three", "Four", "Five"]);
    // Default H (3): the subtree at `Four` gets Four and Five as headings,
    // where the whole page has them as items.
    let e = export_subtree(DEEP, DEEP.find("four body").unwrap()).expect("exported");
    let html = to_html_page(&e);
    assert_eq!(headings(&html), ["Four", "Five"]);
    assert!(items(&html).is_empty(), "{html}");
}

#[test]
fn a_subtree_export_under_h0_is_one_item_in_its_list() {
    let src = "#+OPTIONS: toc:nil H:0\n* A\n** B\nbee\n* C\n";
    let e = export_subtree(src, src.find("bee").unwrap()).expect("exported");
    let html = to_html_page(&e);
    assert_eq!(items(&html), ["B"]);
    assert!(!html.contains(">A<") && !html.contains(">C<"), "{html}");
    assert_eq!(
        html.matches("<ul class=\"org-headlines\">").count(),
        1,
        "{html}"
    );
}
