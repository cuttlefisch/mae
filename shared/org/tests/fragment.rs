//! `parse_fragment` must parse a body exactly as `parse` parses the whole
//! file it comes from: same elements, spans, contents and parents (the
//! wrapping headline becomes the document), offsets shifted by the headline
//! line. The FILE's line-ending convention is passed in, because a fragment
//! cannot know it (review: a CRLF body under an LF headline is a unix file,
//! in which `#+end_src\r` closes nothing). The headline's line ending and the
//! body's are chosen independently, and bodies may start with a BOM, which
//! mid-file is content.

mod support;

use mae_org::{
    parse, parse_fragment, DiagnosticKind, Document, FragmentContext, LineEnding, ParseOptions,
    SyntaxKind, SyntaxNode,
};
use proptest::prelude::*;
use support::*;

const HEADER_ENDINGS: [&str; 3] = ["\n", "\r\n", "\r"];

/// The file `header + body`, parsed whole, as elements of the body.
fn in_file(header: &str, body: &str) -> Vec<El> {
    let p = header.len();
    let doc = parse(&format!("{header}{body}"), &ParseOptions::default());
    mae_elements(&doc)
        .into_iter()
        .filter(|e| !(e.kind == "headline" && e.begin == 0))
        .map(|e| {
            // The wrapper (at 0, before shifting) becomes the document.
            let wrapped = e.parent == ("headline".to_string(), 0);
            let mut e = shift_el(&e, |o| o.saturating_sub(p));
            if wrapped {
                e.parent = ("org-data".to_string(), 0);
            }
            e
        })
        .collect()
}

fn check(name: &str, body: &str, level: usize, eol: &str) -> Result<(), String> {
    let header = format!("{} wrapper{eol}", "*".repeat(level));
    let file_eol = LineEnding::detect(&format!("{header}{body}"));
    // A lone CR ends the headline's line only in a mac file; elsewhere it is
    // content and the "body" would start mid-headline, so there is no body.
    if eol == "\r" && file_eol != LineEnding::Cr {
        return Ok(());
    }
    let ctx = FragmentContext::new(level, file_eol);
    let frag = parse_fragment(body, &ParseOptions::default(), &ctx);
    if frag.syntax().to_string() != body {
        return Err(format!("{name} L{level}: not lossless"));
    }
    let (a, b) = diff(&in_file(&header, body), &mae_elements(&frag));
    if a.is_empty() && b.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{name} L{level} header {eol:?}:\n in file only {a:?}\n in fragment only {b:?}"
        ))
    }
}

/// The variants of a body that change how its file is read.
fn variants(text: &str) -> Vec<(&'static str, String)> {
    let mut v = vec![
        ("as stored", text.to_string()),
        ("bom", format!("\u{feff}{text}")),
    ];
    if !text.contains('\r') {
        v.push(("crlf", to_crlf(text)));
        v.push(("cr", text.replace('\n', "\r")));
    }
    v
}

#[test]
fn a_fragment_parses_as_the_same_text_in_its_file() {
    for f in fixtures() {
        let text = read(&f);
        for (what, body) in variants(&text) {
            for eol in HEADER_ENDINGS {
                check(&format!("{} ({what})", rel(&f)), &body, 2, eol)
                    .unwrap_or_else(|e| panic!("{e}"));
            }
        }
    }
    for f in bundled_corpus() {
        let text = read(&f);
        for level in 1..=3 {
            check(&rel(&f), &text, level, "\n").unwrap_or_else(|e| panic!("{e}"));
        }
    }
}

/// The review's repros: the fragment alone would be read as CRLF or mac.
#[test]
fn a_fragment_takes_the_files_line_ending_not_its_own() {
    for body in ["#+begin_src\r\nx\r\n#+end_src\r\n", "a\r* b\r"] {
        check("review", body, 1, "\n").unwrap_or_else(|e| panic!("{e}"));
        let ctx = FragmentContext::new(1, LineEnding::Lf);
        let frag = parse_fragment(body, &ParseOptions::default(), &ctx);
        assert_eq!(
            frag.blocks().count() + frag.headlines().count(),
            0,
            "{body:?} in an LF file"
        );
    }
}

