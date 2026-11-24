//! Slot components (ADR-119 D18, Amendment 4 H1/H2; parity P-153 to P-163):
//! the engine against the example component directory
//! (docs/verification/fixtures/components/slots) and against hostile
//! components written here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mae_site::components::{
    iso_date, Components, LoadError, PageContext, PageData, SiteContext, Slot, FUEL_LIMIT,
    MAX_DATA_ITEMS,
};
use mae_site::Addressing;
use proptest::prelude::*;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/verification/fixtures/components/slots")
}

/// Every file of the fixture directory but the manifest and the expected
/// output, by name: what the daemon's loader will hand the engine.
fn fixture_files() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for e in std::fs::read_dir(fixture()).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_str().unwrap().to_string();
        if p.is_file() && name != "components.toml" {
            out.insert(name, std::fs::read_to_string(&p).unwrap());
        }
    }
    out
}

fn fixture_components() -> Components {
    let manifest = std::fs::read_to_string(fixture().join("components.toml")).unwrap();
    Components::load(&manifest, &fixture_files()).unwrap()
}

fn site(published: &[(&str, &str)], pages: &[(&str, &str)]) -> Arc<SiteContext> {
    Arc::new(SiteContext {
        locale: "en-US".into(),
        base_path: "/kb".into(),
        kb_id: "demo".into(),
        pages: pages
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        published: Arc::new(
            published
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        ),
    })
}

fn page(id: &str, props: &[(&str, &str)], site: &Arc<SiteContext>) -> PageContext {
    PageContext {
        page: PageData {
            id: id.into(),
            title: format!("Page {id}"),
            url: format!("/kb/demo/{id}"),
            properties: props
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            dates: BTreeMap::new(),
            tags: vec![],
        },
        site: site.clone(),
    }
}

/// Load an inline manifest whose files are given as (name, text).
fn load(manifest: &str, files: &[(&str, &str)]) -> Result<Components, Vec<LoadError>> {
    let files = files
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Components::load(manifest, &files)
}

/// One component `c` in `slot`, declaring `attrs`, with template `tmpl`.
fn one(slot: &str, attrs: &str, tmpl: &str) -> Result<Components, Vec<LoadError>> {
    load(
        &format!(
            "[[component]]\nname = \"c\"\nslot = \"{slot}\"\ntemplate = \"c.html\"\nattributes = [{attrs}]\n"
        ),
        &[("c.html", tmpl)],
    )
}

fn plain_ctx() -> PageContext {
    page("p", &[], &site(&[], &[]))
}

/// Render `slot` with the single component `c`.
fn render_one(slot: Slot, attrs: &str, tmpl: &str) -> (String, Vec<String>) {
    let c = one(slot.name(), attrs, tmpl).unwrap();
    let (html, errors) = c.render_slot(slot, &plain_ctx());
    (html, errors.iter().map(|e| e.to_string()).collect())
}

// ---------------------------------------------------------------------------
// The examples (P-160 to P-163), with exact expected output.

/// Every slot of one page, each under a marker, as expected/<name>.html holds.
fn all_slots(c: &Components, ctx: &PageContext) -> String {
    let mut out = String::new();
    for slot in Slot::ALL {
        let (html, errors) = c.render_slot(slot, ctx);
        assert_eq!(errors, vec![], "{}", slot.name());
        out.push_str(&format!("<!-- slot: {} -->\n{html}\n", slot.name()));
    }
    out
}

fn expected(name: &str) -> String {
    std::fs::read_to_string(fixture().join("expected").join(name)).unwrap()
}

fn fixture_site(published: &[(&str, &str)]) -> Arc<SiteContext> {
    site(published, &[("changes", "/kb/demo/site/changes")])
}

#[test]
fn the_examples_render_as_expected_on_a_draft_page() {
    let c = fixture_components();
    let s = fixture_site(&[("release-notes", "/kb/demo/release-notes")]);
    let ctx = page(
        "guide",
        &[("STATE", "Draft"), ("UPDATED", "2026-11-02")],
        &s,
    );
    assert_eq!(all_slots(&c, &ctx), expected("draft.html"));
}

#[test]
fn the_examples_leave_out_what_a_plain_page_lacks() {
    // No STATE, no UPDATED, and the what's-new node is not published in this
    // view: no badge, no line, no link.
    let c = fixture_components();
    let s = fixture_site(&[("guide", "/kb/demo/guide")]);
    let ctx = page("plain", &[], &s);
    assert_eq!(all_slots(&c, &ctx), expected("plain.html"));
}

#[test]
fn invalid_metadata_gives_a_notice_or_nothing_never_a_failure() {
    // P-150: an unknown state gets the badge's notice; a malformed date
    // omits the line.
    let c = fixture_components();
    let s = fixture_site(&[("release-notes", "/kb/demo/release-notes")]);
    for (state, updated) in [
        ("archived", "2026-02-30"),
        ("x\" onmouseover=\"alert(1)", "[2026-11-02 Mon]"),
    ] {
        let ctx = page("odd", &[("STATE", state), ("UPDATED", updated)], &s);
        assert_eq!(all_slots(&c, &ctx), expected("invalid.html"), "{state}");
    }
}

