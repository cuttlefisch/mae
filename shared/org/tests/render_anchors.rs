//! Where anchors are, and where a link lands (ADR-112 D3, ADR-030).
//!
//! Every rendered headline's heading carries `h-` + the slug of its title,
//! whatever [`HeadlineIds`] says; under `Explicit` the section also carries
//! the node's `id-` or the custom `cid-` anchor. A KB link's `#fragment`
//! (`id:NODE#FRAGMENT`, ADR-030) names a heading by its title: it maps to
//! `#h-<slug(fragment)>` on the target's page, and on this page to that
//! heading when the page has it, else to the node.

use mae_org::{
    heading_anchor, parse, render, render_html, semantics, slug, HeadlineIds, ParseOptions,
    RenderOptions, Resolved, Resolver, SLUG_MAX,
};
use proptest::prelude::*;

fn html_with(doc: &str, ids: HeadlineIds, resolver: &Resolver) -> String {
    let d = parse(doc, &ParseOptions::default());
    let opts = RenderOptions::default().with_headline_ids(ids);
    render_html(&d, &semantics(&d), &opts, resolver)
}

fn html(doc: &str) -> String {
    html_with(doc, HeadlineIds::Explicit, &Resolver::default())
}

fn resolver(pairs: &[(&str, &str)]) -> Resolver {
    let mut r = Resolver::default();
    for (id, href) in pairs {
        r.targets.insert(id.to_string(), Resolved::new(*href));
    }
    r
}

/// The href of the `<a>` whose text is `label`.
fn href_of<'a>(html: &'a str, label: &str) -> &'a str {
    let end = html
        .find(&format!(">{label}</a>"))
        .unwrap_or_else(|| panic!("no link {label}:\n{html}"));
    let start = html[..end].rfind("<a ").expect("an <a>");
    let a = &html[start..end];
    let h = a.find("href=\"").expect("an href") + 6;
    &a[h..h + a[h..].find('"').expect("closed")]
}

/// Every `href="#X"` names an `id="X"` of the same output.
fn same_page_links_land(html: &str) {
    for part in html.split("href=\"#").skip(1) {
        let id = &part[..part.find('"').expect("closed")];
        assert!(
            html.contains(&format!("id=\"{id}\"")),
            "#{id} lands nowhere:\n{html}"
        );
    }
}

/// A headline title: letters of three scripts in both cases, digits,
/// blanks and punctuation that is neither Org markup nor link syntax, so
/// the rendered title is the title as written (digits only and
/// punctuation only included).
fn title() -> impl Strategy<Value = String> {
    let chars = prop::sample::select(vec![
        'a', 'Z', 'é', 'É', 'я', 'Я', '中', 'ß', '2', '9', ' ', ' ', ',', '.', '!', '-', '\'', '(',
        ')', '\u{a0}',
    ]);
    prop::collection::vec(chars, 1..12)
        .prop_map(|cs| cs.into_iter().collect::<String>().trim().to_string())
        .prop_filter("a title", |t| !t.is_empty())
}

/// Every `id="…"` of `html`, in order.
fn ids_of(html: &str) -> Vec<String> {
    html.split(" id=\"")
        .skip(1)
        .map(|p| p[..p.find('"').expect("closed")].to_string())
        .collect()
}

