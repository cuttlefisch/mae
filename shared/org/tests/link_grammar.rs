//! The one link grammar: `classify_link` (ADR-112 D2, ADR-030).
//!
//! The old split orders are reproduced here as small functions copied from
//! their sources, so the inputs on which they disagreed are pinned against
//! the order mae-org chose. They go when their callers move to mae-org (O4).

use mae_org::{classify_link, parse, semantics, LinkClass, LinkKind, ParseOptions, DEFAULT_REL};
use proptest::prelude::*;

/// mae-kb `parse_typed_links` + `split_link_target`: `][` then `|` on the
/// text up to the FIRST `]]`, strip `id:`, then `?`, then `#`.
fn old_kb(inner: &str) -> (String, Option<String>) {
    let (t, _) = match inner.find("][") {
        Some(i) => (&inner[..i], Some(&inner[i + 2..])),
        None => match inner.find('|') {
            Some(i) => (&inner[..i], Some(&inner[i + 1..])),
            None => (inner, None),
        },
    };
    let t = t.strip_prefix("id:").unwrap_or(t).trim();
    let path = t.split_once('?').map_or(t, |p| p.0);
    match path.split_once('#') {
        Some((n, f)) => (n.to_string(), Some(f.to_string())),
        None => (path.to_string(), None),
    }
}

/// The exporter's `parse_org_link_str`: `][` searched in the REST of the
/// text (past the link's own `]]`), then `|`; `id:` stripped; no query split.
fn old_export(rest_after_open: &str) -> String {
    if let Some(p) = rest_after_open.find("][") {
        if rest_after_open[p + 2..].contains("]]") {
            let t = &rest_after_open[..p];
            return t.strip_prefix("id:").unwrap_or(t).to_string();
        }
    }
    let inner = &rest_after_open[..rest_after_open.find("]]").unwrap_or(0)];
    let t = inner.split_once('|').map_or(inner, |p| p.0);
    t.strip_prefix("id:").unwrap_or(t).to_string()
}

/// The help renderer's `render_kb_body`: `|` BEFORE `][`, `?` stripped,
/// `id:` kept.
fn old_help(inner: &str) -> String {
    let t = match inner.find('|') {
        Some(i) => inner[..i].trim(),
        None => inner.find("][").map_or(inner, |i| &inner[..i]).trim(),
    };
    t.split_once('?').map_or(t, |p| p.0).trim_end().to_string()
}

/// mae-org: the tree finds the path, `classify_link` reads it.
fn chosen(text: &str) -> Vec<(String, Option<String>)> {
    let doc = parse(text, &ParseOptions::default());
    semantics(&doc)
        .links
        .into_iter()
        .map(|l| (l.target, l.fragment))
        .collect()
}

#[test]
fn the_three_old_split_orders_and_the_chosen_one() {
    // 1. Store form inside an org description: mae-kb and the exporter kept
    //    `a|b` as the target, the help renderer split at `|` first.
    let t = "[[a|b][c]]";
    assert_eq!(old_kb(&t[2..t.len() - 2]).0, "a|b");
    assert_eq!(old_export(&t[2..]), "a|b");
    assert_eq!(old_help(&t[2..t.len() - 2]), "a");
    // Chosen: the tree splits `][` (path `a|b`), the grammar splits `|`.
    assert_eq!(chosen(t), vec![("a".to_string(), None)]);

    // 2. Two links on a line: the exporter's `][` search ran past the
    //    first link's `]]` into the second link.
    let t = "[[a]] and [[b][c]]";
    assert_eq!(old_export(&t[2..]), "a]] and [[b");
    assert_eq!(old_kb("a").0, "a");
    assert_eq!(chosen(t), vec![("a".into(), None), ("b".into(), None)]);

    // 3. The `id:` prefix: kept by the help renderer.
    assert_eq!(old_help("id:x"), "id:x");
    assert_eq!(old_kb("id:x").0, "x");
    assert_eq!(chosen("[[id:x]]"), vec![("x".into(), None)]);

    // 4. A query after the fragment would be the query's; a fragment after
    //    the query is part of the query (ADR-030 order, as mae-kb had it).
    assert_eq!(old_kb("x#f?rel=r"), ("x".into(), Some("f".into())));
    assert_eq!(old_kb("x?rel=r#f"), ("x".into(), None));
    let c = classify_link("x?rel=r#f");
    assert_eq!(
        (c.target.as_str(), c.fragment, c.rel.as_str()),
        ("x", None, "r#f")
    );

    // 5. The exporter never split the query: `#concept:x?rel=y` hrefs.
    assert_eq!(old_export("concept:x?rel=y]]"), "concept:x?rel=y");
    assert_eq!(
        chosen("[[concept:x?rel=y]]"),
        vec![("concept:x".into(), None)]
    );

    // 6. The store form is read before `?` and `#`, so its description may
    //    contain both.
    let c = classify_link("x|What? C# notes");
    assert_eq!((c.target.as_str(), c.fragment), ("x", None));
    assert_eq!(c.description.as_deref(), Some("What? C# notes"));
    assert_eq!(c.rel, DEFAULT_REL);
}