#[test]
fn the_site_changes_page_lists_versions_newest_first() {
    let c = fixture_components();
    let pages = c.pages();
    assert_eq!(pages.len(), 1);
    assert_eq!(
        (
            pages[0].component.as_str(),
            pages[0].name.as_str(),
            pages[0].title.as_str()
        ),
        ("versions", "changes", "Site changes")
    );
    let html = c
        .render_page("changes", &fixture_site(&[]))
        .unwrap()
        .unwrap();
    assert_eq!(html, expected("changes.html"));
    assert!(c.render_page("nope", &fixture_site(&[])).is_none());
    assert_eq!(
        c.page_hrefs(&Addressing::live("demo").unwrap()),
        BTreeMap::from([("changes".to_string(), "/kb/demo/site/changes".to_string())])
    );
}

#[test]
fn the_fixture_declares_its_behaviour_and_stylesheet() {
    use mae_site::components::AssetKind::{Script, Stylesheet};
    let c = fixture_components();
    assert_eq!(
        c.assets(),
        vec![
            ("state-badge", Stylesheet, "components.css"),
            ("copy-link", Script, "copy-link.js"),
            ("copy-link", Stylesheet, "components.css"),
            ("versions", Stylesheet, "components.css"),
        ]
    );
}

// ---------------------------------------------------------------------------
// P-158: an empty slot is the empty string.

#[test]
fn an_empty_slot_is_the_empty_string() {
    let c = fixture_components();
    let s = fixture_site(&[]);
    for slot in [Slot::DocumentHead, Slot::RightPanel, Slot::PageFooter] {
        assert_eq!(
            c.render_slot(slot, &page("x", &[], &s)),
            (String::new(), vec![])
        );
    }
    let none = load("", &[]).unwrap();
    for slot in Slot::ALL {
        assert_eq!(
            none.render_slot(slot, &plain_ctx()),
            (String::new(), vec![])
        );
    }
}

// ---------------------------------------------------------------------------
// P-153: each slot gets exactly its own components.

#[test]
fn each_slot_renders_only_its_own_components_in_manifest_order() {
    let mut manifest = String::new();
    let mut files = Vec::new();
    for (i, slot) in Slot::ALL.iter().enumerate() {
        for n in 0..2 {
            manifest.push_str(&format!(
                "[[component]]\nname = \"c{i}-{n}\"\nslot = \"{}\"\ntemplate = \"c{i}-{n}.html\"\n",
                slot.name()
            ));
            let body = if *slot == Slot::DocumentHead {
                format!("<meta name=\"description\" content=\"{i}-{n}\">")
            } else {
                format!("<p class=\"site-c{i}-{n}\">{i}-{n}</p>")
            };
            files.push((format!("c{i}-{n}.html"), body));
        }
    }
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let c = load(&manifest, &files).unwrap();
    for (i, slot) in Slot::ALL.iter().enumerate() {
        let (html, errors) = c.render_slot(*slot, &plain_ctx());
        assert_eq!(errors, vec![]);
        let want = if *slot == Slot::DocumentHead {
            format!(
                "<meta name=\"description\" content=\"{i}-0\"><meta name=\"description\" content=\"{i}-1\">"
            )
        } else {
            format!("<p class=\"site-c{i}-0\">{i}-0</p><p class=\"site-c{i}-1\">{i}-1</p>")
        };
        assert_eq!(html, want, "{}", slot.name());
    }
}

// ---------------------------------------------------------------------------
// P-155: the component sanitiser.