/// The text of the heading whose id is `id` (tags stripped).
fn heading_text(html: &str, id: &str) -> String {
    let at = html
        .find(&format!("id=\"{id}\">"))
        .unwrap_or_else(|| panic!("no {id}"));
    let rest = &html[at..];
    let start = rest.find('>').expect(">") + 1;
    let end = rest.find("</h").expect("a heading");
    let mut text = String::new();
    let mut in_tag = false;
    for c in rest[start..end].chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    text.replace("&amp;", "&").replace("&#39;", "'")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn slug_is_idempotent_and_well_formed(title in "\\PC{0,120}") {
        let s = slug(&title);
        prop_assert_eq!(slug(&s), s.clone());
        prop_assert!(s.len() <= SLUG_MAX, "{} > {}", s.len(), SLUG_MAX);
        prop_assert!(!s.starts_with('-') && !s.ends_with('-'), "{s:?}");
        prop_assert!(!s.contains("--"), "{s:?}");
        // Letters and digits of any script, lower-cased, and single dashes:
        // a valid id after its prefix.
        prop_assert!(s.is_empty() || mae_org::is_valid_id(&format!("h-{s}")), "{s:?}");
    }

    /// Every anchor of a page is unique and well formed, whatever the titles
    /// (repeats, digits only, punctuation only, other scripts).
    #[test]
    fn anchors_are_unique_per_page(titles in prop::collection::vec(title(), 1..14)) {
        let doc: String = titles.iter().map(|t| format!("* {t}\n")).collect();
        for ids in [HeadlineIds::Explicit, HeadlineIds::Slug] {
            let out = html_with(&doc, ids, &Resolver::default());
            let all = ids_of(&out);
            let unique: mae_org::seeded::HashSet<&String> = all.iter().collect();
            prop_assert_eq!(unique.len(), all.len(), "{}", out);
            for id in &all {
                prop_assert!(mae_org::is_valid_id(id), "{id:?}");
            }
        }
    }

    /// `[[id:NODE#FRAGMENT]]` lands on the first heading whose title slugs
    /// as the fragment does, and on the node when no heading does (a
    /// fragment with no letter or digit names none): never on a repeat of
    /// another title, never nowhere.
    #[test]
    fn a_fragment_lands_on_the_heading_whose_title_slugs_the_same(
        titles in prop::collection::vec(title(), 1..10),
        frags in prop::collection::vec(title(), 1..6),
    ) {
        let mut doc = String::from("* Node\n:PROPERTIES:\n:ID: n\n:END:\n");
        for t in &titles {
            doc.push_str(&format!("** {t}\n"));
        }
        doc.push_str("* Links\n");
        let asked: Vec<&String> = frags.iter().chain(titles.iter()).collect();
        for (i, f) in asked.iter().enumerate() {
            doc.push_str(&format!("[[id:n#{f}][lk{i}]]\n"));
        }
        let mut slugs: Vec<String> = titles.iter().map(|t| slug(t)).collect();
        slugs.extend(["node".to_string(), "links".to_string()]);
        for ids in [HeadlineIds::Explicit, HeadlineIds::Slug] {
            let out = html_with(&doc, ids, &Resolver::default());
            same_page_links_land(&out);
            for (i, f) in asked.iter().enumerate() {
                let href = href_of(&out, &format!("lk{i}"));
                let lands = !slug(f).is_empty() && slugs.contains(&slug(f));
                let want = if !lands {
                    match ids {
                        HeadlineIds::Explicit => "#id-n".to_string(),
                        _ => "#h-node".to_string(),
                    }
                } else {
                    format!("#{}", heading_anchor(f))
                };
                prop_assert_eq!(href, want.as_str(), "{}", doc);
                if lands {
                    let title = heading_text(&out, &want[1..]);
                    prop_assert_eq!(slug(&title), slug(f), "{}", out);
                }
            }
        }
    }

    #[test]
    fn slug_is_idempotent_on_long_wordy_titles(words in prop::collection::vec("[A-Za-z0-9]{1,20}", 1..12), sep in "[ ,.:;!?-]{1,3}") {
        let title = words.join(&sep);
        let s = slug(&title);
        prop_assert_eq!(slug(&s), s.clone());
        prop_assert!(s.len() <= SLUG_MAX && !s.ends_with('-'));
    }
}

#[test]
fn a_slug_at_the_limit_does_not_end_in_a_dash() {
    // 64 letters, a blank, then more: the separator must not be kept
    // without the character it separates.
    let title = format!("{} more", "a".repeat(SLUG_MAX));
    assert_eq!(slug(&title), "a".repeat(SLUG_MAX));
    let title = format!("{} b", "a".repeat(SLUG_MAX - 1));
    assert_eq!(slug(&title), "a".repeat(SLUG_MAX - 1));
}

