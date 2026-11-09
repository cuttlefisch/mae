//! Test 1 and test 2 as properties over generated pages: Split then Regen is
//! exact on canonical files, and N rewrites only the header of the rest.
//!
//! Bodies are built from fragments that exercise what a literal store must
//! not touch: id links with descriptions equal to targets, ordered steps that
//! repeat a target, include lines (alone, indented, last without a newline),
//! typed links, links inside verbatim/code/src blocks, headline and list-item
//! anchors, leading keywords and comments, multibyte text and UTF-16 surrogate
//! pairs, trailing blanks.

use proptest::prelude::*;

use crate::seed::*;

const TARGETS: &[&str] = &[
    "ens:paso-1",
    "iso:9001",
    "5b7c9d1e-2f3a-4b5c-8d9e-0a1b2c3d4e5f",
    "src:x.y",
];

fn id() -> impl Strategy<Value = String> {
    prop_oneof![
        "[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}",
        "(ens|iso|src|ul|task|decision):[a-z0-9][a-z0-9._-]{0,16}",
    ]
}

fn value() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        "[A-Za-z0-9áéíóúñÑ ._/-]{1,30}".prop_map(|s| s.trim().to_string()),
        prop::sample::select(TARGETS).prop_map(|t| format!("[[id:{t}][{t}]]")),
    ]
}

/// Drawer properties: `ID` first or not, other keys unique case-insensitively.
fn properties() -> impl Strategy<Value = (String, Vec<(String, String)>)> {
    (
        id(),
        prop::collection::vec(("[A-Za-z][A-Za-z0-9_-]{0,24}", value()), 0..5),
        any::<prop::sample::Index>(),
    )
        .prop_map(|(id, extra, at)| {
            let mut seen = std::collections::HashSet::from(["ID".to_string()]);
            let mut props: Vec<(String, String)> = extra
                .into_iter()
                .filter(|(k, _)| seen.insert(k.to_ascii_uppercase()))
                .collect();
            props.insert(at.index(props.len() + 1), ("ID".to_string(), id.clone()));
            (id, props)
        })
}

fn fragment(kind: u8, t: &str, i: usize) -> String {
    match kind % 16 {
        0 => "Texto con ñ, 😀 y espacios finales   ".to_string(),
        1 => format!("{i}. [[id:{t}][Paso {i}]]"),
        2 => format!("- Parámetro :: [[id:{t}][{t}]]"),
        3 => format!("[[id:{t}]]"),
        4 => format!("  [[id:{t}]]"),
        5 => format!("Tipado [[id:{t}?rel=answers&w=0.5&by=x][r]]"),
        6 => format!("=[[id:{t}]]= y ~[[id:{t}]]~"),
        7 => format!("#+begin_src sh\necho \"[[id:{t}]]\"\n#+end_src"),
        8 => format!("* Sección {i}\n:PROPERTIES:\n:ID: ancla-{i}\n:END:"),
        9 => format!("- ítem {i}\n  :PROPERTIES:\n  :ID: item-{i}\n  :END:"),
        10 => "#+caption: c\n| a | b |".to_string(),
        11 => "#+author: alguien".to_string(),
        12 => "# comentario".to_string(),
        13 => String::new(),
        14 => "#+language: es".to_string(),
        _ => "[[https://example.com/x][web]]".to_string(),
    }
}

/// A body and the anchors it holds.
fn body() -> impl Strategy<Value = (String, Vec<String>)> {
    (
        prop::collection::vec((any::<u8>(), 0..TARGETS.len()), 0..14),
        any::<bool>(),
    )
        .prop_map(|(frags, final_nl)| {
            let mut anchors = Vec::new();
            let parts: Vec<String> = frags
                .iter()
                .enumerate()
                .map(|(i, (k, t))| {
                    match k % 16 {
                        8 => anchors.push(format!("ancla-{i}")),
                        9 => anchors.push(format!("item-{i}")),
                        _ => {}
                    }
                    fragment(*k, TARGETS[*t], i)
                })
                .collect();
            let mut b = parts.join("\n");
            if final_nl && !b.is_empty() {
                b.push('\n');
            }
            anchors.sort();
            (b, anchors)
        })
}