#[test]
fn a_component_loses_everything_it_did_not_declare_or_may_not_write() {
    let tmpl = concat!(
        "<script>alert(1)</script>",
        "<style>p{}</style>",
        "<p onclick=\"x()\" style=\"color:red\" id=\"mae-search\"",
        " class=\"mae-chrome org-content language-rust site-kept site-\"",
        " data-x=\"1\" data-site-ok=\"2\" data-org-rel=\"z\" data-mae-n=\"3\"",
        " aria-label=\"kept\" aria-hidden=\"true\" role=\"note\">t</p>",
        "<a href=\"javascript:alert(1)\">j</a>",
        "<a href=\" javascript:alert(1)\">j2</a>",
        "<a href=\"//evil.example/x\">p</a>",
        "<a href=\"/\\evil.example\">b</a>",
        "<a href=\"/kb/k/ok\" data-site-ok=\"javascript:alert(1)\">ok</a>",
        "<img src=\"//evil.example/x.png\" alt=\"i\">",
        "<iframe srcdoc=\"<script>alert(1)</script>\"></iframe>",
        "<button type=\"submit\" formaction=\"/x\">s</button>",
        "<button type=\"button\">b</button>",
        "<time datetime=\"2026-11-24\">d</time>",
        "<svg><script>alert(1)</script></svg>",
        "<!-- a comment -->",
        "<nav class=\"site-nav\"><span>n</span></nav>",
    );
    let (html, errors) = render_one(
        Slot::HeaderRow,
        "\"data-site-ok\", \"aria-label\", \"role\"",
        tmpl,
    );
    assert_eq!(errors, Vec::<String>::new());
    assert_eq!(
        html,
        concat!(
            // ammonia reorders the survivors; the set is what matters.
            "<p class=\"site-kept\" role=\"note\" data-site-ok=\"2\" aria-label=\"kept\">t</p>",
            "<a>j</a>",
            "<a>j2</a>",
            "<a>p</a>",
            "<a>b</a>",
            "<a href=\"/kb/k/ok\" data-site-ok=\"javascript:alert(1)\">ok</a>",
            "<img alt=\"i\">",
            "<button>s</button>",
            "<button type=\"button\">b</button>",
            "<time datetime=\"2026-11-24\">d</time>",
            "<nav class=\"site-nav\"><span>n</span></nav>",
        )
    );
}

#[test]
fn content_a_template_marks_safe_is_still_sanitised() {
    // Page data is content (D3): a careless `safe` must not let it through.
    let c = one(
        "after-body",
        "",
        "<p class=\"site-t\">{{ page.title | safe }}</p>",
    )
    .unwrap();
    let mut ctx = plain_ctx();
    ctx.page.title =
        "<img src=x onerror=alert(1)><script>alert(2)</script><b class=\"mae-x\">t</b>".into();
    let (html, errors) = c.render_slot(Slot::AfterBody, &ctx);
    assert_eq!(errors, vec![]);
    assert_eq!(html, "<p class=\"site-t\"><img src=\"x\">t</p>");
}

#[test]
fn the_document_head_admits_only_descriptive_meta() {
    let tmpl = concat!(
        "<meta name=\"description\" content=\"d\">",
        "<meta name=\"referrer\" content=\"unsafe-url\">",
        "<meta http-equiv=\"refresh\" content=\"0;url=https://evil.example\">",
        "<meta charset=\"x\">",
        "<title>t</title>",
        "<link rel=\"stylesheet\" href=\"https://evil.example/x.css\">",
        "<script>alert(1)</script>",
        "<base href=\"https://evil.example/\">",
        "<p class=\"site-x\">p</p>",
    );
    let (html, errors) = render_one(Slot::DocumentHead, "", tmpl);
    assert_eq!(errors, Vec::<String>::new());
    // Text is dropped too: in <head> it would end the head early.
    assert_eq!(html, "<meta name=\"description\" content=\"d\">");
}

#[test]
fn a_head_components_page_is_sanitised_as_a_body() {
    let manifest = r#"
[[component]]
name = "h"
slot = "document-head"
template = "h.html"
[component.data]
file = "h.json"
fields = { d = "string" }
[component.page]
name = "about"
title = "About"
template = "about.html"
"#;
    let c = load(
        manifest,
        &[
            (
                "h.html",
                "<meta name=\"description\" content=\"{{ data.d }}\"><p>x</p>",
            ),
            ("h.json", "{\"d\": \"A site\"}"),
            (
                "about.html",
                "<p class=\"site-about\">{{ data.d }}</p><meta name=\"robots\" content=\"x\">",
            ),
        ],
    )
    .unwrap();
    let (head, errors) = c.render_slot(Slot::DocumentHead, &plain_ctx());
    assert_eq!(errors, vec![]);
    assert_eq!(head, "<meta name=\"description\" content=\"A site\">");
    let s = site(&[], &[]);
    assert_eq!(
        c.render_page("about", &s).unwrap().unwrap(),
        "<p class=\"site-about\">A site</p>"
    );
}

// ---------------------------------------------------------------------------
// Confinement: every failure names its component and never panics.

#[test]
fn a_failing_component_contributes_nothing_and_its_neighbours_still_render() {
    let manifest = r#"
[[component]]
name = "first"
slot = "after-body"
template = "first.html"

[[component]]
name = "broken"
slot = "after-body"
template = "broken.html"

[[component]]
name = "last"
slot = "after-body"
template = "last.html"
"#;
    let c = load(
        manifest,
        &[
            ("first.html", "<p class=\"site-a\">first</p>"),
            (
                "broken.html",
                "<p class=\"site-b\">partial output</p>{{ fail(\"the broken component failed\") }}",
            ),
            ("last.html", "<p class=\"site-c\">last</p>"),
        ],
    )
    .unwrap();
    let (html, errors) = c.render_slot(Slot::AfterBody, &plain_ctx());
    assert_eq!(
        html,
        "<p class=\"site-a\">first</p><p class=\"site-c\">last</p>"
    );
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].component, "broken");
    assert!(
        errors[0].reason.contains("the broken component failed"),
        "{}",
        errors[0].reason
    );
}

