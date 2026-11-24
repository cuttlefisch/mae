//! The attachment directory the semantics carry on an `attachment:` link
//! (ADR-115 Amendment 1, E2): `SemanticLink::attach_dir` and `attach_id`.
//!
//! - A property over generated outlines (random nesting, IDs present or
//!   absent at random levels, in random key case, a `DIR`/`ATTACH_DIR` at a
//!   random level, a file-level drawer and `#+PROPERTY: ID`), against an
//!   independent model of `org-attach-dir` written here from org-attach.el
//!   and org.el, not from the crate.
//! - The edge cases, and hostile names and IDs: carried verbatim, never
//!   joined (path safety is the host's).
//! - `emacs_agrees_on_generated_outlines` (ignored; needs emacs): the same
//!   generated outlines through Emacs's own `org-attach-dir`
//!   (`conformance/attach-dir.el`), run locally:
//!
//! ```sh
//! cargo test -p mae-org --test attach_dir -- --ignored --nocapture
//! ```

mod support;

use mae_org::{attachment_dir, parse, semantics, ParseOptions, SemanticLink};
use proptest::prelude::*;

fn attachment_links(text: &str) -> Vec<SemanticLink> {
    let doc = parse(text, &ParseOptions::default());
    semantics(&doc)
        .links
        .into_iter()
        .filter(|l| l.target.starts_with("attachment:"))
        .collect()
}

/// `(attach_id, attach_dir)` of each attachment link, in order.
fn dirs(text: &str) -> Vec<(Option<String>, Option<String>)> {
    attachment_links(text)
        .into_iter()
        .map(|l| (l.attach_id, l.attach_dir))
        .collect()
}

fn some(id: &str, dir: &str) -> (Option<String>, Option<String>) {
    (Some(id.to_string()), Some(dir.to_string()))
}

const NONE: (Option<String>, Option<String>) = (None, None);

// ------------------------------------------------------------------ the model

/// One entry of a generated outline: the zeroth section (level 0) or a
/// headline.
#[derive(Debug, Clone)]
struct Entry {
    level: usize,
    /// Property lines, key as written (any case), value.
    props: Vec<(String, String)>,
    /// Attachment links in the body, by name.
    links: Vec<String>,
}

#[derive(Debug, Clone)]
struct Outline {
    entries: Vec<Entry>,
    /// `#+PROPERTY: KEY VALUE` lines in the zeroth section.
    keywords: Vec<(String, String)>,
}

impl Outline {
    fn text(&self) -> String {
        let mut out = String::new();
        for (i, e) in self.entries.iter().enumerate() {
            if e.level > 0 {
                out.push_str(&format!("{} H{i}\n", "*".repeat(e.level)));
            }
            if !e.props.is_empty() {
                out.push_str(":PROPERTIES:\n");
                for (k, v) in &e.props {
                    out.push_str(&format!(":{k}: {v}\n"));
                }
                out.push_str(":END:\n");
            }
            if e.level == 0 {
                for (k, v) in &self.keywords {
                    out.push_str(&format!("#+PROPERTY: {k} {v}\n"));
                }
            }
            for l in &e.links {
                out.push_str(&format!("[[attachment:{l}]]\n"));
            }
        }
        out
    }

    /// The model: `org-attach-dir` with inheritance at a link of entry `i`.
    fn expected(&self, i: usize) -> (Option<String>, Option<String>) {
        // The outline path: the entry, then each nearest earlier entry of a
        // lower level, ending with the zeroth section (level 0).
        let mut path = vec![i];
        let mut level = self.entries[i].level;
        for j in (0..i).rev() {
            if level > 0 && self.entries[j].level < level {
                path.push(j);
                level = self.entries[j].level;
            }
        }
        if *path.last().unwrap() != 0 {
            path.push(0);
        }
        let get = |key: &str| -> Option<String> {
            for &j in &path {
                let v = self.entries[j]
                    .props
                    .iter()
                    .rev()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .map(|(_, v)| v.trim().to_string());
                if let Some(v) = v {
                    return (v != "nil").then_some(v);
                }
            }
            let v = self
                .keywords
                .iter()
                .rev()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v.trim().to_string());
            v.filter(|v| v != "nil")
        };
        if get("DIR").is_some() || get("ATTACH_DIR").is_some() {
            return NONE;
        }
        let Some(id) = get("ID").filter(|id| !id.is_empty()) else {
            return NONE;
        };
        let chars: Vec<char> = id.chars().collect();
        let dir = if chars.len() > 2 {
            let head: String = chars[..2].iter().collect();
            let tail: String = chars[2..].iter().collect();
            format!("data/{head}/{tail}/")
        } else {
            format!("data/__/{}/{id}/", chars[0])
        };
        (Some(id), Some(dir))
    }

    fn all_expected(&self) -> Vec<(Option<String>, Option<String>)> {
        let mut v = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            for _ in &e.links {
                v.push(self.expected(i));
            }
        }
        v
    }
}

