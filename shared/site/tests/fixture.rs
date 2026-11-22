//! The core against the parity fixture (docs/verification/fixtures/kb-site-parity),
//! the same bundle the acceptance suite builds.

use std::path::{Path, PathBuf};

use mae_site::sanitize::sanitize_fragment;
use mae_site::{derive, Addressing, Site, SourcePage, Withheld};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/verification/fixtures/kb-site-parity")
}

fn sources(dir: &Path) -> Vec<SourcePage> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "org") {
                let origin = p.strip_prefix(dir).unwrap().display().to_string();
                out.push(SourcePage {
                    origin,
                    text: std::fs::read_to_string(&p).unwrap(),
                });
            }
        }
    }
    out.sort_by(|a, b| a.origin.cmp(&b.origin));
    out
}

fn export() -> Addressing {
    Addressing::export("").unwrap()
}

fn page(id: &str, extra: &str) -> SourcePage {
    SourcePage {
        origin: format!("{id}.org"),
        text: format!(":PROPERTIES:\n:ID: {id}\n:END:\n#+title: {id}\n\n{extra}\n"),
    }
}

fn bundle() -> Site {
    derive(&sources(&fixture().join("bundle")), &export())
}

#[test]
fn the_whole_bundle_derives_and_nothing_is_withheld() {
    let site = bundle();
    assert_eq!(site.withheld, vec![], "withheld");
    // Every page of the bundle, whatever the fixture grows to.
    assert_eq!(site.pages.len(), sources(&fixture().join("bundle")).len());
    assert!(site.pages.len() >= 16);
    for (id, p) in &site.pages {
        assert_eq!(&p.id, id);
        assert_eq!(p.href, format!("/{}/", mae_site::address::segment(id)));
        assert!(!p.html.is_empty(), "{id} has no body");
    }
}

#[test]
fn the_sanitiser_changes_nothing_mae_org_legitimately_writes() {
    // Each fixture page rendered by mae-org directly: the allow-list must
    // pass it through unchanged, or the sanitiser is eating content.
    for s in sources(&fixture().join("bundle")) {
        let doc = mae_org::parse(&s.text, &mae_org::ParseOptions::default());
        let sem = mae_org::semantics(&doc);
        let opts =
            mae_org::RenderOptions::default().with_headline_ids(mae_org::HeadlineIds::Explicit);
        let html = mae_org::to_html(
            &mae_org::render(&doc, &sem, &opts, &mae_org::Resolver::default()).body,
        );
        assert_eq!(sanitize_fragment(&html), html, "{}", s.origin);
    }
}

#[test]
fn hostile_markup_does_not_survive() {
    let site = derive(
        &sources(&fixture().join("negative/hostile-html/bundle")),
        &export(),
    );
    let page = site
        .pages
        .values()
        .find(|p| p.title == "Hostile")
        .expect("the hostile page publishes");
    let h = page.html.to_lowercase();
    for bad in ["<script", "onerror", "onclick", "javascript:", "<img src=x"] {
        assert!(!h.contains(bad), "{bad} survived: {}", page.html);
    }
}

#[test]
fn content_cannot_spell_a_chrome_id() {
    let html = r#"<p id="mae-header">x</p><p id="id-ok">y</p><p id="evil">z</p>"#;
    assert_eq!(
        sanitize_fragment(html),
        r#"<p>x</p><p id="id-ok">y</p><p>z</p>"#
    );
}

#[test]
fn each_withheld_case_is_withheld_for_its_reason() {
    let neg = fixture().join("negative");
    let case = |name: &str| derive(&sources(&neg.join(name).join("bundle")), &export()).withheld;
    assert!(matches!(
        case("withheld-no-file-id").as_slice(),
        [Withheld::NoFileId { .. }]
    ));
    assert!(
        matches!(case("withheld-duplicate-id").as_slice(), [Withheld::DuplicateId { origins, .. }] if origins.len() == 2)
    );
    assert!(matches!(
        case("withheld-undefined-footnote").as_slice(),
        [Withheld::Unfaithful {
            kind: "footnote_undefined",
            ..
        }]
    ));
}