#[test]
fn external_classifier_is_an_exclude_list() {
    let kind = |p: &str| classify_link(p).kind;
    // Web and mail, any case; the whole URL is the target.
    assert_eq!(kind("https://a.invalid/x?y=1#z"), LinkKind::Web);
    assert_eq!(kind("HTTP://A.INVALID"), LinkKind::Web);
    assert_eq!(kind("mailto:a@b.invalid"), LinkKind::Mailto);
    let web = classify_link("https://a.invalid/x?rel=y#z");
    assert_eq!(web.target, "https://a.invalid/x?rel=y#z");
    assert_eq!((web.fragment, web.rel.as_str()), (None, DEFAULT_REL));
    // Known non-KB schemes, including the three mae-kb's list lacked.
    for p in [
        "file:a.org",
        "doi:10.1/2",
        "help:org",
        "file+sys:/x",
        "file+emacs:/x",
        "FTP://h",
    ] {
        assert_eq!(kind(p), LinkKind::Other, "{p}");
    }
    // Everything else is a KB link: a bare UUID, a namespaced id, a fuzzy
    // name, and an unknown scheme (a missed scheme costs a broken-link
    // report; a missed id would cost a real edge).
    for p in [
        "0f1e2d3c-0000-4000-8000-00000000abcd",
        "concept:buffer",
        "Some Heading",
        "ssh://host.invalid",
        "id:abc",
        "ID:abc",
    ] {
        assert_eq!(kind(p), LinkKind::Kb, "{p}");
    }
    assert_eq!(classify_link("ID:abc").target, "abc");
    // A scheme is a PREFIX: one inside a store-form description is not.
    let c = classify_link("uuid-1|see https://a.invalid");
    assert_eq!((c.kind, c.target.as_str()), (LinkKind::Kb, "uuid-1"));
}

#[test]
fn query_keys_defaults_and_clamping() {
    let c = classify_link("n?rel=teaches&w=0.8&c=0.95");
    assert_eq!(
        (c.rel.as_str(), c.weight, c.confidence),
        ("teaches", 0.8, 0.95)
    );
    let c = classify_link("n?weight=2&confidence=-1&conf=0.5");
    assert_eq!((c.weight, c.confidence), (1.0, 0.5));
    // Malformed, non-finite and empty values fall back; the link survives.
    let c = classify_link("n?w=abc&c=NaN&rel=&weight=inf");
    assert_eq!((c.target.as_str(), c.rel.as_str()), ("n", DEFAULT_REL));
    assert_eq!((c.weight, c.confidence), (1.0, 1.0));
    // A bad value does not undo an earlier good one.
    let c = classify_link("n?w=0.3&w=bad");
    assert_eq!(c.weight, 0.3);
    // Keys are case-sensitive: `W` is an unknown key, carried.
    let c = classify_link("n?W=0.1&by=ai&flag&=x&&");
    assert_eq!(c.weight, 1.0);
    let attrs: Vec<(&str, &str)> = c
        .attrs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(attrs, vec![("W", "0.1"), ("by", "ai"), ("flag", "")]);
}

#[test]
fn paths_are_normalised_like_org() {
    // A line break inside a path, with its blanks, is one space; escaped
    // brackets are unescaped.
    let doc = parse("[[concept:a\n   b][d]] [[x\\]y]]", &ParseOptions::default());
    let links = semantics(&doc).links;
    assert_eq!(links[0].target, "concept:a b");
    assert_eq!(links[1].target, "x]y");
    // A fragment that is only `#` is no fragment.
    assert_eq!(classify_link("x#").fragment, None);
    assert_eq!(classify_link("#frag").target, "");
    assert_eq!(classify_link("#frag").fragment.as_deref(), Some("frag"));
}