/// IDs: 1, 2 and 3 characters, multibyte, a UUID, a timestamp, `nil`, one
/// with a space inside.
const IDS: &[&str] = &[
    "a",
    "ab",
    "abc",
    "é",
    "éa",
    "éab",
    "8f14e45f-ceea-467f-a0e6-4d1a2b3c4d5e",
    "20261124T101010.123456",
    "nil",
    "x y",
];
const DIRS: &[&str] = &["att", "nil", "/abs/dir"];
const KEY_CASES: &[fn(&str) -> String] = &[
    |k| k.to_string(),
    |k| k.to_ascii_lowercase(),
    |k| {
        let mut c = k.chars();
        c.next().map(|f| f.to_string()).unwrap_or_default() + &c.as_str().to_ascii_lowercase()
    },
];

/// A small deterministic generator (xorshift), so the Emacs run sees the
/// same outlines as the property: seed → outline.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.next() % 100 < pct
    }
}

fn outline(seed: u64) -> Outline {
    let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut entries = Vec::new();
    let mut n_links = 0;
    fn prop(r: &mut Rng, key: &str, values: &[&str]) -> (String, String) {
        let value = values[r.below(values.len())].to_string();
        (KEY_CASES[r.below(KEY_CASES.len())](key), value)
    }
    let count = 1 + r.below(8);
    for i in 0..count {
        let level = if i == 0 { 0 } else { 1 + r.below(4) };
        let mut props = Vec::new();
        if r.chance(45) {
            props.push(prop(&mut r, "ID", IDS));
        }
        if r.chance(10) {
            let key = if r.chance(50) { "DIR" } else { "ATTACH_DIR" };
            props.push(prop(&mut r, key, DIRS));
        }
        if r.chance(8) {
            // A second ID line: the last one wins.
            props.push(prop(&mut r, "ID", IDS));
        }
        let mut links = Vec::new();
        for _ in 0..r.below(3) {
            links.push(format!("l{n_links}.png"));
            n_links += 1;
        }
        entries.push(Entry {
            level,
            props,
            links,
        });
    }
    let mut keywords = Vec::new();
    if r.chance(15) {
        keywords.push(("ID".to_string(), IDS[r.below(IDS.len())].to_string()));
    }
    if r.chance(5) {
        keywords.push(("DIR".to_string(), "kwdir".to_string()));
    }
    Outline { entries, keywords }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn attach_dir_follows_org_attach_over_generated_outlines(seed in any::<u64>()) {
        let o = outline(seed);
        let text = o.text();
        prop_assert_eq!(dirs(&text), o.all_expected(), "{}", text);
    }
}

// ------------------------------------------------------------- edge cases

#[test]
fn the_layout_is_uuid_then_fallback_by_characters() {
    assert_eq!(attachment_dir("a").as_deref(), Some("data/__/a/a/"));
    assert_eq!(attachment_dir("ab").as_deref(), Some("data/__/a/ab/"));
    assert_eq!(attachment_dir("abc").as_deref(), Some("data/ab/c/"));
    assert_eq!(
        attachment_dir("8f14e45f-ceea-467f-a0e6-4d1a2b3c4d5e").as_deref(),
        Some("data/8f/14e45f-ceea-467f-a0e6-4d1a2b3c4d5e/")
    );
    // Characters, not bytes, as Emacs's `length' and `substring' count.
    assert_eq!(attachment_dir("éa").as_deref(), Some("data/__/é/éa/"));
    assert_eq!(attachment_dir("éaß").as_deref(), Some("data/éa/ß/"));
    assert_eq!(attachment_dir(""), None);
}

#[test]
fn a_link_takes_the_nearest_id_up_the_outline() {
    let text = "\
:PROPERTIES:
:ID: filelevel
:END:
[[attachment:before.png]]
* With
:PROPERTIES:
:ID: abc
:END:
[[attachment:own.png]]
** Without an id
[[attachment:inherited.png]]
* Other, no id
[[attachment:file.png]]
* Short
:PROPERTIES:
:ID: xy
:END:
[[attachment:short.png]]
";
    assert_eq!(
        dirs(text),
        vec![
            some("filelevel", "data/fi/lelevel/"),
            some("abc", "data/ab/c/"),
            some("abc", "data/ab/c/"),
            some("filelevel", "data/fi/lelevel/"),
            some("xy", "data/__/x/xy/"),
        ]
    );
}