#[test]
fn planning_and_properties_at_the_top_of_a_fragment_belong_to_its_headline() {
    let body = "SCHEDULED: <2026-11-01 Sun>\n:PROPERTIES:\n:ID: x\n:END:\ntext\n";
    let ctx = FragmentContext::new(2, LineEnding::Lf);
    let frag = parse_fragment(body, &ParseOptions::default(), &ctx);
    let kinds: Vec<String> = mae_elements(&frag).into_iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds[..4],
        ["section", "planning", "property-drawer", "node-property"]
    );
    let doc = parse(body, &ParseOptions::default());
    assert!(mae_elements(&doc)
        .iter()
        .all(|e| e.kind != "planning" && e.kind != "property-drawer"));
}

#[test]
fn a_headline_that_would_end_the_body_is_diagnosed() {
    let ctx = FragmentContext::new(2, LineEnding::Lf);
    let frag = parse_fragment(
        "a\n*** child\n** sibling\n* uncle\n",
        &ParseOptions::default(),
        &ctx,
    );
    let d: Vec<(DiagnosticKind, u32)> = frag
        .diagnostics()
        .iter()
        .map(|d| (d.kind, d.span.start))
        .collect();
    assert_eq!(
        d,
        vec![
            (DiagnosticKind::HeadlineOutsideFragment, 12),
            (DiagnosticKind::HeadlineOutsideFragment, 23)
        ]
    );
    // Level 0 is the start of a file: a BOM there is trivia, as in `parse`.
    let ctx = FragmentContext::new(0, LineEnding::Lf);
    let text = "\u{feff}* a\n";
    assert!(
        parse_fragment(text, &ParseOptions::default(), &ctx)
            == parse(text, &ParseOptions::default())
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3000))]

    #[test]
    fn fragments_of_org_shaped_text_match_their_file(
        parts in prop::collection::vec(prop::sample::select(&[
            "SCHEDULED: <2026-01-01 Thu>", ":PROPERTIES:", ":END:", ":ID: v", "* ", "** ", "*** ", "#+begin_src",
            "#+end_src", "- ", "  - ", "text", "# c", "#+NAME: n", "| a |", "\n", "\n", "\n", "\r\n", "\r", "  ",
            "\u{feff}",
        ][..]), 0..60),
        level in 1usize..4,
        eol in prop::sample::select(&HEADER_ENDINGS[..]),
    ) {
        check("proptest", &parts.concat(), level, eol).map_err(TestCaseError::fail)?;
    }
}

/// Every node and token of a tree, objects included: kind and span
/// relative to `base`, in order.
type Tree = Vec<(SyntaxKind, u32, u32)>;

fn flatten(roots: impl Iterator<Item = mae_org::SyntaxElement>, base: u32) -> Tree {
    let mut out = Vec::new();
    for r in roots {
        let all: Vec<mae_org::SyntaxElement> = match &r {
            mae_org::rowan::NodeOrToken::Node(n) => n.descendants_with_tokens().collect(),
            mae_org::rowan::NodeOrToken::Token(_) => vec![r.clone()],
        };
        for e in all {
            let t = e.text_range();
            out.push((
                e.kind(),
                u32::from(t.start()) - base,
                u32::from(t.end()) - base,
            ));
        }
    }
    out
}

/// Review M2: every headline body of `text`, parsed as a fragment with its
/// file's context, is the same tree (elements AND objects: a radio link to
/// a target elsewhere in the file included) as that headline's part of the
/// whole parse. Returns how many bodies were compared.
fn bodies_match_their_file(
    name: &str,
    text: &str,
    options: &ParseOptions,
) -> Result<usize, String> {
    let doc = parse(text, options);
    let mut n = 0;
    for h in doc
        .syntax()
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::HEADLINE)
    {
        let Some((base, rest)) = body_of(&h) else {
            continue;
        };
        let level = mae_org::Headline::cast(h.clone()).map_or(1, |h| h.level());
        let ctx = FragmentContext::from_document(&doc, level);
        let body = &text[base as usize..u32::from(h.text_range().end()) as usize];
        let frag = parse_fragment(body, options, &ctx);
        let want = flatten(rest.into_iter(), base);
        let got = flatten(frag.syntax().children_with_tokens(), 0);
        if want != got {
            let at = want.iter().zip(&got).position(|(a, b)| a != b);
            return Err(format!(
                "{name}: body at {base} (level {level}) differs at {at:?}: file {:?} fragment {:?}\n{body:?}",
                at.and_then(|i| want.get(i)),
                at.and_then(|i| got.get(i)),
            ));
        }
        n += 1;
    }
    Ok(n)
}

