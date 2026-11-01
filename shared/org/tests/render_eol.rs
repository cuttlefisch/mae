//! A document renders the same whatever its line-ending convention
//! (ADR-112 D3): Emacs decodes a DOS or mac file to `\n` lines before Org
//! reads it, so the LF, CRLF and CR spellings of one text are one document,
//! and every renderer line split must follow the document's convention
//! (a CR-only file split on `\n` is a single line: Org's comma escapes and
//! `: ` prefixes would survive on every line but the first).

mod support;

use mae_org::{parse, render, semantics, text, ParseOptions, RenderOptions, Resolved, Resolver};
use std::path::Path;

fn resolver() -> Resolver {
    Resolver::default()
        .with_target("known", Resolved::new("/notes/known.html"))
        .with_target("titled", Resolved::new("/t.html").with_title("T"))
}

/// The body HTML, the node metadata as JSON, and the search text.
fn outputs(t: &str) -> (String, String, String) {
    let doc = parse(t, &ParseOptions::default());
    let sem = semantics(&doc);
    let opts = RenderOptions::default()
        .with_property_drawers(true)
        .with_images(true);
    let r = render(&doc, &sem, &opts, &resolver());
    let nodes = r
        .nodes
        .iter()
        .map(|n| {
            format!(
                "{} {:?} {} {}",
                n.id,
                n.anchor,
                mae_org::to_html(&n.title),
                mae_org::to_html(&n.title_unlinked)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    (mae_org::to_html(&r.body), nodes, text(&doc, &sem))
}

#[test]
fn lf_crlf_and_cr_spellings_render_identically() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut files = support::org_files(&here.join("render"));
    files.extend(support::org_files(&here.join("fixtures")));
    files.extend(support::bundled_corpus());
    let mut checked = 0;
    let mut failures = Vec::new();
    for f in &files {
        let Ok(lf) = std::fs::read_to_string(f) else {
            continue;
        };
        // A CR already in the text is content in an LF file and a line
        // ending in a CR one: the spellings are not the same document.
        if lf.contains('\r') {
            continue;
        }
        checked += 1;
        let want = outputs(&lf);
        for (name, v) in [
            ("crlf", lf.replace('\n', "\r\n")),
            ("cr", lf.replace('\n', "\r")),
        ] {
            let got = outputs(&v);
            for (what, a, b) in [
                ("html", &want.0, &got.0),
                ("nodes", &want.1, &got.1),
                ("text", &want.2, &got.2),
            ] {
                if a != b {
                    let at = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
                    let from = at.saturating_sub(60);
                    failures.push(format!(
                        "{} ({name}) {what} differs at byte {at}:\n  lf:  {:?}\n  {name}: {:?}",
                        support::rel(f),
                        a.get(from..(at + 60).min(a.len())).unwrap_or(""),
                        b.get(from..(at + 60).min(b.len())).unwrap_or("")
                    ));
                }
            }
        }
    }
    assert!(checked >= 300, "only {checked} files checked");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[test]
fn a_cr_only_file_unescapes_and_unprefixes_every_line() {
    let lf = "#+begin_src org\n,* one\n,* two\n#+end_src\n\n: alpha\n: beta\n";
    for t in [
        lf.to_string(),
        lf.replace('\n', "\r"),
        lf.replace('\n', "\r\n"),
    ] {
        let doc = parse(&t, &ParseOptions::default());
        let html = mae_org::render_html(
            &doc,
            &semantics(&doc),
            &RenderOptions::default(),
            &resolver(),
        );
        assert!(html.contains("* one\n* two"), "{t:?}: {html}");
        assert!(html.contains("alpha\nbeta"), "{t:?}: {html}");
        assert!(
            !html.contains(",*") && !html.contains(": beta"),
            "{t:?}: {html}"
        );
    }
}
