//! Minting `CUSTOM_ID`s (`mae_org::custom_id_edits` and friends).
//!
//! The placement oracle is Emacs: `conformance/custom-id-put.el` runs
//! `org-entry-put` over every `tests/custom-id/NAME.org` and records
//! `NAME.expected.org`; the edits, given the same values, must produce the
//! same bytes. The rest pins what Emacs cannot: validation, scope, the
//! duplicate report and replacement.

use mae_org::{
    custom_id_edits, custom_id_edits_with, duplicate_custom_ids, headings_missing_custom_id,
    is_custom_id_uuid, replace_custom_id, CustomIdError, ParseOptions, Span, TextEdit,
};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/custom-id");

/// The script's N-th value (from 1).
fn uuid(n: usize) -> String {
    format!("00000000-0000-4000-8000-{n:012x}")
}

fn opts() -> ParseOptions {
    ParseOptions::default()
}

fn apply(text: &str, edits: &[TextEdit]) -> String {
    let mut out = text.to_string();
    let mut last = usize::MAX;
    for e in edits {
        let (s, t) = (e.start as usize, e.end as usize);
        assert!(t <= last, "edits sorted descending, never overlapping");
        out.replace_range(s..t, &e.insert);
        last = s;
    }
    out
}

fn minted(text: &str, ranges: Option<&[Span]>) -> Vec<TextEdit> {
    let mut n = 0;
    custom_id_edits(text, &opts(), ranges, &mut || {
        n += 1;
        uuid(n)
    })
    .unwrap()
}

fn corpus() -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(DIR).unwrap() {
        let path = e.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if !name.ends_with(".org") || name.ends_with(".expected.org") {
            continue;
        }
        let expected = path.with_extension("expected.org");
        let read = |p: &std::path::Path| String::from_utf8(std::fs::read(p).unwrap()).unwrap();
        out.push((name, read(&path), read(&expected)));
    }
    assert!(out.len() >= 4, "the corpus is there");
    out
}

/// Every corpus file, minted, is byte for byte what Emacs wrote; minting the
/// result again finds nothing to do.
#[test]
fn placement_matches_org_entry_put() {
    for (name, text, expected) in corpus() {
        let edits = minted(&text, None);
        assert_eq!(apply(&text, &edits), expected, "{name}");
        assert!(minted(&expected, None).is_empty(), "{name}: idempotent");
        let missing = headings_missing_custom_id(&text, &opts(), None);
        assert_eq!(missing.len(), edits.len(), "{name}: one edit per heading");
        for h in missing {
            let line = &text[h.start as usize..h.end as usize];
            assert!(
                line.starts_with('*') && !line.contains('\n'),
                "{name}: {line:?}"
            );
        }
    }
}

/// Scope: a headline counts when its headline line STARTS in a range, the
/// range's start included and its end excluded.
#[test]
fn ranges_select_headlines_by_where_their_line_starts() {
    let text = "* a\n* b\n* c\n";
    let at = |s: u32, e: u32| vec![Span { start: s, end: e }];
    let titles = |r: Option<&[Span]>| -> Vec<String> {
        headings_missing_custom_id(text, &opts(), r)
            .iter()
            .map(|s| text[s.start as usize..s.end as usize].to_string())
            .collect()
    };
    assert_eq!(titles(Some(&at(4, 8))), ["* b"]);
    assert_eq!(
        titles(Some(&at(5, 8))),
        Vec::<String>::new(),
        "b starts before 5"
    );
    assert_eq!(titles(Some(&at(0, 4))), ["* a"], "end excluded");
    assert_eq!(titles(Some(&at(0, 5))), ["* a", "* b"]);
    assert_eq!(titles(Some(&[])), Vec::<String>::new());
    assert_eq!(titles(None).len(), 3);
    let only_b = minted(text, Some(&at(4, 8)));
    assert_eq!(
        apply(text, &only_b),
        format!(
            "* a\n* b\n:PROPERTIES:\n:CUSTOM_ID: {}\n:END:\n* c\n",
            uuid(1)
        )
    );
}

/// Only a lowercase UUID v4 is ever written: anything else, a newline or
/// `:END:` included, fails the whole call with no edits.
#[test]
fn only_lowercase_uuid_v4_values_are_written() {
    let good = "1b4e28ba-2fa1-4d2b-a83f-0c6e1f3e9d6a";
    assert!(is_custom_id_uuid(good));
    for bad in [
        "",
        "1B4E28BA-2FA1-4D2B-A83F-0C6E1F3E9D6A",  // upper case
        "1b4e28ba-2fa1-1d2b-a83f-0c6e1f3e9d6a",  // version 1
        "1b4e28ba-2fa1-4d2b-c83f-0c6e1f3e9d6a",  // variant c
        "1b4e28ba2fa14d2ba83f0c6e1f3e9d6a",      // no hyphens
        "1b4e28ba-2fa1-4d2b-a83f-0c6e1f3e9d6a ", // trailing blank
        "1b4e28ba-2fa1-4d2b-a83f-0c6e1f3e9d6\n", // a newline
        "x\n:END:\n* injected",
        "1b4e28ba-2fa1-4d2b-a83f-0c6e1f3e9d6g", // not hex
    ] {
        assert!(!is_custom_id_uuid(bad), "{bad:?}");
        let r = custom_id_edits("* a\n* b\n", &opts(), None, &mut || bad.to_string());
        assert_eq!(r, Err(CustomIdError::NotAUuid(bad.to_string())), "{bad:?}");
        let r = replace_custom_id(
            "* a\n:PROPERTIES:\n:CUSTOM_ID: x\n:END:\n",
            &opts(),
            29,
            bad,
        );
        assert_eq!(r, Err(CustomIdError::NotAUuid(bad.to_string())), "{bad:?}");
    }
    // A bad value after good ones still yields nothing.
    let mut n = 0;
    let r = custom_id_edits("* a\n* b\n", &opts(), None, &mut || {
        n += 1;
        if n == 1 {
            good.to_string()
        } else {
            "bad".to_string()
        }
    });
    assert!(r.is_err());
}