/// The one error a render of `tmpl` gives, which must name the component.
fn render_error(tmpl: &str) -> String {
    let (html, errors) = render_one(Slot::PageFooter, "", tmpl);
    assert_eq!(html, "", "{tmpl}");
    assert_eq!(errors.len(), 1, "{tmpl}");
    assert!(errors[0].starts_with("component `c`: "), "{}", errors[0]);
    errors[0].clone()
}

#[test]
fn strict_undefined_fails_the_render() {
    for t in [
        "{{ page.nope }}",
        "{{ nope }}",
        "{% if page.nope %}x{% endif %}",
        "{% for x in nope %}{% endfor %}",
        // A page template's `page` and a slot component's `data` do not exist.
        "{{ data }}",
    ] {
        let e = render_error(t);
        assert!(e.contains("undefined"), "{t}: {e}");
    }
    // Asking is how a template tests: these render.
    let (html, errors) = render_one(
        Slot::PageFooter,
        "",
        "{% if page.nope is defined or \"X\" in page.properties %}x{% else %}<p class=\"site-no\">no</p>{% endif %}",
    );
    assert_eq!(
        (html.as_str(), errors.len()),
        ("<p class=\"site-no\">no</p>", 0)
    );
}

#[test]
fn a_render_out_of_fuel_fails() {
    // A million loop steps, writing nothing, so only fuel can stop it.
    let e = render_error(
        "{% for i in range(1000) %}{% for j in range(1000) %}{% endfor %}{% endfor %}",
    );
    assert!(e.contains("fuel"), "{e}");
}

#[test]
fn a_runaway_recursive_loop_fails() {
    let e = render_error("{% for x in [1] recursive %}{{ loop([1]) }}{% endfor %}");
    assert!(e.contains("recursion"), "{e}");
}

#[test]
fn a_range_over_the_cap_fails() {
    let e = render_error("{% for i in range(1001) %}{% endfor %}");
    assert!(e.contains("at most 1000"), "{e}");
}

#[test]
fn a_render_writing_too_much_fails() {
    let e = render_error("{% for i in range(1000) %}{{ \"x\" * 100 }}{% endfor %}");
    assert!(e.contains("65536"), "{e}");
}

#[test]
fn debug_is_removed_and_fail_fails() {
    let e = render_error("{{ debug() }}");
    assert!(e.contains("debug"), "{e}");
    let e = render_error("{{ fail(\"stop here\") }}");
    assert!(e.contains("stop here"), "{e}");
}

#[test]
fn a_template_cannot_include_extend_or_import_anything() {
    for t in [
        "{% include \"x\" %}",
        "{% include \"c.html\" %}",
        "{% extends \"c.html\" %}",
        "{% import \"c.html\" as m %}",
        "{% macro m() %}{% endmacro %}",
    ] {
        let errs = one("after-body", "", t).unwrap_err();
        assert_eq!(errs.len(), 1, "{t}");
        assert_eq!(
            (errs[0].component.as_deref(), errs[0].field.as_str()),
            (Some("c"), "template"),
            "{t}"
        );
        assert!(
            errs[0].reason.contains("does not compile"),
            "{t}: {}",
            errs[0]
        );
    }
}

// ---------------------------------------------------------------------------
// P-154: what a template sees.

