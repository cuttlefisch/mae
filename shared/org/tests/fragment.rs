//! `parse_fragment(text, level)` must parse `text` exactly as `parse` parses
//! it when it sits under a level-`level` headline in a document: same
//! elements, spans, contents and parents (the wrapping headline becomes the
//! document), offsets shifted by the headline line. Checked over every
//! fixture and bundled file at levels 1–3, and on random org-shaped input.

mod support;

use mae_org::{parse, parse_fragment, DiagnosticKind, LineEnding, ParseOptions};
use proptest::prelude::*;
use support::*;

/// The elements of `text` parsed under a level-`level` headline, mapped back
/// to `text`'s offsets, with the wrapping headline removed.
fn under_headline(text: &str, level: usize) -> Vec<El> {
    let eol = match LineEnding::detect(text) {
        LineEnding::CrLf => "\r\n",
        LineEnding::Cr => "\r",
        LineEnding::Lf => "\n",
    };
    let prefix = format!("{} wrapper{eol}", "*".repeat(level));
    let p = prefix.len();
    let doc = parse(&format!("{prefix}{text}"), &ParseOptions::default());
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

fn check(name: &str, text: &str, level: usize) -> Result<(), String> {
    if text.starts_with('\u{feff}') {
        return Ok(()); // mid-file a BOM is content; at a fragment's start, trivia
    }
    let frag = parse_fragment(text, &ParseOptions::default(), level);
    if frag.syntax().to_string() != text {
        return Err(format!("{name} L{level}: not lossless"));
    }
    let (a, b) = diff(&under_headline(text, level), &mae_elements(&frag));
    if a.is_empty() && b.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{name} L{level}:\n in document only {a:?}\n in fragment only {b:?}"
        ))
    }
}

#[test]
fn a_fragment_parses_as_the_same_text_under_a_headline() {
    let mut files = fixtures();
    files.extend(bundled_corpus());
    for f in &files {
        let text = read(f);
        for level in 1..=3 {
            check(&rel(f), &text, level).unwrap_or_else(|e| panic!("{e}"));
        }
    }
}

#[test]
fn planning_and_properties_at_the_top_of_a_fragment_belong_to_its_headline() {
    let body = "SCHEDULED: <2026-11-01 Sun>\n:PROPERTIES:\n:ID: x\n:END:\ntext\n";
    let frag = parse_fragment(body, &ParseOptions::default(), 2);
    let kinds: Vec<String> = mae_elements(&frag).into_iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds[..4],
        ["section", "planning", "property-drawer", "node-property"]
    );
    // As a document the same text has no planning and a plain drawer.
    let doc = parse(body, &ParseOptions::default());
    assert!(mae_elements(&doc)
        .iter()
        .all(|e| e.kind != "planning" && e.kind != "property-drawer"));
}

#[test]
fn a_headline_that_would_end_the_body_is_diagnosed() {
    let frag = parse_fragment(
        "a\n*** child\n** sibling\n* uncle\n",
        &ParseOptions::default(),
        2,
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
    assert!(
        parse_fragment("a\n", &ParseOptions::default(), 0)
            == parse("a\n", &ParseOptions::default())
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn fragments_of_org_shaped_text_match_their_document_parse(
        parts in prop::collection::vec(prop::sample::select(&[
            "SCHEDULED: <2026-01-01 Thu>", ":PROPERTIES:", ":END:", ":ID: v", "* ", "** ", "*** ", "#+begin_src",
            "#+end_src", "- ", "  - ", "text", "# c", "#+NAME: n", "| a |", "\n", "\n", "\n", "\r\n", "  ",
        ][..]), 0..60),
        level in 1usize..4,
    ) {
        let text = parts.concat();
        check("proptest", &text, level).map_err(TestCaseError::fail)?;
    }
}