#[test]
fn every_heading_carries_its_slug_anchor_whatever_the_mode() {
    let doc = "* Node one\n:PROPERTIES:\n:ID: n1\n:CUSTOM_ID: c1\n:END:\n* Custom\n:PROPERTIES:\n:CUSTOM_ID: c2\n:END:\n* Plain\n* Plain\n";
    let explicit = html(doc);
    for want in [
        "<section class=\"org-section org-level-1\" id=\"id-n1\"><h1 id=\"h-node-one\">",
        "<section class=\"org-section org-level-1\" id=\"cid-c2\"><h1 id=\"h-custom\">",
        "<section class=\"org-section org-level-1\"><h1 id=\"h-plain\">",
        "<h1 id=\"h-plain--2\">",
    ] {
        assert!(explicit.contains(want), "{want} missing:\n{explicit}");
    }
    let slugs = html_with(doc, HeadlineIds::Slug, &Resolver::default());
    assert!(
        !slugs.contains("id=\"id-") && !slugs.contains("id=\"cid-"),
        "{slugs}"
    );
    for want in [
        "<h1 id=\"h-node-one\">",
        "<h1 id=\"h-custom\">",
        "<h1 id=\"h-plain--2\">",
    ] {
        assert!(slugs.contains(want), "{want} missing:\n{slugs}");
    }
}

#[test]
fn a_cross_page_fragment_lands_on_the_target_heading() {
    let r = resolver(&[("x", "/notes/x.html"), ("y", "/notes/y.html#own")]);
    let doc = "[[id:x#Rope Internals][a]] [[id:x#rope-internals][b]] [[x#Rope   internals!][c]] [[id:y#rope][d]] [[id:x#¿?][e]]\n";
    let out = html_with(doc, HeadlineIds::Explicit, &r);
    // Written as the title or as its slug: the same anchor.
    for label in ["a", "b", "c"] {
        assert_eq!(href_of(&out, label), "/notes/x.html#h-rope-internals");
    }
    // The resolver's own fragment wins.
    assert_eq!(href_of(&out, "d"), "/notes/y.html#own");
    // A fragment with no letter or digit names no heading.
    assert_eq!(href_of(&out, "e"), "/notes/x.html");
    // On the target page, that anchor exists.
    let target = "* Rope internals\n";
    assert!(html(target).contains(&format!("id=\"{}\"", heading_anchor("Rope Internals"))));
}

#[test]
fn a_same_page_fragment_lands_on_the_heading_or_else_the_node() {
    let doc = "* Buffer\n:PROPERTIES:\n:ID: buf\n:END:\n** Rope internals\nText.\n* Links\n[[id:buf#rope-internals][there]] [[id:buf#missing][fallback]] [[id:buf][node]]\n";
    let explicit = html(doc);
    assert_eq!(href_of(&explicit, "there"), "#h-rope-internals");
    assert_eq!(href_of(&explicit, "fallback"), "#id-buf");
    assert_eq!(href_of(&explicit, "node"), "#id-buf");
    same_page_links_land(&explicit);
    let slugs = html_with(doc, HeadlineIds::Slug, &Resolver::default());
    assert_eq!(href_of(&slugs, "there"), "#h-rope-internals");
    assert_eq!(href_of(&slugs, "fallback"), "#h-buffer");
    same_page_links_land(&slugs);
}

#[test]
fn an_archived_headlines_custom_id_resolves_as_in_org() {
    // Org exports the archived headline (not its contents) with its
    // CUSTOM_ID as the anchor, and `[[#carch]]` lands there.
    let doc = "* C :ARCHIVE:\n:PROPERTIES:\n:CUSTOM_ID: carch\n:END:\nc body\n* D\n[[#carch][to archived]]\n";
    let out = html(doc);
    assert!(out.contains("id=\"cid-carch\""), "{out}");
    assert_eq!(href_of(&out, "to archived"), "#cid-carch");
    assert!(!out.contains("c body"), "{out}");
    same_page_links_land(&out);
}

#[test]
fn node_metadata_names_the_anchor_links_land_on() {
    let doc = "* Node\n:PROPERTIES:\n:ID: n1\n:END:\n";
    let d = parse(doc, &ParseOptions::default());
    let sem = semantics(&d);
    for (ids, want) in [
        (HeadlineIds::Explicit, "id-n1"),
        (HeadlineIds::Slug, "h-node"),
    ] {
        let opts = RenderOptions::default().with_headline_ids(ids);
        let r = render(&d, &sem, &opts, &Resolver::default());
        assert_eq!(r.nodes[0].anchor.as_deref(), Some(want));
    }
}