/// The attribute `name`'s value in `html`, unescaped as a parser would.
fn attr(html: &str, name: &str) -> String {
    let start = html
        .find(&format!(" {name}=\""))
        .unwrap_or_else(|| panic!("{name} in {html}"))
        + name.len()
        + 3;
    let end = start + html[start..].find('"').unwrap();
    html[start..end]
        .replace("&quot;", "\"")
        .replace("&nbsp;", "\u{a0}")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[test]
fn a_template_sees_every_field_of_the_page_and_the_site() {
    let tmpl = concat!(
        "<span class=\"site-echo\" data-id=\"{{ page.id }}\" data-title=\"{{ page.title }}\"",
        " data-url=\"{{ page.url }}\" data-tags=\"{{ page.tags | join('|') }}\"",
        " data-props=\"{% for k, v in page.properties | items %}{{ k }}={{ v }};{% endfor %}\"",
        " data-dates=\"{% for k, v in page.dates | items %}{{ k }}={{ v }};{% endfor %}\"",
        " data-locale=\"{{ site.locale }}\" data-base=\"{{ site.base_path }}\"",
        " data-kb=\"{{ site.kb_id }}\"",
        " data-pages=\"{% for k, v in site.pages | items %}{{ k }}={{ v }};{% endfor %}\"",
        " data-keys=\"{{ page | list | sort | join(',') }}/{{ site | list | sort | join(',') }}\"",
        "></span>",
    );
    let declared = "\"data-id\", \"data-title\", \"data-url\", \"data-tags\", \"data-props\", \
                    \"data-dates\", \"data-locale\", \"data-base\", \"data-kb\", \"data-pages\", \
                    \"data-keys\"";
    let c = one("before-body", declared, tmpl).unwrap();
    let s = Arc::new(SiteContext {
        locale: "fr-FR".into(),
        base_path: "/kb".into(),
        kb_id: "kb-7".into(),
        pages: BTreeMap::from([("changes".into(), "/kb/kb-7/site/changes".into())]),
        published: Arc::new(BTreeMap::new()),
    });
    // Values a careless template would mangle or that try to break out.
    let ctx = PageContext {
        page: PageData {
            id: "concept:a/b".into(),
            title: "A \"quoted\" <b>title</b> & 'é'".into(),
            url: "/kb/kb-7/concept%3Aa%2Fb".into(),
            properties: BTreeMap::from([
                ("STATE".into(), "x\" onmouseover=\"alert(1)".into()),
                ("UPDATED".into(), "2026-11-02".into()),
            ]),
            dates: BTreeMap::from([
                ("created".into(), "2026-01-01".into()),
                ("updated".into(), "2026-11-02".into()),
            ]),
            tags: vec!["alpha".into(), "β".into()],
        },
        site: s,
    };
    let (html, errors) = c.render_slot(Slot::BeforeBody, &ctx);
    assert_eq!(errors, vec![]);
    assert!(!html.contains("onmouseover=\"alert"), "{html}");
    let want = [
        ("data-id", "concept:a/b"),
        ("data-title", "A \"quoted\" <b>title</b> & 'é'"),
        ("data-url", "/kb/kb-7/concept%3Aa%2Fb"),
        ("data-tags", "alpha|β"),
        (
            "data-props",
            "STATE=x\" onmouseover=\"alert(1);UPDATED=2026-11-02;",
        ),
        ("data-dates", "created=2026-01-01;updated=2026-11-02;"),
        ("data-locale", "fr-FR"),
        ("data-base", "/kb"),
        ("data-kb", "kb-7"),
        ("data-pages", "changes=/kb/kb-7/site/changes;"),
        // Exactly these fields, and `published` is not a field of `site`.
        (
            "data-keys",
            "dates,id,properties,tags,title,url/base_path,kb_id,locale,pages",
        ),
    ];
    for (name, value) in want {
        assert_eq!(attr(&html, name), value, "{name}");
    }
}

#[test]
fn published_answers_only_for_nodes_in_this_view() {
    let tmpl = concat!(
        "{% for id in [\"in-view\", \"not-in-view\", \"\", 7] %}",
        "<p class=\"site-p\" data-site-r=\"{{ published(id) }}\"></p>",
        "{% endfor %}",
    );
    let c = one("header-row", "\"data-site-r\"", tmpl).unwrap();
    let s = site(&[("in-view", "/kb/demo/in-view")], &[]);
    let (html, errors) = c.render_slot(Slot::HeaderRow, &page("x", &[], &s));
    assert_eq!(errors, vec![]);
    assert_eq!(
        html,
        concat!(
            "<p class=\"site-p\" data-site-r=\"/kb/demo/in-view\"></p>",
            "<p class=\"site-p\" data-site-r=\"None\"></p>",
            "<p class=\"site-p\" data-site-r=\"None\"></p>",
            "<p class=\"site-p\" data-site-r=\"None\"></p>",
        )
    );
    // It answers one ID at a time: the set itself cannot be listed.
    for t in [
        "{% for x in published %}{{ x }}{% endfor %}",
        "{{ published | list }}",
        "{{ published.keys() }}",
    ] {
        render_error(t);
    }
    assert!(render_error("{{ published() }}").contains("one node ID"));
    assert!(render_error("{{ published(\"a\", \"b\") }}").contains("one node ID"));
}

// ---------------------------------------------------------------------------
// iso_date

#[test]
fn iso_date_reads_real_days_only() {
    for ok in [
        "2026-11-24",
        "2024-02-29",
        "2000-02-29",
        "0001-01-01",
        "9999-12-31",
    ] {
        assert_eq!(iso_date(ok).map(|d| d.to_string()).as_deref(), Some(ok));
    }
    for bad in [
        "",
        "2026-11-2",
        "2026-11-240",
        "2026/11/24",
        "2026-13-01",
        "2026-00-10",
        "2026-11-00",
        "2026-11-31",
        "2023-02-29",
        "1900-02-29",
        "0000-01-01",
        "+026-11-24",
        "2026-1a-24",
        " 2026-11-24",
        "2026-11-24T10:00",
        "[2026-11-24 Tue]",
        "２０２６-11-24",
    ] {
        assert_eq!(iso_date(bad), None, "{bad:?}");
    }
    // In a template: a non-string is none, not an error.
    let (html, errors) = render_one(
        Slot::PageFooter,
        "\"data-site-d\"",
        "<p class=\"site-d\" data-site-d=\"{{ iso_date(42) }}|{{ iso_date(none) }}|{{ iso_date('2024-02-29').iso }}|{{ iso_date('2024-02-29').month }}\"></p>",
    );
    assert_eq!(errors, Vec::<String>::new());
    assert_eq!(attr(&html, "data-site-d"), "None|None|2024-02-29|2");
}

proptest! {
    /// Any string: no panic, and a date it reads displays as the input.
    #[test]
    fn iso_date_never_panics_and_round_trips(s in "\\PC{0,12}|[0-9]{4}-[0-9]{2}-[0-9]{2}") {
        if let Some(d) = iso_date(&s) {
            prop_assert_eq!(d.to_string(), s);
            prop_assert!(d.year >= 1 && (1..=12).contains(&d.month) && (1..=31).contains(&d.day));
        }
    }

    /// Every real day reads; the day after the month's last does not.
    #[test]
    fn iso_date_agrees_with_the_calendar(y in 1u16..=9999, m in 1u8..=12) {
        let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
        let last = match m { 2 if leap => 29, 2 => 28, 4 | 6 | 9 | 11 => 30, _ => 31 };
        for d in 1..=last {
            let s = format!("{y:04}-{m:02}-{d:02}");
            prop_assert!(iso_date(&s).is_some(), "{}", s);
        }
        let s = format!("{y:04}-{m:02}-{:02}", last + 1);
        prop_assert!(iso_date(&s).is_none(), "{}", s);
    }
}

// ---------------------------------------------------------------------------
// The fuel limit is a decision: pin why it is enough.

#[test]
fn the_largest_legal_site_changes_page_fits_well_within_the_fuel_limit() {
    let manifest = std::fs::read_to_string(fixture().join("components.toml")).unwrap();
    let mut files = fixture_files();
    let versions: Vec<String> = (0..MAX_DATA_ITEMS)
        .map(|i| {
            format!(
                "{{\"version\": \"{i}.0\", \"date\": \"{:04}-01-01\", \"summary\": \"Version {i}, with a summary of some length.\"}}",
                1000 + i
            )
        })
        .collect();
    files.insert("versions.json".into(), format!("[{}]", versions.join(",")));
    let c = Components::load(&manifest, &files).unwrap();
    let s = fixture_site(&[("release-notes", "/kb/demo/release-notes")]);
    let used = c.fuel_used("versions", None, &s).unwrap();
    eprintln!(
        "fuel: the site-changes page over {MAX_DATA_ITEMS} versions spent {used} of {FUEL_LIMIT}"
    );
    assert!(used > 0, "fuel is not being counted");
    assert!(
        used < FUEL_LIMIT / 2,
        "the changes page spent {used} of {FUEL_LIMIT}"
    );
    // It renders, too (and within the output cap).
    assert!(c.render_page("changes", &s).unwrap().is_ok());
    // Each example's slot template, on a page with every property.
    let ctx = page("g", &[("STATE", "draft"), ("UPDATED", "2026-01-01")], &s);
    for name in [
        "state-badge",
        "copy-link",
        "whats-new",
        "versions",
        "last-updated",
    ] {
        let used = c.fuel_used(name, Some(&ctx), &s).unwrap();
        assert!(used < FUEL_LIMIT / 2, "{name} spent {used}");
    }
}

// ---------------------------------------------------------------------------
// Load errors: every kind, all at once.

/// One component per kind of load error (and two for duplicates).
const BROKEN_MANIFEST: &str = r#"
mystery = 1

[[component]]
name = "bad-slot"
slot = "sidebar"
template = "ok.html"

[[component]]
name = "twin"
slot = "header-row"
template = "ok.html"

[[component]]
name = "twin"
slot = "page-footer"
template = "ok.html"

[[component]]
name = "attrs"
slot = "header-row"
template = "ok.html"
attributes = ["onclick", "style", "srcdoc", "data-mae-x", "aria-owns", "data-ok", "href"]

[[component]]
name = "missing"
slot = "header-row"
template = "missing.html"
script = "missing.js"
stylesheet = "missing.css"

[[component]]
name = "escape"
slot = "header-row"
template = "../ok.html"

[[component]]
name = "Bad_Name"
slot = "header-row"
template = "ok.html"

[[component]]
name = "typo"
slot = "header-row"
templat = "ok.html"

[[component]]
name = "head-attrs"
slot = "document-head"
template = "ok.html"
attributes = ["data-x"]

[[component]]
name = "bad-page"
[component.page]
name = "a/b"
title = "T"
template = "ok.html"
[component.data]
file = "ok.json"
fields = { n = "integer" }

[[component]]
name = "page-no-data"
[component.page]
name = "solo"
title = "T"
template = "ok.html"

[[component]]
name = "page-one"
[component.page]
name = "Changes"
title = "T"
template = "ok.html"
[component.data]
file = "ok.json"
fields = { n = "integer" }

[[component]]
name = "page-two"
[component.page]
name = "changes"
title = "T"
template = "ok.html"
[component.data]
file = "ok.json"
fields = { n = "integer" }

[[component]]
name = "bad-data"
slot = "header-row"
template = "ok.html"
[component.data]
file = "bad.json"
list = true
fields = { version = "string", date = "date", Bad = "string", n = "float" }

[[component]]
name = "bad-entries"
slot = "header-row"
template = "ok.html"
[component.data]
file = "entries.json"
list = true
fields = { version = "string", date = "date", node = "id?" }

[[component]]
name = "no-compile"
slot = "header-row"
template = "broken.html"

[[component]]
name = "neither"
"#;

/// Every error BROKEN_MANIFEST must give, as (component, field).
const BROKEN_ERRORS: &[(Option<&str>, &str)] = &[
    (None, "mystery"),
    (Some("bad-slot"), "slot"),
    (Some("attrs"), "attributes"), // onclick
    (Some("attrs"), "attributes"), // style
    (Some("attrs"), "attributes"), // srcdoc
    (Some("attrs"), "attributes"), // data-mae-x
    (Some("attrs"), "attributes"), // aria-owns
    (Some("attrs"), "attributes"), // href
    (Some("missing"), "template"),
    (Some("missing"), "script"),
    (Some("missing"), "stylesheet"),
    (Some("escape"), "template"),
    (Some("Bad_Name"), "name"),
    (Some("typo"), "template"), // required for a slot
    (Some("typo"), "templat"),  // unknown
    (Some("head-attrs"), "attributes"),
    (Some("bad-page"), "page.name"),
    (Some("page-no-data"), "page"),
    (Some("bad-data"), "data.fields"),  // Bad
    (Some("bad-data"), "data.fields"),  // float
    (Some("bad-entries"), "data.file"), // [0].date
    (Some("bad-entries"), "data.file"), // [1].version
    (Some("bad-entries"), "data.file"), // [1].node
    (Some("bad-entries"), "data.file"), // [2].extra
    (Some("bad-entries"), "data.file"), // [2].version
    (Some("bad-entries"), "data.file"), // [3]
    (Some("neither"), "slot"),
    (Some("twin"), "name"),
    (Some("page-two"), "page.name"),
    (Some("no-compile"), "template"),
];

/// Words some of those errors must say.
const BROKEN_SAYS: &[&str] = &[
    "`sidebar` is not one of document-head,",
    "`onclick` is not a data- key",
    "`data-mae-x` is in the chrome's",
    "`aria-owns` names other elements by id",
    "`missing.html` is not in the component directory",
    "`../ok.html` is not one plain file name",
    "`a/b` is not one URL segment",
    "`changes` is the same address as component `page-one`'s page",
    "`entries.json`[0].date: is not an ISO date",
    "`entries.json`[2].extra: is not a declared field",
    "`entries.json`[3]: is not a JSON object",
    "`broken.html` does not compile",
];

#[test]
fn every_load_error_is_reported_at_once_naming_component_and_field() {
    let manifest = BROKEN_MANIFEST;
    let errs = load(
        manifest,
        &[
            ("ok.html", "<p class=\"site-ok\">ok</p>"),
            ("ok.json", "{\"n\": 1}"),
            ("bad.json", "[]"),
            (
                "entries.json",
                r#"[{"version": "1", "date": "2026-02-30"},
                    {"version": 2, "date": "2026-01-01", "node": ".."},
                    {"date": "2026-01-01", "extra": true},
                    "not a record"]"#,
            ),
            ("broken.html", "{% if %}"),
        ],
    )
    .unwrap_err();
    let got: Vec<(Option<&str>, &str)> = errs
        .iter()
        .map(|e| (e.component.as_deref(), e.field.as_str()))
        .collect();
    let want = BROKEN_ERRORS;
    let mut got_sorted = got.clone();
    got_sorted.sort();
    let mut want_sorted = want.to_vec();
    want_sorted.sort();
    assert_eq!(
        got_sorted,
        want_sorted,
        "{}",
        errs.iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );
    let text: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
    let has = |s: &str| text.iter().any(|t| t.contains(s));
    for s in BROKEN_SAYS {
        assert!(has(s), "no error says {s:?}:\n{}", text.join("\n"));
    }
}

