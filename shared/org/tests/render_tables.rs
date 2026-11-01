//! Org's special table rows and columns are not data (ADR-112 D3).
//!
//! `tests/render/tables.org` was exported by `emacs -Q --batch` (Emacs
//! 30.2, Org 9.7.11) with `org-export-as 'html`; `EMACS` below is that
//! output with the `<colgroup>`s, classes and table attributes removed and
//! an empty cell's `&#xa0;` as nothing. The rules, from `ox.el`:
//! - a row is special when its first cell is `/` (column groups), when it
//!   holds only alignment/width cookies (`<l>`, `<r>`, `<c>`, `<10>`,
//!   `<c10>`, `<>`, any case) and empty cells, or, in a table with a
//!   special column, when its first cell is `^`, `_`, `$` or `!`
//!   (`org-export-table-row-is-special-p`); special rows are dropped before
//!   the header is found;
//! - a table has a special column when every first cell of a non-rule row
//!   is empty or one of `/ # ! $ * _ ^`, and at least one is not empty
//!   (`org-export-table-has-special-column-p`); that column is dropped.
//!
//! MAE drops column groups rather than rendering `<colgroup>` (listed in
//! `conformance/render-divergences.toml`).

use mae_org::{parse, render_html, semantics, ParseOptions, RenderOptions, Resolver};

const EMACS: &[&str] = &[
    r#"<table><thead><tr><th scope="col">h1</th><th scope="col">h2</th></tr></thead><tbody><tr><td>a</td><td>b</td></tr></tbody></table>"#,
    r#"<table><tbody><tr><td>x1</td><td>x2</td></tr></tbody></table>"#,
    r#"<table><tbody><tr><td>y1</td><td>y2</td></tr></tbody></table>"#,
    r#"<table><tbody><tr><td>&lt;l&gt;</td><td>z</td></tr><tr><td>z1</td><td>z2</td></tr></tbody></table>"#,
    r#"<table><thead><tr><th scope="col">a</th><th scope="col">b</th></tr></thead><tbody><tr><td>1</td><td>2</td></tr><tr><td>s1</td><td>s2</td></tr></tbody></table>"#,
    r#"<table><tbody><tr><td></td><td>e1</td><td>e2</td></tr><tr><td></td><td>e3</td><td>e4</td></tr></tbody></table>"#,
    r#"<table><tbody><tr><td>^</td><td>p1</td><td>p2</td></tr><tr><td>q</td><td>q1</td><td>q2</td></tr></tbody></table>"#,
    r#"<table><tbody><tr><td>r1</td><td>r2</td><td>r3</td></tr></tbody></table>"#,
    r#"<table><thead><tr><th scope="col">w1</th><th scope="col">w2</th></tr></thead><tbody><tr><td>w3</td><td>w4</td></tr></tbody></table>"#,
    r#"<table><tbody><tr><td><strong>b</strong></td><td>k1</td></tr><tr><td>#</td><td>k2</td></tr></tbody></table>"#,
];

/// `html` without class attributes and line breaks.
fn normalise(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(i) = rest.find(" class=\"") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 8..];
        rest = &after[after.find('"').map_or(after.len(), |j| j + 1)..];
    }
    out.push_str(rest);
    out.replace('\n', "")
}

#[test]
fn special_rows_and_columns_render_as_org_exports_them() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/render/tables.org");
    let text = std::fs::read_to_string(path).expect("fixture");
    let doc = parse(&text, &ParseOptions::default());
    let html = render_html(
        &doc,
        &semantics(&doc),
        &RenderOptions::default(),
        &Resolver::default(),
    );
    let html = normalise(&html);
    let tables: Vec<String> = html
        .split("<table>")
        .skip(1)
        .map(|t| format!("<table>{}", &t[..t.find("</table>").expect("closed") + 8]))
        .collect();
    assert_eq!(tables.len(), EMACS.len(), "{html}");
    for (i, (got, want)) in tables.iter().zip(EMACS).enumerate() {
        assert_eq!(got, want, "table {i}");
    }
}