fn page_fields() -> impl Strategy<Value = SplitFile> {
    let title =
        prop::option::of("[A-Za-zÁÉáéñ0-9 ,.¿?()-]{0,30}".prop_map(|s| s.trim().to_string()));
    let tags = prop::option::of(prop::collection::vec("[A-Za-z0-9_@#%]{1,8}", 0..4));
    (properties(), title, tags, body()).prop_map(
        |((id, properties), title, filetags, (body, anchors))| SplitFile {
            id,
            properties,
            title,
            filetags,
            body,
            anchors,
        },
    )
}

fn sorted(mut f: SplitFile) -> SplitFile {
    f.anchors.sort();
    f
}

/// How a non-canonical header may be written: what N must undo.
#[derive(Debug, Clone)]
struct Style {
    sep: &'static str,
    trailing: &'static str,
    title_key: &'static str,
    title_gap: &'static str,
    tags_first: bool,
}

fn style() -> impl Strategy<Value = Style> {
    (
        prop::sample::select(vec![" ", "  ", "\t", "       "]),
        prop::sample::select(vec!["", " ", "\t "]),
        prop::sample::select(vec!["title", "TITLE", "Title"]),
        prop::sample::select(vec![" ", "   "]),
        any::<bool>(),
    )
        .prop_map(|(sep, trailing, title_key, title_gap, tags_first)| Style {
            sep,
            trailing,
            title_key,
            title_gap,
            tags_first,
        })
}

fn styled(f: &SplitFile, s: &Style) -> String {
    let mut out = String::from(":PROPERTIES:\n");
    for (k, v) in &f.properties {
        out.push_str(&format!(
            ":{k}:{}{v}{}\n",
            if v.is_empty() { "" } else { s.sep },
            s.trailing
        ));
    }
    out.push_str(":END:\n");
    let title = f
        .title
        .as_ref()
        .map(|t| format!("#+{}:{}{t}\n", s.title_key, s.title_gap));
    let tags = f.filetags.as_ref().map(|t| {
        let v = if t.is_empty() {
            String::new()
        } else {
            format!(" :{}:", t.join(":"))
        };
        format!("#+filetags:{v}\n")
    });
    let (first, second) = if s.tags_first {
        (tags, title)
    } else {
        (title, tags)
    };
    out.push_str(&first.unwrap_or_default());
    out.push_str(&second.unwrap_or_default());
    out.push_str(&f.body);
    out
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 192, ..ProptestConfig::default() })]

    /// Test 1: F = regen(fields) splits back to exactly those fields and body
    /// (G1), so N(F) == F (G3). Kills: any rewriting of the body, a lost or
    /// re-cased key, a lost empty value, reordered properties or tags, an
    /// anchor taken for a node.
    #[test]
    fn canonical_pages_split_and_regenerate_exactly(fields in page_fields()) {
        let f = regen(&fields);
        prop_assert_eq!(drawer_id(&f), Some(fields.id.clone()), "the history reader agrees with split");
        let split_back = split(&f).map(sorted);
        prop_assert_eq!(split_back, Ok(fields));
        prop_assert_eq!(normalise(&f), Ok(f.clone()));
    }

    /// Test 2: a header written another way normalises to the canonical file,
    /// N is idempotent, and the body is untouched. Kills: a normalisation that
    /// reaches into the body, or one that is not a fixed point.
    #[test]
    fn other_header_layouts_normalise_to_the_canonical_page(fields in page_fields(), s in style()) {
        let canonical = regen(&fields);
        let p = styled(&fields, &s);
        prop_assert!(p.ends_with(&fields.body));
        prop_assert_eq!(drawer_id(&p), Some(fields.id.clone()), "the history reader agrees with split");
        let n = normalise(&p).expect("a styled header still splits");
        prop_assert_eq!(&n, &canonical);
        prop_assert_eq!(normalise(&n), Ok(n.clone()));
        prop_assert_eq!(split(&p).map(sorted), Ok(fields));
    }
}
