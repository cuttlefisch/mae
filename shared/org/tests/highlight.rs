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

/// `(kind, start, end, own_end)` of every `kind` span in `text`.
fn spans_of_kind(text: &str, kind: SyntaxKind) -> Vec<(u32, u32, u32)> {
    highlights(&parse(text, &ParseOptions::default()))
        .into_iter()
        .filter(|h| h.kind == kind)
        .map(|h| (h.start, h.end, h.own_end))
        .collect()
}

/// An object's `end` keeps its post-blank, as `org-element` reports it;
/// `own_end` stops after its closing markup. The offsets are a browser
/// client's expectations for styling a node and for "is the cursor in it".
#[test]
fn an_objects_own_end_excludes_the_spaces_after_it() {
    use SyntaxKind::*;
    assert_eq!(spans_of_kind("un *negrita* y más\n", BOLD), [(3, 13, 12)]);
    assert_eq!(
        spans_of_kind("ver [[id:n1][Uno]] y más\n", LINK),
        [(4, 19, 18)]
    );
    let t = "~code~ y =verb= y /it/ fin\n";
    assert_eq!(spans_of_kind(t, CODE), [(0, 7, 6)]);
    assert_eq!(spans_of_kind(t, VERBATIM), [(9, 16, 15)]);
    assert_eq!(spans_of_kind(t, ITALIC), [(18, 23, 22)]);
    assert_eq!(
        spans_of_kind("a +s+   b _u_\tc\n", STRIKE_THROUGH),
        [(2, 8, 5)]
    );
    assert_eq!(
        spans_of_kind("a +s+   b _u_\tc\n", UNDERLINE),
        [(10, 14, 13)]
    );
    // Punctuation, not space, follows: no post-blank, so the ends agree.
    let p = "*a*, [[https://x.example][web]].\n";
    assert_eq!(spans_of_kind(p, BOLD), [(0, 3, 3)]);
    assert_eq!(spans_of_kind(p, LINK), [(5, 31, 31)]);
}

/// An element's post-blank is the blank lines after it: its own end keeps
/// its last line ending and drops them.
#[test]
fn an_elements_own_end_excludes_the_blank_lines_after_it() {
    use SyntaxKind::*;
    assert_eq!(spans_of_kind("para\n\n\nnext\n", PARAGRAPH)[0], (0, 7, 5));
    let src = "#+begin_src sh\necho\n#+end_src\n\n\nAfter\n";
    assert_eq!(spans_of_kind(src, SRC_BLOCK), [(0, 32, 30)]);
}

/// Over every fixture and the bundled corpus: `own_end` lies within the
/// span, only spaces, tabs and line endings follow it, and a token's equals
/// its end.
#[test]
fn own_end_only_ever_drops_trailing_blank() {
    for f in fixtures().into_iter().chain(bundled_corpus()) {
        let text = read(&f);
        for h in highlights(&parse(&text, &ParseOptions::default())) {
            let (s, o, e) = (h.start as usize, h.own_end as usize, h.end as usize);
            assert!(s <= o && o <= e, "{f:?}: {h:?}");
            assert!(
                text[o..e]
                    .chars()
                    .all(|c| matches!(c, ' ' | '\t' | '\n' | '\r')),
                "{f:?}: {h:?} drops {:?}",
                &text[o..e]
            );
            if !h.kind.is_node() {
                assert_eq!(h.own_end, h.end, "{f:?}: token {h:?}");
            }
        }
    }
}

