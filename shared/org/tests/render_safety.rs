//! The renderer's safety properties (ADR-112 D3), over the bundled corpus,
//! every fixture and generated documents, each against the independent
//! oracle in `render_check`:
//! - **Closure:** every `href` is a resolver-supplied target (one the
//!   contract accepts), an http/https/mailto URL, or `#ID` of an id in the
//!   same output; every `src` an http/https URL with images on.
//! - **Allowlist:** every tag and attribute is documented, every value in
//!   its validated form, every id unique.
//! - **Unrendered regions contribute nothing:** a marker placed in every
//!   unrendered construct (as text, id, title and link target) appears
//!   nowhere in the hast, the HTML or the text.

mod render_check;
mod support;

use mae_org::{
    parse, render, semantics, HeadlineIds, ParseOptions, RenderOptions, Rendered, Resolver,
    TreeFormat,
};
use proptest::prelude::*;
use render_check::{adversarial_resolver, check, from_tree, kb_targets, Contract};
use std::path::Path;

fn option_sets() -> Vec<RenderOptions> {
    vec![
        RenderOptions::default(),
        RenderOptions::new(HeadlineIds::Slug, true, true),
        RenderOptions::new(HeadlineIds::Explicit, true, false),
    ]
}

/// Renders `text` under every option set with the adversarial resolver and
/// checks every tree in the result (body, titles, aliases, properties).
fn check_all(text_in: &str) -> Result<(), String> {
    let doc = parse(text_in, &ParseOptions::default());
    let sem = semantics(&doc);
    let resolver = adversarial_resolver(&kb_targets(&sem));
    for opts in option_sets() {
        let out = render(&doc, &sem, &opts, &resolver);
        let contract = Contract::new(&resolver, opts.images);
        check(&from_tree(&out.body), &contract)?;
        for n in &out.nodes {
            check(&from_tree(&n.title), &contract)?;
            for a in &n.aliases {
                check(&from_tree(a), &contract)?;
            }
            for (_, v) in &n.properties {
                check(&from_tree(v), &contract)?;
            }
            if let Some(a) = &n.anchor {
                if !contains_id(&out, a) {
                    return Err(format!("node {} has anchor {a} not in the body", n.id));
                }
            }
        }
    }
    Ok(())
}

fn contains_id(out: &Rendered, id: &str) -> bool {
    out.to_json(TreeFormat::Html)
        .contains(&format!("id=\\\"{id}\\\""))
}

fn corpus() -> Vec<std::path::PathBuf> {
    let mut v = support::bundled_corpus();
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    for d in ["fixtures", "semantics", "render"] {
        v.extend(support::org_files(&here.join(d)));
    }
    v
}