#[test]
fn a_manifest_that_is_not_toml_is_one_error() {
    let errs = load("[[component]\nname=", &[]).unwrap_err();
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].component, None);
}

#[test]
fn a_data_list_over_the_cap_is_refused() {
    let manifest = r#"
[[component]]
name = "v"
slot = "page-footer"
template = "v.html"
[component.data]
file = "v.json"
list = true
fields = { n = "integer" }
"#;
    let ok = format!("[{}]", vec!["{\"n\": 1}"; MAX_DATA_ITEMS].join(","));
    assert!(load(manifest, &[("v.html", ""), ("v.json", &ok)]).is_ok());
    let over = format!("[{}]", vec!["{\"n\": 1}"; MAX_DATA_ITEMS + 1].join(","));
    let errs = load(manifest, &[("v.html", ""), ("v.json", &over)]).unwrap_err();
    assert_eq!(errs.len(), 1);
    assert!(errs[0].reason.contains("at most 500"), "{}", errs[0]);
}

#[test]
fn optional_fields_absent_from_the_data_are_none_to_a_template() {
    let manifest = r#"
[[component]]
name = "v"
slot = "page-footer"
template = "v.html"
attributes = ["data-site-s"]
[component.data]
file = "v.json"
fields = { s = "string?", t = "string" }
"#;
    let c = load(
        manifest,
        &[
            (
                "v.html",
                "<p class=\"site-v\" data-site-s=\"{{ data.s }}\">{{ data.t }}</p>",
            ),
            ("v.json", "{\"t\": \"x\"}"),
        ],
    )
    .unwrap();
    let (html, errors) = c.render_slot(Slot::PageFooter, &plain_ctx());
    assert_eq!(errors, vec![]);
    assert_eq!(html, "<p class=\"site-v\" data-site-s=\"None\">x</p>");
}