/// Review B4: a repeat suffix in the slug alphabet made `#x 2` land on the
/// second `x`, and every non-ASCII title slug to `h`, so `#Глава 2` landed
/// on whichever came first.
#[test]
fn repeats_and_other_scripts_never_take_another_titles_anchor() {
    let doc = "* x\n* x\n* x 2\n* Глава 1\n* Глава 2\n* 中文\n* Links\n[[#x 2][a]]\n";
    let out = html(doc);
    for want in [
        "<h1 id=\"h-x\">x</h1>",
        "<h1 id=\"h-x--2\">x</h1>",
        "<h1 id=\"h-x-2\">x 2</h1>",
        "<h1 id=\"h-глава-1\">Глава 1</h1>",
        "<h1 id=\"h-глава-2\">Глава 2</h1>",
        "<h1 id=\"h-中文\">中文</h1>",
    ] {
        assert!(out.contains(want), "{want} missing:\n{out}");
    }
    let r = resolver(&[("x", "/x.html")]);
    let doc = "[[id:x#Глава 2][g]] [[id:x#x 2][x2]]\n";
    let out = html_with(doc, HeadlineIds::Explicit, &r);
    // Another page's anchor, percent-encoded into its URL.
    assert_eq!(
        href_of(&out, "g"),
        "/x.html#h-%D0%B3%D0%BB%D0%B0%D0%B2%D0%B0-2"
    );
    assert_eq!(href_of(&out, "x2"), "/x.html#h-x-2");
}