/// Given values: too few is an error naming both counts; extras go unused.
#[test]
fn given_values_must_cover_every_headline() {
    let text = "* a\n* b\n* c\n";
    let vals: Vec<String> = (1..=3).map(uuid).collect();
    assert_eq!(
        custom_id_edits_with(text, &opts(), None, &vals[..2]),
        Err(CustomIdError::TooFewUuids { needed: 3, got: 2 })
    );
    let mut more = vals.clone();
    more.push(uuid(9));
    let edits = custom_id_edits_with(text, &opts(), None, &more).unwrap();
    let out = apply(text, &edits);
    for n in 1..=3 {
        assert!(out.contains(&uuid(n)), "{n}");
    }
    assert!(!out.contains(&uuid(9)));
    assert!(out.find(&uuid(1)) < out.find(&uuid(3)), "document order");
}

const DUPS: &str = "\
* one
:PROPERTIES:
:CUSTOM_ID: aaa
:END:
* two
:PROPERTIES:
:CUSTOM_ID: bbb
:END:
* three
:PROPERTIES:
:CUSTOM_ID: aaa
:END:
* four, last line wins
:PROPERTIES:
:CUSTOM_ID: ccc
:CUSTOM_ID: bbb
:END:
* five, empty
:PROPERTIES:
:CUSTOM_ID:
:END:
* six, empty
:PROPERTIES:
:CUSTOM_ID:
:END:
* seven
:PROPERTIES:
:CUSTOM_ID: aaa
:END:
#+begin_example
 * indented: block text, not a heading
:PROPERTIES:
:CUSTOM_ID: ccc
:END:
#+end_example
* eight
:PROPERTIES:
:CUSTOM_ID: ccc
:END:
";

/// Duplicates are reported as groups in first-occurrence order, each value
/// span holding the value Org reads (the last line) and each heading span the
/// headline line. Empty values, and drawer text in a block under no
/// headline of its own, are not CUSTOM_IDs.
#[test]
fn duplicates_are_grouped_and_never_resolved() {
    let groups = duplicate_custom_ids(DUPS, &opts());
    let show: Vec<Vec<(String, String)>> = groups
        .iter()
        .map(|g| {
            g.iter()
                .map(|o| {
                    let s = |sp: Span| DUPS[sp.start as usize..sp.end as usize].to_string();
                    (s(o.value), s(o.heading))
                })
                .collect()
        })
        .collect();
    let pair = |v: &str, h: &str| (v.to_string(), h.to_string());
    assert_eq!(
        show,
        vec![
            vec![
                pair("aaa", "* one"),
                pair("aaa", "* three"),
                pair("aaa", "* seven")
            ],
            vec![pair("bbb", "* two"), pair("bbb", "* four, last line wins")],
        ]
    );
}

/// Replacing one occurrence of a duplicate leaves the others; a position
/// that is not the start of a CUSTOM_ID value Org reads is refused.
#[test]
fn replace_targets_exactly_one_value() {
    let groups = duplicate_custom_ids(DUPS, &opts());
    let third = groups[0][1].value.start;
    let edit = replace_custom_id(DUPS, &opts(), third, &uuid(7)).unwrap();
    let out = apply(DUPS, &[edit]);
    assert_eq!(out.matches("aaa").count(), 2);
    assert!(out.contains(&format!("* three\n:PROPERTIES:\n:CUSTOM_ID: {}\n", uuid(7))));
    let left = duplicate_custom_ids(&out, &opts());
    assert_eq!(left[0].len(), 2, "one and seven still share aaa");

    let ccc_shadowed = DUPS.find(":CUSTOM_ID: ccc\n:CUSTOM_ID: bbb").unwrap() as u32 + 12;
    let in_block = DUPS.find("ccc\n:END:\n#+end").unwrap() as u32;
    assert!(in_block > 0);
    for at in [
        third + 1,
        third - 1,
        0,
        ccc_shadowed,
        in_block,
        DUPS.len() as u32 + 5,
    ] {
        assert_eq!(
            replace_custom_id(DUPS, &opts(), at, &uuid(7)),
            Err(CustomIdError::NotACustomIdValue(at)),
            "{at}"
        );
    }
}

/// Over every parser fixture: minting leaves no headline needing one, a
/// second pass finds nothing, and every edit only inserts (or fills an empty
/// value), so the original text is the minted text without the inserts.
/// Found a drawer placed after a planning line's blank lines, which Org does
/// not read as the property drawer.
#[test]
fn minting_every_fixture_leaves_nothing_missing() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    let mut files = 0;
    for e in std::fs::read_dir(dir).unwrap() {
        let path = e.unwrap().path();
        if path.extension().and_then(|x| x.to_str()) != Some("org") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        files += 1;
        let edits = minted(&text, None);
        let out = apply(&text, &edits);
        let name = path.display();
        assert!(
            headings_missing_custom_id(&out, &opts(), None).is_empty(),
            "{name}"
        );
        assert!(minted(&out, None).is_empty(), "{name}: idempotent");
        for e in &edits {
            let replaced = &text[e.start as usize..e.end as usize];
            assert!(
                replaced.trim().is_empty(),
                "{name}: only blanks replaced, not {replaced:?}"
            );
        }
    }
    assert!(files > 50, "the fixtures are there ({files})");
}
