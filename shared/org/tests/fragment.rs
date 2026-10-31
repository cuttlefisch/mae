//! `parse_fragment` must parse a body exactly as `parse` parses the whole
//! file it comes from: same elements, spans, contents and parents (the
//! wrapping headline becomes the document), offsets shifted by the headline
//! line. The FILE's line-ending convention is passed in, because a fragment
//! cannot know it (review: a CRLF body under an LF headline is a unix file,
//! in which `#+end_src\r` closes nothing). The headline's line ending and the
//! body's are chosen independently, and bodies may start with a BOM, which
//! mid-file is content.

mod support;

use mae_org::{parse, parse_fragment, DiagnosticKind, FragmentContext, LineEnding, ParseOptions};
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
    let ctx = FragmentContext {
        level,
        line_ending: file_eol,
    };
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
        let ctx = FragmentContext {
            level: 1,
            line_ending: LineEnding::Lf,
        };
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
    let ctx = FragmentContext {
        level: 2,
        line_ending: LineEnding::Lf,
    };
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
    let ctx = FragmentContext {
        level: 2,
        line_ending: LineEnding::Lf,
    };
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
    let ctx = FragmentContext {
        level: 0,
        line_ending: LineEnding::Lf,
    };
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