#[test]
fn no_id_anywhere_gives_no_directory() {
    assert_eq!(
        dirs("[[attachment:x.png]]\n* H\n[[attachment:y.png]]\n"),
        vec![NONE, NONE]
    );
}

#[test]
fn a_dir_anywhere_up_the_outline_gives_no_directory() {
    // Org reads DIR before ID, each inherited: a DIR above the ID's own
    // headline still wins, and so does `#+PROPERTY: DIR`.
    let text = "\
* Top
:PROPERTIES:
:DIR: elsewhere
:END:
** Mid
:PROPERTIES:
:ID: abcdef
:END:
*** Leaf
[[attachment:x.png]]
* Second
:PROPERTIES:
:ID: abcdef
:ATTACH_DIR: old
:END:
[[attachment:y.png]]
* Third
:PROPERTIES:
:ID: abcdef
:DIR: nil
:END:
[[attachment:z.png]]
";
    assert_eq!(
        dirs(text),
        vec![NONE, NONE, some("abcdef", "data/ab/cdef/")]
    );
    let kw =
        "#+PROPERTY: DIR shared\n* H\n:PROPERTIES:\n:ID: abcdef\n:END:\n[[attachment:x.png]]\n";
    assert_eq!(dirs(kw), vec![NONE]);
}

#[test]
fn nil_and_empty_ids_stop_the_search_and_give_nothing() {
    // `nil' is a value found (Org stops there) meaning none; an empty ID
    // makes Emacs signal an error, and MAE gives no directory
    // (registered: attachment-dir-empty-id).
    let text = "\
:PROPERTIES:
:ID: filelevel
:END:
* Nil
:PROPERTIES:
:ID: nil
:END:
[[attachment:a.png]]
* Empty
:PROPERTIES:
:ID:
:END:
[[attachment:b.png]]
";
    assert_eq!(dirs(text), vec![NONE, NONE]);
}

#[test]
fn property_keywords_and_appends_count_as_org_counts_them() {
    let kw = "#+PROPERTY: ID fromkeyword\n[[attachment:a.png]]\n* H\n[[attachment:b.png]]\n";
    assert_eq!(
        dirs(kw),
        vec![
            some("fromkeyword", "data/fr/omkeyword/"),
            some("fromkeyword", "data/fr/omkeyword/"),
        ]
    );
    // `ID+` appends with a space, as `org-entry-get' joins it.
    let plus = "* H\n:PROPERTIES:\n:ID: ab\n:ID+: cd\n:END:\n[[attachment:a.png]]\n";
    assert_eq!(dirs(plus), vec![some("ab cd", "data/ab/ cd/")]);
}

#[test]
fn a_link_in_a_title_or_a_footnote_takes_its_own_place_in_the_outline() {
    // Org expands attachment links in the buffer before parsing
    // (`org-attach-expand-links'), so a footnote definition's link takes the
    // headline the definition is under, not where it is referenced.
    let text = "\
* A [[attachment:title.png]]
:PROPERTIES:
:ID: aaaa
:END:
See[fn:1].
* B
:PROPERTIES:
:ID: bbbb
:END:
[fn:1] [[attachment:note.png]]
";
    assert_eq!(
        dirs(text),
        vec![some("aaaa", "data/aa/aa/"), some("bbbb", "data/bb/bb/")]
    );
}

#[test]
fn only_attachment_links_written_so_carry_a_directory() {
    // Org's link type is case-sensitive: `ATTACHMENT:` is not org-attach's.
    let text = "* H\n:PROPERTIES:\n:ID: abcd\n:END:\n[[ATTACHMENT:a.png]] [[file:b.png]] [[attachment:c.png][described]] https://x.org/d.png\n";
    let doc = parse(text, &ParseOptions::default());
    let got: Vec<(String, Option<String>)> = semantics(&doc)
        .links
        .into_iter()
        .map(|l| (l.target, l.attach_dir))
        .collect();
    assert_eq!(
        got,
        vec![
            ("ATTACHMENT:a.png".to_string(), None),
            ("file:b.png".to_string(), None),
            // A described attachment link is a link to the file: it still
            // lives in the directory.
            (
                "attachment:c.png".to_string(),
                Some("data/ab/cd/".to_string())
            ),
            ("https://x.org/d.png".to_string(), None),
        ]
    );
}