/// One query pair: a recognised key with a valid or invalid value, or an
/// unknown key.
fn pair() -> impl Strategy<Value = (String, String)> {
    let key = prop_oneof![
        Just("rel".to_string()),
        Just("w".to_string()),
        Just("weight".to_string()),
        Just("c".to_string()),
        Just("conf".to_string()),
        Just("confidence".to_string()),
        "[a-z]{1,6}".prop_map(|k| format!("x{k}")),
    ];
    let value = prop_oneof![
        (-2.0f64..3.0).prop_map(|f| format!("{f}")),
        Just("NaN".to_string()),
        Just("inf".to_string()),
        Just("-inf".to_string()),
        Just(String::new()),
        "[a-z0-9.#]{0,8}",
    ];
    (key, value)
}

fn query(pairs: &[(String, String)]) -> String {
    let parts: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();
    parts.join("&")
}

/// The oracle: recognised keys read independently of order, last valid wins.
fn expected(pairs: &[(String, String)]) -> (String, f64, f64) {
    let valid = |v: &str| {
        v.parse::<f64>()
            .ok()
            .filter(|f| f.is_finite())
            .map(|f| f.clamp(0.0, 1.0))
    };
    let mut rel = DEFAULT_REL.to_string();
    let (mut w, mut c) = (1.0, 1.0);
    for (k, v) in pairs {
        match k.as_str() {
            "rel" if !v.is_empty() => rel = v.clone(),
            "w" | "weight" => w = valid(v).unwrap_or(w),
            "c" | "conf" | "confidence" => c = valid(v).unwrap_or(c),
            _ => {}
        }
    }
    (rel, w, c)
}

proptest! {
    #[test]
    fn query_parsing_is_total_and_keeps_unknown_keys_in_order(
        // `k-` keeps the id clear of every scheme (`news:`, `id:`, …).
        id in "k-[a-z0-9:_-]{0,12}",
        frag in proptest::option::of("[a-z0-9]{1,5}"),
        pairs in proptest::collection::vec(pair(), 0..8),
    ) {
        let frag_part = frag.as_ref().map(|f| format!("#{f}")).unwrap_or_default();
        let raw = format!("{id}{frag_part}?{}", query(&pairs));
        let c: LinkClass = classify_link(&raw);
        prop_assert_eq!(c.kind, LinkKind::Kb);
        prop_assert_eq!(&c.target, &id);
        prop_assert_eq!(&c.fragment, &frag);
        let (rel, w, conf) = expected(&pairs);
        prop_assert_eq!(&c.rel, &rel);
        prop_assert_eq!((c.weight, c.confidence), (w, conf));
        prop_assert!((0.0..=1.0).contains(&c.weight) && (0.0..=1.0).contains(&c.confidence));
        let unknown: Vec<(String, String)> =
            pairs.iter().filter(|(k, _)| k.starts_with('x')).cloned().collect();
        prop_assert_eq!(c.attrs, unknown);
    }

    /// Recognised keys are orderless: reversing the pairs changes nothing
    /// when each recognised key appears once.
    #[test]
    fn recognised_keys_are_orderless(
        rel in "[a-z]{1,6}", w in 0.0f64..1.0, c in 0.0f64..1.0, extra in "[a-z]{1,4}",
    ) {
        let pairs = [
            ("rel".to_string(), rel),
            ("w".to_string(), format!("{w}")),
            ("c".to_string(), format!("{c}")),
            (format!("x{extra}"), "v".to_string()),
        ];
        let mut rev = pairs.clone();
        rev.reverse();
        let a = classify_link(&format!("n?{}", query(&pairs)));
        let b = classify_link(&format!("n?{}", query(&rev)));
        prop_assert_eq!((&a.rel, a.weight, a.confidence), (&b.rel, b.weight, b.confidence));
        prop_assert_eq!(a.attrs, b.attrs);
    }

    /// Any text classifies without panicking, and a KB target never keeps
    /// the parts the grammar splits off.
    #[test]
    fn classification_is_total(raw in any::<String>()) {
        let c = classify_link(&raw);
        if c.kind == LinkKind::Kb {
            prop_assert!(!c.target.contains('|') && !c.target.contains('?') && !c.target.contains('#'));
        }
    }
}