#[test]
fn links_resolve_to_pages_and_a_missing_id_stays_unresolved() {
    let site = bundle();
    let home = site
        .pages
        .values()
        .find(|p| p.title == "Fixture home")
        .unwrap();
    let started = site
        .pages
        .values()
        .find(|p| p.title == "Getting started")
        .unwrap();
    assert!(
        home.html.contains(&format!("href=\"{}\"", started.href)),
        "{}",
        home.html
    );
    assert!(
        home.html.contains("org-unresolved"),
        "the absent ID: {}",
        home.html
    );
}

#[test]
fn the_export_refuses_ids_equal_under_case_folding_and_the_live_app_does_not() {
    let page = |id: &str| SourcePage {
        origin: format!("{id}.org"),
        text: format!(":PROPERTIES:\n:ID: {id}\n:END:\n#+title: {id}\n\nx\n"),
    };
    let both = [page("Abc"), page("abc")];
    let site = derive(&both, &export());
    assert!(site.pages.is_empty());
    assert!(matches!(site.withheld.as_slice(), [Withheld::FsCollision { ids }] if ids.len() == 2));
    let live = derive(&both, &Addressing::live("k").unwrap());
    assert_eq!(live.pages.len(), 2);
}

/// The fixture's hostile page never reaches the sanitiser: mae-org already
/// drops raw HTML, export blocks and `javascript:` links. So the final
/// allow-list is tested on hostile HTML directly, as a template or a future
/// renderer change could hand it over (D19: it runs over everything).
#[test]
fn the_sanitiser_strips_what_a_template_or_renderer_could_inject() {
    let cases = [
        r#"<script>alert(1)</script><p>ok</p>"#,
        r#"<img src=x onerror="alert(1)"><p onclick="alert(1)">ok</p>"#,
        r#"<a href="javascript:alert(1)">a</a><a href="JaVaScRiPt:alert(1)">b</a>"#,
        r#"<a href="java&#115;cript:alert(1)">c</a><a href="data:text/html,<script>alert(1)</script>">d</a>"#,
        r#"<iframe src="https://evil.example"></iframe><object data="x"></object><embed src="x">"#,
        r#"<svg onload="alert(1)"><circle/></svg><math><mi>x</mi></math>"#,
        r#"<p style="background:url(https://evil.example)">s</p><form action="x"><input name="q"></form>"#,
        r#"<p id="mae-header">clobber</p><a name="mae-x">n</a><base href="https://evil.example/">"#,
        r#"<!-- <script>alert(1)</script> --><template><p>t</p></template>"#,
    ];
    for html in cases {
        let out = sanitize_fragment(html).to_lowercase();
        for bad in [
            "<script",
            "onerror",
            "onclick",
            "onload",
            "javascript:",
            "data:text",
            "<iframe",
            "<object",
            "<embed",
            "<svg",
            "<math",
            "style=",
            "<form",
            "<input",
            "mae-header",
            "name=",
            "<base",
            "<!--",
            "<template",
        ] {
            assert!(!out.contains(bad), "{bad} survived {html:?}: {out}");
        }
    }
    // And what is allowed still comes through.
    assert_eq!(
        sanitize_fragment(
            r#"<p class="org-x"><a href="https://e.example/a" data-org-rel="cites">l</a></p>"#
        ),
        r#"<p class="org-x"><a href="https://e.example/a" data-org-rel="cites">l</a></p>"#
    );
}

/// Each defect the first review found, as the input that triggered it.
#[test]
fn a_dot_segment_id_is_withheld_and_never_becomes_an_href() {
    let site = derive(
        &[page(".", ""), page("..", ""), page("ok", "[[id:..][up]]")],
        &export(),
    );
    assert_eq!(site.pages.keys().collect::<Vec<_>>(), vec!["ok"]);
    assert_eq!(
        site.withheld
            .iter()
            .filter(|w| matches!(w, Withheld::InvalidId { .. }))
            .count(),
        2
    );
    let ok = &site.pages["ok"].html;
    assert!(
        !ok.contains("href=\"/../\"") && !ok.contains("href=\"/./\""),
        "{ok}"
    );
}