/// The edit a browser client's property test found: in a CRLF file, an LF typed between
/// a CR and its LF makes the file unix (a lone LF anywhere does, as Emacs
/// decides it), a fact about the whole file the region alone cannot carry.
/// So the section asks for a full parse. Kills: answering from the region.
#[test]
fn an_edit_that_changes_the_files_line_ending_needs_a_full_parse() {
    let text = "#+title: T\r\n\r\n* A uno\r\ntexto\r\nmás\r\n* Dos\r\nfin\r\n";
    let doc = parse(text, &ParseOptions::default());
    assert_eq!(doc.line_ending(), mae_org::LineEnding::CrLf);
    let s = sections(&doc).into_iter().nth(1).unwrap();
    let region = &text[s.start as usize..s.end as usize];
    let cr = region.find('\r').unwrap();
    for insert in ["x\n", "#+TODO: A\n"] {
        let new = format!("{}{insert}{}", &region[..=cr], &region[cr + 1..]);
        let whole = format!(
            "{}{new}{}",
            &text[..s.start as usize],
            &text[s.end as usize..]
        );
        assert_eq!(
            parse(&whole, &ParseOptions::default()).line_ending(),
            mae_org::LineEnding::Lf
        );
        assert!(
            matches!(
                section_highlights(&new, &ParseOptions::default(), &s),
                SectionHighlights::NeedsFull(_)
            ),
            "{insert:?} answered from the region"
        );
    }
}

/// A headline line ending in a multi-byte character: the terminator before
/// the next region is read without slicing inside that character. Each
/// input panicked in 0.5.0, and the anchor read is the terminator itself.
#[test]
fn a_multibyte_character_before_the_line_ending_is_read_whole() {
    for (text, want) in [
        ("* ñ\n", mae_org::LineEnding::Lf),
        ("* €\n", mae_org::LineEnding::Lf),
        ("* 😀\n", mae_org::LineEnding::Lf),
        ("* 𝄞\n", mae_org::LineEnding::Lf),
        ("* a😀\n", mae_org::LineEnding::Lf),
        ("** 😀\n", mae_org::LineEnding::Lf),
        ("texto\n* 😀\n", mae_org::LineEnding::Lf),
        ("* ñ\r\n", mae_org::LineEnding::CrLf),
        ("* 😀\rmás\r", mae_org::LineEnding::Cr),
    ] {
        let doc = parse(text, &ParseOptions::default());
        let after_heading: Vec<_> = sections(&doc).into_iter().filter(|s| s.start > 0).collect();
        assert!(!after_heading.is_empty(), "{text:?}");
        for s in after_heading {
            assert_eq!(s.eol_anchor, Some(want), "{text:?}");
        }
        assert!(
            sections_match(text, text, &ParseOptions::default()).is_ok(),
            "{text:?}"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3000))]

    /// Incremental equals full: after a random edit inside a random section
    /// of org-shaped text with every line-ending convention, either the
    /// section asks for a full parse, or its spans equal a full parse of the
    /// edited file and the file's line ending is unchanged. Kills an edit
    /// answered from the region that changed a fact about the whole file.
    #[test]
    fn an_answered_section_edit_equals_a_full_parse(
        parts in prop::collection::vec(prop::sample::select(&[
            "* ", "** ", "text", "*b*", "<<<r>>>", " r ", "#+TODO: X | Y", "X ", "- ", "  ",
            "\n", "\n", "\r\n", "\r\n", "\r", "ñ", "😀",
        ][..]), 1..40),
        pick in any::<prop::sample::Index>(),
        at in any::<prop::sample::Index>(),
        cut in 0usize..4,
        insert in prop::sample::select(&["", "\n", "\r", "\r\n", "x", "*y* ", "<<<r>>>", "é", "😀\n"][..]),
    ) {
        let text = parts.concat();
        let opts = ParseOptions::default();
        let doc = parse(&text, &opts);
        let secs = sections(&doc);
        let s = &secs[pick.index(secs.len())];
        let region = &text[s.start as usize..s.end as usize];
        let bounds: Vec<usize> = (0..=region.len()).filter(|i| region.is_char_boundary(*i)).collect();
        let a = bounds[at.index(bounds.len())];
        let b = bounds.iter().copied().filter(|&i| i >= a).nth(cut).unwrap_or(a);
        let new = format!("{}{insert}{}", &region[..a], &region[b..]);
        if let SectionHighlights::Spans(got) = section_highlights(&new, &opts, s) {
            let whole = format!("{}{new}{}", &text[..s.start as usize], &text[s.end as usize..]);
            let full = parse(&whole, &opts);
            prop_assert_eq!(full.line_ending(), doc.line_ending(), "the convention changed: {:?}", whole);
            let end = s.start + new.len() as u32;
            prop_assert_eq!(got, whole_within(&highlights(&full), s.start, end), "{:?}", whole);
        }
    }
}
