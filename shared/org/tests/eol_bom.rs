//! Line endings and the byte-order mark: the two places mae-org follows how
//! Emacs DECODES a file for a user rather than the oracle's raw-byte view
//! (see conformance/divergences.toml).

mod support;

use mae_org::{Headline, SyntaxKind};
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
    let shifted: Vec<El> = plain
        .iter()
        .map(|e| El {
            kind: e.kind.clone(),
            begin: e.begin + 3,
            end: e.end + 3,
        })
        .collect();
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

#[test]
fn mixed_line_endings_round_trip_and_split_only_at_lf() {
    let text = read(&repo_root().join("shared/org/tests/fixtures/mixed-eol.org"));
    let doc = parse_default(&text);
    assert_eq!(doc.syntax().to_string(), text);
    let titles: Vec<String> = doc.headlines().map(|h| h.title()).collect();
    assert_eq!(titles, vec!["LF heading", "Next"]);
}
