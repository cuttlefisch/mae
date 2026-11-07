//! `highlights` / `sections` / `section_highlights`: a section re-highlighted
//! alone, with its file's context, gives exactly the spans the whole parse
//! gives there; and an edit that changes what the rest of the file reads is
//! refused (`NeedsFull`) rather than answered wrongly.

mod support;

use mae_org::{
    highlights, parse, section_highlights, sections, Highlight, NeedsFull, ParseOptions,
    SectionHighlights, SyntaxKind,
};
use proptest::prelude::*;
use support::*;

/// The whole parse's spans inside `[start, end)`.
fn whole_within(all: &[Highlight], start: u32, end: u32) -> Vec<Highlight> {
    all.iter()
        .filter(|h| h.start >= start && h.end <= end)
        .copied()
        .collect()
}

/// Every section of `text`, re-highlighted alone and unchanged, matches the
/// whole parse. Returns how many sections were compared.
fn sections_match(name: &str, text: &str, options: &ParseOptions) -> Result<usize, String> {
    let doc = parse(text, options);
    let all = highlights(&doc);
    let secs = sections(&doc);
    for s in &secs {
        let region = &text[s.start as usize..s.end as usize];
        let want = whole_within(&all, s.start, s.end);
        match section_highlights(region, options, s) {
            SectionHighlights::Spans(got) if got == want => {}
            SectionHighlights::Spans(got) => {
                let at = want.iter().zip(&got).position(|(a, b)| a != b);
                return Err(format!(
                    "{name}: section {}..{} differs at {at:?}: whole {:?} alone {:?}\n{region:?}",
                    s.start,
                    s.end,
                    at.and_then(|i| want.get(i)).or(want.get(got.len())),
                    at.and_then(|i| got.get(i)).or(got.get(want.len())),
                ));
            }
            SectionHighlights::NeedsFull(why) => {
                return Err(format!(
                    "{name}: unchanged section {}..{} refused: {why:?}\n{region:?}",
                    s.start, s.end
                ))
            }
        }
    }
    Ok(secs.len())
}

#[test]
fn every_section_of_the_corpus_highlights_alone_as_in_its_file() {
    let options = ParseOptions::default();
    let mut count = 0;
    for f in fixtures().into_iter().chain(bundled_corpus()) {
        let text = read(&f);
        for t in [text.clone(), to_crlf(&text)] {
            count += sections_match(&rel(&f), &t, &options).unwrap_or_else(|e| panic!("{e}"));
        }
    }
    assert!(count > 500, "only {count} sections compared");
}

/// The sections tile the text between headline lines: each starts after a
/// headline's line or at 0, and none overlaps the next.
#[test]
fn sections_are_ordered_and_disjoint() {
    for f in fixtures() {
        let text = read(&f);
        let secs = sections(&parse(&text, &ParseOptions::default()));
        assert_eq!(secs[0].start, 0, "{}", rel(&f));
        for w in secs.windows(2) {
            assert!(
                w[0].end <= w[1].start,
                "{}: {:?}",
                rel(&f),
                (w[0].end, w[1].start)
            );
        }
    }
}

#[test]
fn plain_text_is_left_out_and_markup_is_kept() {
    let doc = parse("* TODO a :t:\nsome *bold* text\n", &ParseOptions::default());
    let kinds: Vec<SyntaxKind> = highlights(&doc).iter().map(|h| h.kind).collect();
    for k in [
        SyntaxKind::HEADLINE,
        SyntaxKind::TODO_KEYWORD,
        SyntaxKind::TAGS,
        SyntaxKind::BOLD,
    ] {
        assert!(kinds.contains(&k), "{k:?} missing from {kinds:?}");
    }
    for k in [
        SyntaxKind::TEXT,
        SyntaxKind::WHITESPACE,
        SyntaxKind::NEWLINE,
        SyntaxKind::BLANK_LINE,
    ] {
        assert!(!kinds.contains(&k), "{k:?} kept");
    }
}

const FILE: &str =
    "#+TODO: TODO | DONE\nintro\n* A\nalpha <<<radio>>> text\n** A1\nchild\n* B\nbeta radio here\n";

/// The section of `FILE` whose region starts with `starts`, edited to `new`.
fn edit(starts: &str, new: &str) -> SectionHighlights {
    let options = ParseOptions::default();
    let doc = parse(FILE, &options);
    let s = sections(&doc)
        .into_iter()
        .find(|s| FILE[s.start as usize..].starts_with(starts))
        .expect("section");
    section_highlights(new, &options, &s)
}

#[test]
fn an_edit_that_changes_what_the_file_reads_needs_a_full_parse() {
    let full = |w| SectionHighlights::NeedsFull(w);
    assert_eq!(edit("child", "child\n* new\n"), full(NeedsFull::Headline));
    assert_eq!(
        edit("child", "child\n*** deeper\n"),
        full(NeedsFull::Headline)
    );
    assert_eq!(edit("alpha", "alpha text\n"), full(NeedsFull::Declarations));
    assert_eq!(
        edit("child", "child <<<other>>>\n"),
        full(NeedsFull::Declarations)
    );
    assert_eq!(
        edit("#+TODO", "#+TODO: A | B\nintro\n"),
        full(NeedsFull::Declarations)
    );
    assert_eq!(edit("child", "child"), full(NeedsFull::Boundary));
    // Mid-file a byte-order mark is content: it would join the next line.
    assert_eq!(edit("child", "\u{feff}"), full(NeedsFull::Boundary));
}

#[test]
fn an_edit_inside_a_section_is_answered_and_matches_a_full_parse() {
    let new = "child with *bold* and ~code~\n";
    let SectionHighlights::Spans(got) = edit("child", new) else {
        panic!("refused")
    };
    let text = FILE.replacen("child\n", new, 1);
    let doc = parse(&text, &ParseOptions::default());
    let s = sections(&doc)
        .into_iter()
        .find(|s| text[s.start as usize..].starts_with("child"))
        .unwrap();
    assert_eq!(got, whole_within(&highlights(&doc), s.start, s.end));
    assert!(got.iter().any(|h| h.kind == SyntaxKind::BOLD));
}

/// The last section, not followed by a headline, may end without a line
/// ending.
#[test]
fn the_last_section_may_end_without_a_line_ending() {
    assert!(matches!(
        edit("beta", "beta radio here"),
        SectionHighlights::Spans(_)
    ));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn sections_of_org_shaped_text_match_their_file(
        parts in prop::collection::vec(prop::sample::select(&[
            "SCHEDULED: <2026-01-01 Thu>", ":PROPERTIES:", ":END:", ":ID: v", "* ", "** ", "*** ",
            "#+begin_src", "#+end_src", "- ", "  - ", "text", "*b*", "<<<r>>>", " r ", "# c",
            "#+NAME: n", "#+TODO: X | Y", "X ", "| a |", "[[x]]", "\n", "\n", "\n", "\r\n", "  ",
        ][..]), 0..80),
    ) {
        let text = parts.concat();
        sections_match("proptest", &text, &ParseOptions::default())
            .map_err(TestCaseError::fail)?;
    }
}