/// A title with objects in it: words, sub- and superscripts, entities,
/// emphasis, code and verbatim, a hidden inline src block, other scripts.
fn object_title() -> impl Strategy<Value = String> {
    let piece = prop::sample::select(vec![
        "H_2O",
        "A^2",
        "x_{ij}",
        "\\alpha",
        "Caf\\'e",
        "*bold*",
        "/it/",
        "_u_",
        "+s+",
        "~code~",
        "=verb=",
        "src_sh{echo x}",
        "levels",
        "Straße",
        "ΣΟΦΟΣ",
        "İstanbul",
        "2",
        "notes",
        "\\to",
    ]);
    prop::collection::vec(piece, 1..5).prop_map(|ps| ps.join(" "))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Review round 2 (B4): a fragment written as the title (objects and
    /// all) lands on that heading, on this page and in another page's
    /// anchor. `H_2O levels` slugged `h-h2o-levels` but the fragment
    /// `#H_2O levels` asked for `h-h-2o-levels`.
    #[test]
    fn a_fragment_written_as_the_title_lands_on_it(titles in prop::collection::vec(object_title(), 1..5)) {
        let mut doc = String::from("* Node\n:PROPERTIES:\n:ID: n\n:END:\n");
        for t in &titles {
            doc.push_str(&format!("** {t}\n"));
        }
        doc.push_str("* Links\n");
        for (i, t) in titles.iter().enumerate() {
            doc.push_str(&format!("[[id:n#{t}][lk{i}]]\n"));
        }
        let out = html(&doc);
        let r = resolver(&[("n", "/n.html")]);
        let cross = html_with(&doc, HeadlineIds::Explicit, &r);
        let heads: Vec<String> = out
            .split("<h2 id=\"")
            .skip(1)
            .map(|p| p[..p.find('"').unwrap()].to_string())
            .collect();
        for (i, head) in heads.iter().enumerate().take(titles.len()) {
            let href = href_of(&out, &format!("lk{i}"));
            // A title with no letter or digit once rendered (only a hidden
            // src block) has the `h--untitled` anchor, which no fragment names.
            if head.starts_with("h--untitled") {
                prop_assert_eq!(href, "#id-n");
                continue;
            }
            prop_assert_ne!(href, "#id-n", "fell back to the node:\n{}", doc);
            // It lands on a heading whose rendered title slugs as this one's.
            let mine = slug(&heading_text(&out, head));
            let there = slug(&heading_text(&out, &href[1..]));
            prop_assert_eq!(&there, &mine, "{}", out);
            // Another page: the same anchor, percent-encoded.
            let x = href_of(&cross, &format!("lk{i}"));
            let want = format!("#{}", &href[1..]);
            prop_assert!(
                x.starts_with("/n.html#") && percent_decode(&x["/n.html".len()..]) == want,
                "{x} vs {want}"
            );
        }
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

/// The reviewer's cases: each fragment is the title as written.
#[test]
fn fragments_with_objects_land_on_their_heading() {
    let doc = "* Node\n:PROPERTIES:\n:ID: n1\n:END:\n** H_2O levels\n** Caf\\'e \\alpha notes\n** A^2 and B\n** İstanbul ΣΟΦΟΣ Straße ǅemal\n** Ⓐ circled\nSee [[id:n1#H_2O levels][a]], [[id:n1#Caf\\'e \\alpha notes][b]], [[id:n1#A^2 and B][c]], [[id:n1#İstanbul ΣΟΦΟΣ Straße ǅemal][d]], [[id:n1#Ⓐ circled][e]].\n";
    let out = html(doc);
    // (`\'e` is no Org entity: it renders as written.)
    for (label, want) in [
        ("a", "#h-h2o-levels"),
        ("b", "#h-caf-e-α-notes"),
        ("c", "#h-a2-and-b"),
        ("d", "#h-İstanbul-σοφοσ-straße-ǆemal"),
        ("e", "#h-circled"),
    ] {
        assert_eq!(href_of(&out, label), want, "{out}");
        assert!(
            out.contains(&format!("<h2 id=\"{}\">", &want[1..])),
            "{out}"
        );
    }
    // A src block its heading shows (`:exports code` in its own drawer):
    // the slug never holds code, so the fragment, read with no scope of
    // its own, lands there too.
    let doc = "* Node\n:PROPERTIES:\n:ID: n2\n:END:\n** Run src_sh{ls} now\n:PROPERTIES:\n:header-args: :exports code\n:END:\nSee [[id:n2#Run src_sh{ls} now][r]].\n";
    let out = html(doc);
    assert!(out.contains("<h2 id=\"h-run-now\">Run <code"), "{out}");
    assert_eq!(href_of(&out, "r"), "#h-run-now", "{out}");
}

/// A title with links in it: KB links with and without a description,
/// external links, words.
fn linked_title() -> impl Strategy<Value = String> {
    let piece = prop::sample::select(vec![
        "See",
        "[[id:other]]",
        "[[id:other][the other]]",
        "[[https://x.example]]",
        "[[https://x.example][site]]",
        "<https://y.example>",
        "notes",
        "[[*Elsewhere]]",
    ]);
    prop::collection::vec(piece, 1..5).prop_map(|ps| ps.join(" "))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Review round 3: anchors are frozen at the tag, so they must not
    /// depend on resolver data. `* See [[id:other]]` was `h-see-other`
    /// with a resolver title and `h-see` without. In a slug a link is its
    /// rendered description only; a link with none adds nothing.
    #[test]
    fn heading_anchors_do_not_depend_on_the_resolver(titles in prop::collection::vec(linked_title(), 1..6)) {
        let doc: String = titles.iter().map(|t| format!("* {t}\n")).collect();
        let titled = Resolver::default()
            .with_target("other", Resolved::new("/o.html").with_title("A Resolved Title"));
        let ids = |r: &Resolver| -> Vec<String> {
            html_with(&doc, HeadlineIds::Slug, r)
                .split("<h1 id=\"")
                .skip(1)
                .map(|p| p[..p.find('"').unwrap()].to_string())
                .collect()
        };
        prop_assert_eq!(ids(&Resolver::default()), ids(&titled), "{}", doc);
    }
}

#[test]
fn a_link_in_a_title_slugs_as_its_description() {
    let doc = "* See [[id:other]]\n* See [[id:other][the other]] too\n";
    let out = html(doc);
    assert!(out.contains("<h1 id=\"h-see\">"), "{out}");
    assert!(out.contains("<h1 id=\"h-see-the-other-too\">"), "{out}");
}

/// A target and the link searching for it match with blanks collapsed and
/// case folded, as in Emacs 30.2 (`<<Foo   Bar>>` is where `[[foo bar]]`
/// and `[[Foo Bar]]` both land): the one name key `replace_body` compares
/// names by too (review round 4, M2).
#[test]
fn a_target_matches_its_link_in_any_case_and_spacing() {
    let h = html("Para <<Foo   Bar>> x.\n\nSee [[foo bar]] and [[Foo Bar]] y.\n");
    assert_eq!(h.matches("href=\"#t-1\"").count(), 2, "{h}");
}
