//! `mae_org::headline_properties`: every headline's property drawer, node or
//! not. Which drawer Org reads as a headline's is the parser's, already held
//! to Emacs by the conformance gate and the `CUSTOM_ID` corpus; these pin
//! the projection: which headlines are listed, and every field of each.

use mae_org::{headline_properties, HeadlineProperties, ParseOptions, Span};

fn props(text: &str, ranges: Option<&[Span]>) -> Vec<HeadlineProperties> {
    headline_properties(text, &ParseOptions::default(), ranges)
}

/// `(headline line, level, [(key, key as written, value)])`, as text.
type Shown = (String, usize, Vec<(String, String, String)>);

fn show(text: &str, ranges: Option<&[Span]>) -> Vec<Shown> {
    props(text, ranges)
        .into_iter()
        .map(|h| {
            let line = text[h.heading.start as usize..h.heading.end as usize].to_string();
            let ps = h
                .properties
                .into_iter()
                .map(|p| (p.key, p.key_as_written, p.value))
                .collect();
            (line, h.level, ps)
        })
        .collect()
}

fn p(key: &str, written: &str, value: &str) -> (String, String, String) {
    (key.into(), written.into(), value.into())
}

const DOC: &str = "\
:PROPERTIES:
:ID: file-level
:END:
#+title: t
* Folded, anchor only
:PROPERTIES:
:CUSTOM_ID: 1b4e28ba-2fa1-4d2b-a83f-0c6e1f3e9d6a
:VISIBILITY: folded
:END:
Text.
* No drawer
** A node :tag:
:PROPERTIES:
:ID: n1
:END:
* After planning
SCHEDULED: <2026-11-13 Fri>
:PROPERTIES:
:Visibility:   children   \t
:END:
* Planning, a blank line, then a drawer Org does not read
SCHEDULED: <2026-11-13 Fri>

:PROPERTIES:
:VISIBILITY: folded
:END:
* A blank line, then a drawer Org does not read

:PROPERTIES:
:VISIBILITY: folded
:END:
* Indented, repeated and appending keys
  :PROPERTIES:
  :multi: a
  :MULTI+: b
  :multi: c
  :END:
* Empty drawer
:PROPERTIES:
:END:
* Lower case drawer
:properties:
:other: v
:end:
* A drawer further down the section
Text first.
:PROPERTIES:
:VISIBILITY: folded
:END:
#+begin_example
* A heading, even in a block
:PROPERTIES:
:K: v
:END:
#+end_example
";

#[test]
fn every_headline_with_a_drawer_org_reads_and_only_those() {
    assert_eq!(
        show(DOC, None),
        vec![
            (
                "* Folded, anchor only".into(),
                1,
                vec![
                    p(
                        "CUSTOM_ID",
                        "CUSTOM_ID",
                        "1b4e28ba-2fa1-4d2b-a83f-0c6e1f3e9d6a"
                    ),
                    p("VISIBILITY", "VISIBILITY", "folded"),
                ]
            ),
            ("** A node :tag:".into(), 2, vec![p("ID", "ID", "n1")]),
            (
                "* After planning".into(),
                1,
                vec![p("VISIBILITY", "Visibility", "children")]
            ),
            (
                "* Indented, repeated and appending keys".into(),
                1,
                vec![
                    p("MULTI", "multi", "a"),
                    p("MULTI+", "MULTI+", "b"),
                    p("MULTI", "multi", "c"),
                ]
            ),
            ("* Empty drawer".into(), 1, vec![]),
            (
                "* Lower case drawer".into(),
                1,
                vec![p("OTHER", "other", "v")]
            ),
            (
                "* A heading, even in a block".into(),
                1,
                vec![p("K", "K", "v")]
            ),
        ]
    );
}

/// Scope: a headline counts when its headline line STARTS in a range, the
/// start included and the end excluded.
#[test]
fn ranges_select_headlines_by_where_their_line_starts() {
    let at = |needle: &str| DOC.find(needle).unwrap() as u32;
    let folded = at("* Folded");
    let node = at("** A node");
    let titles =
        |r: &[Span]| -> Vec<String> { show(DOC, Some(r)).into_iter().map(|s| s.0).collect() };
    assert_eq!(
        titles(&[Span {
            start: folded,
            end: folded + 1
        }]),
        ["* Folded, anchor only"]
    );
    assert_eq!(
        titles(&[Span {
            start: folded + 1,
            end: node
        }]),
        Vec::<String>::new(),
        "starts before the range; the end excluded"
    );
    assert_eq!(
        titles(&[Span {
            start: folded,
            end: node + 1
        }]),
        ["* Folded, anchor only", "** A node :tag:"]
    );
    assert_eq!(titles(&[]), Vec::<String>::new());
}

/// Over every parser fixture: each listed headline's last value for a key is
/// what the drawer view reads for it, the heading span is one line starting
/// with stars, levels match the stars, and a headline is listed exactly when
/// it has a drawer: the unscoped count equals the count of `:PROPERTIES:`
/// drawers Org reads under a headline.
#[test]
fn over_every_fixture_the_listing_agrees_with_the_document() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    let mut files = 0;
    let mut listed = 0;
    for e in std::fs::read_dir(dir).unwrap() {
        let path = e.unwrap().path();
        if path.extension().and_then(|x| x.to_str()) != Some("org") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        files += 1;
        let name = path.display();
        let doc = mae_org::parse(&text, &ParseOptions::default());
        let with_drawer: Vec<_> = doc
            .headlines()
            .filter_map(|h| h.property_drawer().map(|d| (h, d)))
            .collect();
        let got = props(&text, None);
        assert_eq!(got.len(), with_drawer.len(), "{name}");
        for (g, (h, d)) in got.iter().zip(&with_drawer) {
            let line = &text[g.heading.start as usize..g.heading.end as usize];
            assert!(
                line.starts_with('*') && !line.contains('\n'),
                "{name}: {line:?}"
            );
            assert_eq!(
                g.level,
                line.chars().take_while(|c| *c == '*').count(),
                "{name}"
            );
            assert_eq!(g.level, h.level(), "{name}");
            for q in &g.properties {
                let last = g.properties.iter().rev().find(|r| r.key == q.key).unwrap();
                assert_eq!(d.get(&q.key), Some(last.value.clone()), "{name}: {}", q.key);
                assert!(q.key.eq_ignore_ascii_case(&q.key_as_written), "{name}");
            }
        }
        listed += got.len();
    }
    assert!(files > 50, "the fixtures are there ({files})");
    assert!(
        listed > 10,
        "some fixtures have headline drawers ({listed})"
    );
}