#[test]
fn a_node_claiming_another_pages_id_withholds_both_and_links_resolve_nowhere() {
    let thief = SourcePage {
        origin: "b.org".into(),
        text: ":PROPERTIES:\n:ID: b\n:END:\n#+title: B\n* Stolen\n:PROPERTIES:\n:ID: a\n:END:\n"
            .into(),
    };
    let site = derive(
        &[page("a", ""), thief, page("c", "[[id:a][to a]]")],
        &export(),
    );
    assert!(
        !site.pages.contains_key("a") && !site.pages.contains_key("b"),
        "{:?}",
        site.pages.keys()
    );
    assert!(
        matches!(site.withheld.as_slice(), [Withheld::DuplicateId { id, origins }] if id == "a" && origins.len() == 2)
    );
    let c = &site.pages["c"].html;
    assert!(c.contains("org-unresolved") && !c.contains("href="), "{c}");
}

#[test]
fn a_page_withheld_as_unfaithful_is_not_a_link_target() {
    let bad = page("u", "A note[fn:missing].");
    let site = derive(&[bad, page("c", "[[id:u][to u]]")], &export());
    assert!(matches!(
        site.withheld.as_slice(),
        [Withheld::Unfaithful {
            kind: "footnote_undefined",
            ..
        }]
    ));
    let c = &site.pages["c"].html;
    assert!(!c.contains("/u/"), "a link to an unpublished page: {c}");
}

#[test]
fn a_uuid_custom_id_on_two_pages_resolves_nowhere() {
    let uuid = "0a1b2c3d-0000-4000-8000-000000000abc";
    let holder = |id: &str| {
        SourcePage {
        origin: format!("{id}.org"),
        text: format!(":PROPERTIES:\n:ID: {id}\n:END:\n#+title: {id}\n* S\n:PROPERTIES:\n:CUSTOM_ID: {uuid}\n:END:\n"),
    }
    };
    let one = derive(
        &[holder("p"), page("c", &format!("[[id:{uuid}][to it]]"))],
        &export(),
    );
    assert!(
        one.pages["c"].html.contains("href=\"/p/#cid-"),
        "{}",
        one.pages["c"].html
    );
    let two = derive(
        &[
            holder("p"),
            holder("q"),
            page("c", &format!("[[id:{uuid}][to it]]")),
        ],
        &export(),
    );
    assert!(
        two.pages["c"].html.contains("org-unresolved"),
        "{}",
        two.pages["c"].html
    );
}

