//! Citations render as Org's basic processor exports an unresolved one
//! (review round 5, D): MAE reads no bibliography, so no key is ever shown,
//! in any output, attributes included. Each expectation below is Emacs
//! 30.2's (`emacs -Q`, ox-html body only, no bibliography), the page's
//! text with its tags stripped.

use mae_org::{
    parse, render, render_html, semantics, text, ParseOptions, RenderOptions, Resolver, TreeFormat,
};

fn visible(html: &str) -> String {
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
    out
}

fn page(doc: &str) -> String {
    let d = parse(doc, &ParseOptions::default());
    render_html(
        &d,
        &semantics(&d),
        &RenderOptions::default(),
        &Resolver::default(),
    )
}

#[test]
fn a_citation_renders_as_the_basic_processor_renders_an_unresolved_one() {
    let cases: &[(&str, &str)] = &[
        ("[cite:@SECk1]", "A (??, ????) B."),
        ("[cite:@SECk1;@SECk2]", "A (??, ????, ??, a) B."),
        (
            "[cite:SECpre @SECk1 SECsuf]",
            "A (SECpre ??, ???? SECsuf) B.",
        ),
        (
            "[cite:SECgpre; SECpre @SECk1 SECsuf; SECgsuf]",
            "A (SECgpre SECpre ??, ???? SECsuf SECgsuf) B.",
        ),
        ("[cite/t:@SECk1]", "A ?? (????) B."),
        ("[cite/n:@SECk1]", "A B."),
        ("[cite/a:@SECk1]", "A ?? B."),
        (
            "[cite/t:SECpre @SECk1 SECsuf;@SECk2]",
            "A SECpre ?? (????) SECsuf, ?? (a) B.",
        ),
        ("[cite/noauthor:@SECk1]", "A (????) B."),
        ("[cite/locators:@SECk1]", "A (??, ????) B."),
        ("[cite:@SECk1 p. 3]", "A (??, ???? p. 3) B."),
        ("[cite/a/f:@SECk1]", "A ?? B."),
        (
            "[cite:@k1] [cite:@k1] [cite:@k2] [cite:@k2] [cite:@k3]",
            "A (??, ????) (??, ) (??, a) (??, a) (??, a) B.",
        ),
        ("[cite/nb:@k1;@k2] [cite/nb:@k2]", "A (1, 2) (2) B."),
        ("[cite/b:@k1]", "A (??, ????) B."),
        ("[cite/t/c:@k1]", "A ?? (????) B."),
        ("[cite/na:@k1] [cite/na:@k1]", "A (????) () B."),
        ("[cite/bc:@k1]", "A (??, ????) B."),
        ("[cite/t/b:@k1]", "A ?? ???? B."),
        ("[cite/na/b:@k1]", "A ???? B."),
        (
            "[cite:*SECbold* pre @k1 =SECv= suf]",
            "A (SECbold pre ??, ???? SECv suf) B.",
        ),
        (
            "[cite:pre @k1 [[https://e.com][SEClink]] ]",
            "A (pre ??, ???? [[https://e.com][SEClink]]) B.",
        ),
        ("[cite/numeric:@k3]", "A (1) B."),
        // A nocite citation numbers its key: Emacs `A (2) B.`.
        ("[cite/n:@k0] [cite/nb:@k3]", "A (2) B."),
    ];
    let mut failures = Vec::new();
    for (cite, want) in cases {
        let doc = format!("A {cite} B.\n");
        let got = visible(&page(&doc));
        if got.trim() != *want {
            failures.push(format!("{cite}: {got:?}, Emacs {want:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Fail closed: a prefix or suffix holding a construct the export hides
/// shows none of the citation's affixes (Emacs shows the inline src
/// block's code there, `(??, ???? echo SEChid)`; registered: citations).
#[test]
fn a_citation_holding_a_hidden_construct_shows_no_affix() {
    for cite in [
        "[cite:@k1 src_sh[:exports none]{echo SEChid}]",
        "[cite:SECpre @k1 call_f(SEChid)]",
        "[cite:@k1 @@html:<b>SEChid</b>@@ SECsuf]",
    ] {
        let got = visible(&page(&format!("A {cite} B.\n")));
        assert_eq!(got.trim(), "A (??, ????) B.", "{cite}");
    }
}

/// No key in any output channel (the consumer's fixture 08, and keys in
/// every position), attributes and metadata included.
#[test]
fn no_citation_key_reaches_any_output() {
    let doc = "* H [cite:@SECKEYtitle]\n:PROPERTIES:\n:ID: h\n:P: [cite:@SECKEYprop]\n:END:\nA citation [cite:@SECKEYa;pre @SECKEYb suf] in the text.\n- tag [cite:@SECKEYtag] :: x\n| [cite/nb:@SECKEYcell] |\n";
    let d = parse(doc, &ParseOptions::default());
    let sem = semantics(&d);
    for opts in [
        RenderOptions::default(),
        RenderOptions::default().with_property_drawers(true),
    ] {
        let r = render(&d, &sem, &opts, &Resolver::default());
        for out in [r.to_json(TreeFormat::Html), r.to_json(TreeFormat::Hast)] {
            assert!(!out.contains("SECKEY"), "{out}");
        }
    }
    assert!(!text(&d, &sem).contains("SECKEY"));
    assert!(!mae_org::node_text(&d, &sem, "h")
        .unwrap()
        .contains("SECKEY"));
}