/// A headline's body: where it starts (after the headline line's ending)
/// and the children after the line. `None` when the line ends the file.
fn body_of(h: &SyntaxNode) -> Option<(u32, Vec<mae_org::SyntaxElement>)> {
    let kids: Vec<_> = h.children_with_tokens().collect();
    let nl = kids.iter().position(|k| {
        k.as_token()
            .is_some_and(|t| t.kind() == SyntaxKind::NEWLINE)
    })?;
    let base = u32::from(kids[nl].text_range().end());
    Some((base, kids[nl + 1..].to_vec()))
}

fn ctx_text(doc: &Document) -> String {
    doc.syntax().to_string()
}

#[test]
fn every_headline_body_parses_as_in_its_file() {
    let mut count = 0;
    let options = ParseOptions::default();
    let files = fixtures().into_iter().chain(bundled_corpus());
    for f in files {
        let text = read(&f);
        for t in [text.clone(), to_crlf(&text)] {
            count +=
                bodies_match_their_file(&rel(&f), &t, &options).unwrap_or_else(|e| panic!("{e}"));
        }
    }
    assert!(count > 100, "only {count} bodies compared");
}

/// The reviewer's case: a radio target before the first headline makes a
/// radio link in a headline's body. Parsed alone, the body cannot know.
#[test]
fn a_body_links_to_a_radio_target_elsewhere_in_its_file() {
    let text = "<<<Widget>>> is defined here.\n\n* H\nbody uses Widget here\n";
    let options = ParseOptions::default();
    assert_eq!(bodies_match_their_file("review", text, &options), Ok(1));
    let doc = parse(text, &options);
    assert_eq!(doc.radio_targets(), ["Widget".to_string()]);
    let body = "body uses Widget here\n";
    let links = |d: &Document| {
        d.syntax()
            .descendants()
            .filter(|n| n.kind() == SyntaxKind::LINK)
            .count()
    };
    let alone = parse_fragment(body, &options, &FragmentContext::new(1, LineEnding::Lf));
    assert_eq!(links(&alone), 0);
    let ctx = FragmentContext::from_document(&doc, 1);
    assert_eq!(links(&parse_fragment(body, &options, &ctx)), 1);
    // The setters say the same as the document.
    let by_hand = FragmentContext::new(1, LineEnding::Lf)
        .with_radio_targets(["Widget"])
        .with_todo_sequences(doc.todo_sequences().to_vec());
    assert_eq!(by_hand, ctx);
    assert_eq!(ctx_text(&parse_fragment(body, &options, &by_hand)), body);
}

/// The file's TODO keywords, declared outside the body, or inside it while
/// another `#+TODO:` elsewhere also counts.
#[test]
fn a_body_reads_the_files_todo_keywords() {
    let options = ParseOptions::default();
    for text in [
        "#+TODO: WAIT | GONE\n* H\n** WAIT child\n",
        "* H\n#+TODO: WAIT | GONE\n** NEXT child\n* I\n#+TODO: NEXT | DONE\n",
    ] {
        assert!(bodies_match_their_file("todo", text, &options).is_ok_and(|n| n >= 1));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1500))]

    /// Generated files with radio targets in and out of headlines.
    #[test]
    fn generated_bodies_with_radio_targets_parse_as_in_their_file(
        parts in prop::collection::vec(prop::sample::select(&[
            "<<<widget>>> ", "<<<a b>>> ", "<<<x\u{a0}y>>> ", "widget ", "Widget ", "a b ", "a  b ",
            "x\u{a0}y ", "*widget* ", "[[widget]] ", "* ", "** ", "*** ", "\n", "\n", "\n", "text ",
            "#+TODO: WAIT | GONE\n", "WAIT ", ":PROPERTIES:\n:ID: x\n:END:\n", "- item ",
        ][..]), 0..40),
    ) {
        let text = parts.concat();
        bodies_match_their_file("generated", &text, &ParseOptions::default())
            .map_err(TestCaseError::fail)?;
    }
}