#[test]
fn the_sanitiser_refuses_protocol_relative_urls_spoofed_classes_and_misplaced_attributes() {
    for (html, bad) in [
        (r#"<a href="//evil.example/x">a</a>"#, "evil"),
        (r#"<a href="\\evil.example/x">a</a>"#, "evil"),
        (r#"<a href=" //evil.example/x">a</a>"#, "evil"),
        ("<a href=\"/\t/evil.example/x\">a</a>", "evil"),
        ("<a href=\"/\n/evil.example/x\">a</a>", "evil"),
        ("<img src=\"/\r/evil.example/x.png\">", "evil"),
        (r#"<a href="/\evil.example/x">a</a>"#, "evil"),
        (r#"<span href="/x" src="/y" value="1">s</span>"#, "href"),
        (r#"<p class="mae-panel org-link">p</p>"#, "mae-panel"),
    ] {
        let out = sanitize_fragment(html);
        assert!(!out.contains(bad), "{html} -> {out}");
    }
    assert_eq!(
        sanitize_fragment(r#"<p class="mae-panel org-link">p</p>"#),
        r#"<p class="org-link">p</p>"#
    );
    assert_eq!(
        sanitize_fragment(r#"<p class="mae-panel">p</p>"#),
        "<p>p</p>"
    );
}

#[test]
fn an_addressing_cannot_be_built_from_a_bad_base_or_kb_id() {
    assert!(Addressing::export("//evil").is_err());
    assert!(Addressing::live("Not-Valid").is_err());
}

/// P-36: every published page that links to a page, and nothing else.
#[test]
fn backlinks_are_the_published_pages_that_link_here_and_nothing_else() {
    let src = |id: &str, body: &str| SourcePage {
        origin: format!("{id}.org"),
        text: format!(":PROPERTIES:\n:ID: {id}\n:END:\n#+title: {id}\n\n{body}\n"),
    };
    let site = derive(
        &[
            src(
                "a",
                "To [[id:b-head][B's heading]] and to [[id:a][myself]].",
            ),
            src("b", "* Head\n:PROPERTIES:\n:ID: b-head\n:END:\nB."),
            src("c", "To [[id:a][A]]."),
            src("d", "To [[id:a][A]], with a note[fn:missing]."),
            src("e", "* Private :noexport:\nTo [[id:a][A]]."),
        ],
        &export(),
    );
    assert!(!site.pages.contains_key("d"), "d is withheld");
    let ids = |p: &str| {
        site.pages[p]
            .backlinks
            .iter()
            .map(|b| b.id.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids("b"),
        vec!["a"],
        "a link to a page's node is a link to the page"
    );
    assert_eq!(
        ids("a"),
        vec!["c"],
        "not d (withheld), not e (noexport), not a itself"
    );
    assert_eq!(ids("c"), Vec::<&str>::new());
    assert_eq!(site.pages["b"].backlinks[0].href, site.pages["a"].href);
}

#[test]
fn every_fixture_backlink_is_a_published_page_that_really_links_there() {
    let site = bundle();
    let mut total = 0;
    for (id, page) in &site.pages {
        for b in &page.backlinks {
            total += 1;
            let from = site
                .pages
                .get(&b.id)
                .unwrap_or_else(|| panic!("{} backlinks unpublished {}", id, b.id));
            assert!(
                from.html.contains(&format!("href=\"{}", page.href)),
                "{} lists {} without a link",
                id,
                b.id
            );
        }
    }
    assert!(total > 0, "the fixture has links");
}

/// The backlink count of every fixture page, as the parity suite computes it
/// independently from the bundle's links (P-36).
#[test]
fn each_fixture_page_has_the_backlinks_the_parity_suite_counts() {
    let expected: &[(&str, usize)] = &[
        ("index.org", 3),
        ("reference/code.org", 2),
        ("reference/diagrams.org", 2),
        ("reference/long-page.org", 2),
        ("whats-new.org", 2),
        ("guides/getting-started.org", 4),
        ("guides/advanced/configuration.org", 1),
        ("guides/advanced/deep/deeper/deepest-page.org", 1),
        ("guides/a-page-whose-title-is-long-enough-to-wrap.org", 0),
        ("journal/2026-01-15.org", 0),
        ("journal/2026-02-15.org", 0),
        ("journal/2026-03-15.org", 0),
        ("reference/special-blocks.org", 0),
        ("reference/wide-table.org", 0),
        ("status/draft.org", 0),
        ("status/malformed-date.org", 0),
        ("tagging/malformed-tags.org", 0),
    ];
    let sources = sources(&fixture().join("bundle"));
    assert_eq!(
        sources.len(),
        expected.len(),
        "every page has an expectation"
    );
    let site = derive(&sources, &export());
    for (origin, want) in expected {
        let s = sources
            .iter()
            .find(|s| &s.origin == origin)
            .unwrap_or_else(|| panic!("no {origin}"));
        let id = mae_org::semantics(&mae_org::parse(&s.text, &mae_org::ParseOptions::default()))
            .nodes
            .iter()
            .find(|n| n.origin == mae_org::NodeOrigin::File)
            .map(|n| n.id.clone())
            .unwrap();
        assert_eq!(site.pages[&id].backlinks.len(), *want, "{origin}");
    }
}
