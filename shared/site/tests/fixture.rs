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
    Addressing::Export {
        base: String::new(),
    }
}

fn bundle() -> Site {
    derive(&sources(&fixture().join("bundle")), &export())
}

#[test]
fn the_whole_bundle_derives_and_nothing_is_withheld() {
    let site = bundle();
    assert_eq!(site.withheld, vec![], "withheld");
    assert_eq!(site.pages.len(), 16);
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
    assert!(
        matches!(site.withheld.as_slice(), [Withheld::CaseCollision { ids }] if ids.len() == 2)
    );
    let live = derive(&both, &Addressing::Live { kb_id: "k".into() });
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
            r#"<p class="x"><a href="https://e.example/a" data-org-rel="cites">l</a></p>"#
        ),
        r#"<p class="x"><a href="https://e.example/a" data-org-rel="cites">l</a></p>"#
    );
}