#[test]
fn corpus_and_fixtures_keep_closure_and_the_allowlist() {
    let files = corpus();
    assert!(files.len() > 300, "corpus went missing: {}", files.len());
    let mut failures = Vec::new();
    for f in &files {
        let Ok(t) = std::fs::read_to_string(f) else {
            continue;
        };
        if let Err(e) = check_all(&t) {
            failures.push(format!("{}: {e}", support::rel(f)));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Hostile inputs, one assertion each, so a regression names the input.
#[test]
fn hostile_inputs_never_reach_an_attribute_or_markup() {
    let cases: &[(&str, &str)] = &[
        ("[[javascript:alert(1)][x]]", "javascript:"),
        ("[[JaVaScRiPt:alert(1)][x]]", "JaVaScRiPt:"),
        ("[[data:text/html,<script>alert(1)</script>][x]]", "data:"),
        ("[[vbscript:msgbox(1)][x]]", "vbscript:"),
        ("[[&#106;avascript:alert(1)][x]]", "&#106;"),
        ("[[ javascript:alert(1)][x]]", "javascript:"),
        ("[[\u{1}javascript:alert(1)][x]]", "javascript:"),
        ("[[\tjavascript:alert(1)][x]]", "javascript:"),
        ("[[//evil.example/x][x]]", "evil.example"),
        (
            "[[https://x.org/\"onmouseover=alert(1)][x]]",
            "\"onmouseover",
        ),
        ("[[https://x.org/a>b<c]]", "a>b<c"),
        ("[[https://x.org/a\nb][x]]", "a\nb"),
        ("@@html:<script>alert(1)</script>@@", "<script"),
        (
            "#+begin_export html\n<script>alert(1)</script>\n#+end_export\n",
            "<script",
        ),
        ("#+HTML: <script>alert(1)</script>\n", "<script"),
        ("#+ATTR_HTML: :onclick alert(1)\nparagraph\n", "onclick"),
        ("plain <script>alert(1)</script>\n", "<script"),
        (
            "#+begin_src \"><script>alert(1)</script>\nx\n#+end_src\n",
            "<script",
        ),
        ("* h\n:PROPERTIES:\n:ID: a\"b<c d\n:END:\n", "a\"b"),
        (
            "* h\n:PROPERTIES:\n:CUSTOM_ID: x\" onclick=\"y\n:END:\n",
            "onclick=",
        ),
        ("#+begin_x\"><script>\ny\n#+end_x\"><script>\n", "<script"),
    ];
    for (input, needle) in cases {
        let doc = parse(input, &ParseOptions::default());
        let sem = semantics(&doc);
        let opts = RenderOptions::new(HeadlineIds::Explicit, true, true);
        let out = render(&doc, &sem, &opts, &Resolver::default());
        let html = mae_org::to_html(&out.body);
        assert!(
            needle_only_in_text(&html, needle),
            "{input:?} leaked {needle:?} into markup: {html}"
        );
        assert!(!html.contains("<script"), "{input:?}: {html}");
        assert!(needle_only_in_text(&html, "onclick="), "{input:?}: {html}");
        check(
            &from_tree(&out.body),
            &Contract::new(&Resolver::default(), true),
        )
        .unwrap_or_else(|e| panic!("{input:?}: {e}\n{html}"));
    }
}

/// The protocol allowlist is the renderer's own, not the declared link
/// types': a KB that declares `javascript` or `data` a web scheme (ADR-112
/// D4 lets it) still gets no such href.
#[test]
fn a_declared_web_scheme_is_not_an_allowed_protocol() {
    let mut opts = mae_org::SemanticsOptions::default();
    opts.link_types
        .web
        .extend(["javascript".into(), "data".into(), "ftp".into()]);
    let input = "[[javascript:alert(1)][a]] [[data:text/html,x][b]] [[ftp://x.org/f][c]]\n";
    let doc = parse(input, &ParseOptions::default());
    let sem = mae_org::semantics_with(&doc, &opts);
    assert!(
        sem.links.iter().all(|l| l.kind == mae_org::LinkKind::Web),
        "{sem:#?}"
    );
    let out = render(&doc, &sem, &RenderOptions::default(), &Resolver::default());
    let html = mae_org::to_html(&out.body);
    assert!(!html.contains("href"), "{html}");
    check(
        &from_tree(&out.body),
        &Contract::new(&Resolver::default(), false),
    )
    .unwrap();
}

/// `needle` (escaped as HTML text would escape it) occurs only between
/// tags, never inside one.
fn needle_only_in_text(html: &str, needle: &str) -> bool {
    let escaped = needle
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let mut inside = false;
    let mut tags = String::new();
    for c in html.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            c if inside => tags.push(c),
            _ => {}
        }
    }
    !tags.contains(needle) && !tags.contains(&escaped)
}

/// Org snippets the generators combine: every construct the renderer
/// models, the hostile shapes above, and links to a small pool of ids so
/// that some resolve, some are same-page and some are refused.
const PIECES: &[&str] = &[
    "Plain text with *bold* /it/ =v= ~c~ x^2 \\alpha \\\\\n",
    "* H1 :tag:\n",
    "** H2\n:PROPERTIES:\n:ID: n1\n:CUSTOM_ID: c1\n:END:\n",
    "*** TODO [#A] H3 [1/2]\n:PROPERTIES:\n:ID: n2\n:END:\n",
    "* H1\n",
    "[[id:n1][to n1]] [[id:n2]] [[n3]] [[id:n4?rel=r&w=0.3#f][meta]] [[*H1]] [[#c1]] [[#nope]]\n",
    "[[https://x.org/a b][web]] <https://y.org> mailto:a@b.c [[file:x.png]] [[https://x.org/i.png]]\n",
    "[[javascript:alert(1)]] [[//evil/x][pr]] [[ data:x][d]] [[https://x\"y]]\n",
    "<<tgt>> <<<radio>>> radio [[tgt]]\n",
    "Note[fn:1] and[fn:2] and[fn::anon] and[fn:x:inline]\n",
    "[fn:1] Def one [[id:n1]].\n\n",
    "[fn:2] Def two refers[fn:3].\n\n[fn:3] Three.\n\n",
    "- item\n  :PROPERTIES:\n  :ID: i1\n  :END:\n- [X] two\n  1. nested\n",
    "- tag :: desc\n- [@4] other\n",
    "| a | b |\n|---+---|\n| 1 | 2 |\n",
    "#+CAPTION: cap\n#+ATTR_HTML: :onclick x\n| c |\n",
    "#+begin_src \"><x\ncode\n#+end_src\n",
    "#+begin_quote\nq\n#+end_quote\n#+begin_verse\n  v\n#+end_verse\n",
    "#+begin_foo\"<\nbar\n#+end_foo\"<\n",
    ": fixed\n-----\n\\begin{x}\ny\n\\end{x}\n",
    "#+TRANSCLUDE: n1\n#+TRANSCLUDE: [[id:n2][T]] :level 1\n",
    "@@html:<b>@@ {{{m}}} [cite:@k] src_py{x} call_f() <2026-01-01>\n",
    "# comment [[id:n1]]\n",
    ":LOGBOOK:\nlog <<tgt2>>\n:END:\n",
    "\n\n",
    "\r\n",
];

fn org_doc() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(PIECES), 0..24).prop_map(|v| v.concat())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn generated_documents_keep_closure_and_the_allowlist(doc in org_doc()) {
        prop_assert!(check_all(&doc).is_ok(), "{:?}: {:?}", doc, check_all(&doc));
    }

    #[test]
    fn arbitrary_text_keeps_closure_and_the_allowlist(doc in any::<String>()) {
        prop_assert!(check_all(&doc).is_ok(), "{:?}: {:?}", doc, check_all(&doc));
    }
}
