//! Review round 6 (minor 10): ox-html converts special strings in every
//! plain-text object (`org-export-with-special-strings`, on by default):
//! `\-` a soft hyphen, `---` and `--` before a character that is not `-`
//! an em and an en dash, `...` an ellipsis. Never in code, verbatim, an
//! inline src block, a link's path or across objects (a hidden export
//! snippet between two dashes keeps them), and not in MAE's search text
//! or slugs. Each line is Emacs 30.2's page of
//! `tests/special-strings/s1.org`, entities decoded.

use mae_org::{parse, render, semantics, text, ParseOptions, RenderOptions, Resolver};

fn shown(html: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in html.chars() {
        match c {
            '<' => tag = true,
            '>' if tag => tag = false,
            c if !tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
}

#[test]
fn special_strings_convert_as_ox_html_converts_them() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/special-strings/s1.org");
    let doc = std::fs::read_to_string(path).unwrap();
    let d = parse(&doc, &ParseOptions::default());
    let sem = semantics(&d);
    let r = render(&d, &sem, &RenderOptions::default(), &Resolver::default());
    let page = shown(&mae_org::to_html(&r.body));
    for want in [
        "A \u{2013} B",
        "P1 A \u{2013} B \u{2014} C \u{2026} D \u{ad} E",
        "P2 a\u{2013}b a\u{2014}",
        "P3 a -- b c...d a--b",
        "P4 x \u{2013} y b \u{2013} c",
        "P5 a--b and a -\u{2014} b",
        "P6 (pre \u{2013} x ??, ???? s \u{2026} t)",
        "P7 x\u{2013}\ny",
        "a \u{2013} b",
        "P8 a\u{ad}b \\\u{ad} c",
    ] {
        assert!(page.contains(want), "{want:?} not in {page:?}");
    }
    let html = mae_org::to_html(&r.body);
    assert!(html.contains("href=\"https://e.com/a--b\""), "{html}");
    // Search text and anchors read the source's characters.
    assert!(text(&d, &sem).contains("A -- B --- C ... D \\- E"));
    assert!(html.contains("id=\"h-a-b\""), "{html}");
}
