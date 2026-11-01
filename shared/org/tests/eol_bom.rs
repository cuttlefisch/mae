//! Line endings and the byte-order mark, as Emacs decodes them when it visits
//! a file (the oracle decodes the same way and maps offsets back to bytes)
//! (see conformance/divergences.toml).

mod support;

use mae_org::{Headline, LineEnding, SyntaxKind};
use support::*;

#[test]
fn bom_is_trivia_so_the_first_line_stays_at_column_0() {
    let text = read(&repo_root().join("shared/org/tests/fixtures/bom.org"));
    assert!(text.starts_with('\u{feff}'), "fixture must keep its BOM");
    let doc = parse_default(&text);
    assert_eq!(doc.syntax().to_string(), text);
    let first = doc.syntax().first_token().expect("tokens");
    assert_eq!((first.kind(), first.text()), (SyntaxKind::BOM, "\u{feff}"));
    let h: Vec<Headline> = doc.headlines().collect();
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].title(), "Heading after BOM");
    assert_eq!(usize::from(h[0].syntax().text_range().start()), 3);
    // Without the BOM the same text is the same tree, shifted by 3.
    let plain = mae_elements(&parse_default(&text['\u{feff}'.len_utf8()..]));
    let shifted: Vec<El> = plain.iter().map(|e| shift_el(e, |o| o + 3)).collect();
    assert_eq!(mae_elements(&doc), shifted);
}

#[test]
fn a_bom_anywhere_but_the_start_is_content() {
    let doc = parse_default("x\n\u{feff}* not a headline\n");
    assert_eq!(doc.headlines().count(), 0);
}

#[test]
fn crlf_is_one_line_ending_and_a_lone_cr_is_content() {
    let text = read(&repo_root().join("shared/org/tests/fixtures/crlf.org"));
    assert!(text.contains("\r\n"), "fixture must keep its CRLFs");
    let doc = parse_default(&text);
    assert_eq!(doc.syntax().to_string(), text);
    let h = doc.headlines().next().expect("headline");
    assert_eq!(
        h.tags(),
        vec!["tag".to_string()],
        "the CR is not part of the tag line"
    );
    // The CRLF variant of the LF text has the LF text's structure.
    let lf = text.replace("\r\n", "\n");
    let want: Vec<El> = mae_elements(&parse_default(&lf))
        .iter()
        .map(|e| lf_to_crlf(&lf, e))
        .collect();
    assert_eq!(mae_elements(&doc), want);
    // A lone CR does not end a line: "a\rb" is one paragraph line, and a
    // headline after a lone CR is not at column 0.
    let doc = parse_default("a\r* b\n");
    assert_eq!(doc.headlines().count(), 0);
    let newlines = doc
        .syntax()
        .descendants_with_tokens()
        .filter(|e| e.kind() == SyntaxKind::NEWLINE)
        .count();
    assert_eq!(newlines, 1);
}

/// One lone LF anywhere makes the whole file unix, so EVERY CR is content
/// (Emacs `find-file`; the reviewer's repro and the oracle agree): a tag
/// followed by CR is not a tag, and a `#+end_src` followed by CR does not
/// close its block.
#[test]
fn a_mixed_file_is_unix_and_every_cr_is_content() {
    let text = read(&repo_root().join("shared/org/tests/fixtures/mixed-eol.org"));
    let doc = parse_default(&text);
    assert_eq!(doc.line_ending(), LineEnding::Lf);
    assert_eq!(doc.syntax().to_string(), text);
    let h: Vec<(String, Vec<String>)> = doc.headlines().map(|h| (h.title(), h.tags())).collect();
    assert_eq!(h[0], ("LF heading".to_string(), vec!["lf".to_string()]));
    assert_eq!(
        h[1],
        ("CRLF heading :crlf:".to_string(), vec![]),
        "the tag line ends in CR, so no tags; org-trim drops the CR from the title"
    );
    assert_eq!(
        doc.blocks().count(),
        0,
        "`#+end_src\\r` closes nothing in a unix file"
    );
    let tag = parse_default(&read(
        &repo_root().join("shared/org/tests/fixtures/eol-mixed-tag.org"),
    ));
    assert!(tag.headlines().next().is_some_and(|h| h.tags().is_empty()));
}

/// A file with only CRs is mac: CR ends lines.
#[test]
fn a_cr_only_file_is_mac_and_cr_ends_lines() {
    let text = read(&repo_root().join("shared/org/tests/fixtures/eol-cr-only.org"));
    let doc = parse_default(&text);
    assert_eq!(doc.line_ending(), LineEnding::Cr);
    assert_eq!(doc.syntax().to_string(), text);
    let h: Vec<(String, Vec<String>)> = doc.headlines().map(|h| (h.title(), h.tags())).collect();
    assert_eq!(
        h,
        vec![
            ("a".to_string(), vec![]),
            ("b".to_string(), vec!["t".to_string()])
        ]
    );
    assert_eq!(doc.blocks().count(), 1);
}

/// DOS needs every LF to follow a CR; a lone CR inside a DOS file is content.
#[test]
fn a_dos_file_keeps_a_lone_cr_as_content() {
    let text = read(&repo_root().join("shared/org/tests/fixtures/eol-dos-lone-cr.org"));
    let doc = parse_default(&text);
    assert_eq!(doc.line_ending(), LineEnding::CrLf);
    assert_eq!(
        doc.headlines().next().map(|h| h.tags()),
        Some(vec!["tag".to_string()])
    );
    let para = doc
        .syntax()
        .descendants()
        .find(|n| n.kind() == SyntaxKind::PARAGRAPH)
        .expect("paragraph");
    assert!(para.to_string().contains("lone\rCR"));
}