// ---------------------------------------------------------------------------
// What a loader reads, and what the router must agree with.

/// The loader reads exactly the files a manifest names (H1: a manifest, not
/// a naming convention); a name that could leave the directory is not read,
/// and the load reports it.
#[test]
fn a_loader_reads_exactly_the_files_the_manifest_names() {
    let manifest = std::fs::read_to_string(fixture().join("components.toml")).unwrap();
    let named: Vec<String> = Components::named_files(&manifest).into_iter().collect();
    assert_eq!(
        named,
        [
            "components.css",
            "copy-link.html",
            "copy-link.js",
            "last-updated.html",
            "site-changes.html",
            "state-badge.html",
            "version-footer.html",
            "versions.json",
            "whats-new.html",
            "whats-new.json",
        ]
    );
    let hostile = "[[component]]\nname = \"x\"\nslot = \"header-row\"\n\
                   template = \"../etc/passwd.html\"\nscript = \"/abs.js\"\n\
                   stylesheet = \".hidden.css\"\n[component.data]\nfile = \"a/b.json\"\n";
    assert!(Components::named_files(hostile).is_empty());
    let errors = load(hostile, &[]).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.field == "template" && e.reason.contains("plain file name")),
        "{errors:?}"
    );
    assert!(Components::named_files("not toml [").is_empty());
}