#[test]
fn hostile_names_and_ids_are_carried_verbatim_and_never_joined() {
    // mae-org reports the target and the directory; it builds no path from
    // the NAME, and validates neither: path safety is the host's.
    let names = [
        "../../etc/passwd.png",
        "/abs/x.png",
        "%2e%2e/%2e%2e/x.png",
        "a/../../b.png",
    ];
    for name in names {
        let text = format!("* H\n:PROPERTIES:\n:ID: abcdef\n:END:\n[[attachment:{name}]]\n");
        let links = attachment_links(&text);
        assert_eq!(links.len(), 1, "{text}");
        let l = &links[0];
        assert_eq!(l.target, format!("attachment:{name}"));
        assert_eq!(l.attach_dir.as_deref(), Some("data/ab/cdef/"));
        let dir = l.attach_dir.as_deref().unwrap();
        assert!(
            !dir.contains(name) && !dir.contains("..") && !dir.contains('%'),
            "{dir}"
        );
    }
    // An ID is author text too: reported as written, `..' and all.
    let text = "* H\n:PROPERTIES:\n:ID: ../../x\n:END:\n[[attachment:a.png]]\n";
    // (Emacs's `format "%s/%s"' of `..' and `/../x'.)
    assert_eq!(dirs(text), vec![some("../../x", "data/..//../x/")]);
}

#[test]
fn the_json_carries_both_fields() {
    let doc = parse(
        "* H\n:PROPERTIES:\n:ID: abcdef\n:END:\n[[attachment:x.png]] [[https://a.org]]\n",
        &ParseOptions::default(),
    );
    let json: serde_json::Value = serde_json::from_str(&semantics(&doc).to_json()).unwrap();
    let links = json["links"].as_array().unwrap();
    assert_eq!(links[0]["target"], "attachment:x.png");
    assert_eq!(links[0]["attach_dir"], "data/ab/cdef/");
    assert_eq!(links[0]["attach_id"], "abcdef");
    assert_eq!(links[1]["attach_dir"], serde_json::Value::Null);
    assert_eq!(links[1]["attach_id"], serde_json::Value::Null);
}

// -------------------------------------------------------- the Emacs oracle

/// How many generated outlines the Emacs run checks.
const ORACLE_OUTLINES: u64 = 400;

#[test]
#[ignore = "needs emacs; run locally"]
fn emacs_agrees_on_generated_outlines() {
    use std::path::Path;
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join("attach-dir-oracle");
    let input = base.join("in");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&input).unwrap();
    let mut outlines = Vec::new();
    for seed in 0..ORACLE_OUTLINES {
        let o = outline(seed);
        let name = format!("o{seed:05}.org");
        std::fs::write(input.join(&name), o.text()).unwrap();
        outlines.push((name, o));
    }
    // The edge cases' documents too, checked against Emacs the same way.
    let out = base.join("emacs.tsv");
    let el = Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance/attach-dir.el");
    let args: Vec<std::ffi::OsString> = vec![
        "-Q".into(),
        "--batch".into(),
        "-l".into(),
        el.into(),
        input.clone().into(),
        out.clone().into(),
    ];
    let exit = support::run_emacs_bounded(
        &args,
        2 * 1024 * 1024,
        std::time::Duration::from_secs(300),
        std::process::Stdio::null(),
    );
    assert_eq!(exit, support::EmacsExit::Ok, "emacs failed");
    let tsv = std::fs::read_to_string(&out).unwrap();
    let mut emacs: std::collections::BTreeMap<String, Vec<Vec<String>>> = Default::default();
    for line in tsv.lines() {
        let f: Vec<String> = line.split('\t').map(str::to_string).collect();
        emacs.entry(f[0].clone()).or_default().push(f);
    }
    let (mut links, mut with_dir, mut dir_prop) = (0, 0, 0);
    for (name, o) in &outlines {
        let text = o.text();
        let mae = attachment_links(&text);
        let rows = emacs.get(name).cloned().unwrap_or_default();
        assert_eq!(mae.len(), rows.len(), "{name}: link count\n{text}");
        for (l, row) in mae.iter().zip(&rows) {
            links += 1;
            let (dir_set, id, result) = (&row[2], &row[3], &row[4]);
            if dir_set == "t" {
                dir_prop += 1;
                assert_eq!(l.attach_dir, None, "{name}: DIR set\n{text}");
                continue;
            }
            match result.as_str() {
                "nil" | "ERROR" => assert_eq!(l.attach_dir, None, "{name}: {row:?}\n{text}"),
                dir => {
                    with_dir += 1;
                    assert_eq!(
                        l.attach_dir.as_deref(),
                        Some(dir),
                        "{name}: {row:?}\n{text}"
                    );
                    assert_eq!(l.attach_id.as_deref(), Some(id.as_str()), "{name}\n{text}");
                }
            }
        }
    }
    eprintln!(
        "attach-dir oracle: {} outlines, {links} links ({with_dir} with a directory, {dir_prop} under DIR), Emacs agrees",
        outlines.len()
    );
    assert!(
        with_dir > 100 && dir_prop > 10,
        "the corpus must exercise both sides"
    );
}