/// The router swaps the document head's `<meta name>`s with the page; it
/// must name exactly the ones the head's sanitiser admits, or a slot's
/// description outlives its page (or the chrome's viewport is dropped).
#[test]
fn the_router_swaps_exactly_the_head_metas_the_sanitiser_admits() {
    let router = mae_site::assets::ROUTER_JS;
    let list = router
        .split("HEAD_META_NAMES = [")
        .nth(1)
        .and_then(|r| r.split(']').next())
        .expect("the router's HEAD_META_NAMES");
    let names: Vec<&str> = list
        .split(',')
        .map(|n| n.trim().trim_matches('"'))
        .filter(|n| !n.is_empty())
        .collect();
    assert_eq!(names, mae_site::components::HEAD_META_NAMES);
}

/// P-154: a derived page carries its file-level properties, `ID` left out,
/// under mae-org's canonical keys (`:state:` and `:State:` are `STATE`),
/// with hidden references removed (P-13), and its dates from `CREATED` and
/// `UPDATED`, as written. A headline's properties are not the page's.
#[test]
fn a_derived_page_carries_its_file_properties_and_dates() {
    let text =
        ":PROPERTIES:\n:ID: p\n:state: Draft\n:Updated: 2026-11-02\n:CREATED: <2026-01-05 Mon>\n\
                :Owner: see internal:secret now\n:END:\n#+title: P\n\n* H\n:PROPERTIES:\n:ID: h\n\
                :STATE: approved\n:END:\nBody.\n";
    let opts = mae_site::SiteOptions {
        hidden_namespaces: vec!["internal".into()],
        ..Default::default()
    };
    let site = mae_site::derive_with(
        &[mae_site::SourcePage {
            origin: "p.org".into(),
            text: text.into(),
        }],
        &Addressing::live("demo").unwrap(),
        &opts,
    );
    let p = &site.pages["p"];
    let props: Vec<(&str, &str)> = p
        .properties
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(
        props,
        [
            ("CREATED", "<2026-01-05 Mon>"),
            ("OWNER", "see  now"),
            ("STATE", "Draft"),
            ("UPDATED", "2026-11-02"),
        ]
    );
    let dates: Vec<(&str, &str)> = p
        .dates
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(
        dates,
        [("created", "<2026-01-05 Mon>"), ("updated", "2026-11-02")]
    );
}
