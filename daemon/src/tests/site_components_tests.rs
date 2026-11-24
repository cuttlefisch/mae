//! Slot components in the live site (ADR-119 D18, Amendment 4 H1/H2): the
//! operator directory loaded at startup, the slots in the served shell, the
//! component page under `/kb/<kb-id>/site/<name>`, and the component assets.
//! Against a real `DocStore` and the example components in
//! docs/verification/fixtures/components/slots.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use http_body_util::BodyExt;
use hyper::body::Bytes;
use hyper::{Response, StatusCode};
use mae_daemon::doc_store::DocStore;
use mae_daemon::site::{self, ComponentsFor, SitePage, View};
use mae_daemon::site_components::{KbComponents, SiteComponents};
use mae_mcp::identity::Identity;
use mae_sync::kb::{KbCollectionDoc, Role, TransportPolicy};

use super::kb_query_tests::fresh_doc_store;

const BY: mae_daemon::doc_editors::Attribution<'static> =
    mae_daemon::doc_editors::Attribution::Principal("test-editor");
const READER: &str = "oauth:reader@example.com";
const STRANGER: &str = "oauth:stranger@example.com";
const KB: &str = "kb-comp";
const LIMITS: site::SiteLimits = site::SiteLimits {
    max_scan_nodes: 500,
    max_body_bytes: 65_536,
};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/verification/fixtures/components/slots")
}

/// A component directory written for one test: the manifest and its files.
fn dir_with(manifest: &str, files: &[(&str, &str)]) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("components.toml"), manifest).unwrap();
    for (name, text) in files {
        std::fs::write(d.path().join(name), text).unwrap();
    }
    d
}

fn load(dirs: &[(&str, &Path)]) -> Result<SiteComponents, Vec<String>> {
    let map: BTreeMap<String, PathBuf> = dirs
        .iter()
        .map(|(k, p)| (k.to_string(), p.to_path_buf()))
        .collect();
    SiteComponents::load(&map)
}

/// A plaintext KB, `READER` a viewer, from Org files as `kb seed` reads them.
async fn seed_files(store: &DocStore, kb_id: &str, files: &[String]) {
    let owner = Arc::new(Identity::generate("owner"));
    store.set_signer(Arc::clone(&owner));
    let mut coll = KbCollectionDoc::new_owned(kb_id, &owner.fingerprint(), "owner");
    coll.set_transport_policy(TransportPolicy::Hub);
    coll.upsert_member(READER, "reader", Role::Viewer);
    let nodes: Vec<_> = files
        .iter()
        .map(|t| {
            let f = mae_daemon::seed::split(t).expect("a fixture file splits");
            mae_daemon::seed::node_from_file(&f, 0)
        })
        .collect();
    for n in &nodes {
        let _ = coll.add_node(&n.id(), &n.title());
    }
    store
        .share_doc(&format!("kbc:{kb_id}"), &coll.encode_state(), BY)
        .await
        .unwrap();
    for n in &nodes {
        let name = mae_sync::kb_node_doc_name(kb_id, &n.id());
        store.share_doc(&name, &n.encode_state(), BY).await.unwrap();
    }
}

fn org(id: &str, props: &str, title: &str, body: &str) -> String {
    format!(":PROPERTIES:\n:ID: {id}\n{props}:END:\n#+title: {title}\n\n{body}\n")
}

/// The fixture pages: a draft with a date, a release-notes page the
/// what's-new link points at, and a page with a section heading.
async fn fixture_kb() -> Arc<DocStore> {
    let store = fresh_doc_store().await;
    let files = [
        org(
            "guide",
            ":STATE: Draft\n:UPDATED: 2026-11-02\n",
            "Guide",
            "Intro.\n\n* Section one\nText.",
        ),
        org("release-notes", "", "Release notes", "Notes."),
        org("plain", "", "Plain", "Nothing else."),
    ];
    seed_files(&store, KB, &files).await;
    store
}

fn components_for(c: Option<Arc<KbComponents>>) -> Option<ComponentsFor> {
    c.map(|components| ComponentsFor {
        components,
        locale: "en-US".into(),
    })
}

async fn view(store: &DocStore, v: View, who: &str, c: Option<Arc<KbComponents>>) -> SitePage {
    site::view(
        store,
        KB,
        &v,
        Some(who),
        LIMITS,
        &Default::default(),
        components_for(c),
    )
    .await
}

async fn body(r: Response<http_body_util::Full<Bytes>>) -> (StatusCode, String, String) {
    let status = r.status();
    let headers = format!("{:?}", r.headers());
    let b = r.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8(b.to_vec()).unwrap())
}

/// The served page for `page`, as `site_answer` writes it.
async fn served(page: SitePage) -> (StatusCode, String) {
    let (s, _, b) = body(crate::oauth::site_answer(page, "en-US".into())).await;
    (s, b)
}

// ---------------------------------------------------------------------------
// H1: the directory, loaded at startup.

#[test]
fn the_fixture_directory_loads() {
    let c = load(&[(KB, &fixture_dir())]).expect("the examples load");
    assert!(c.for_kb(KB).is_some());
    assert!(c.for_kb("other").is_none());
}

#[test]
fn a_missing_directory_refuses_startup_naming_the_kb() {
    let gone = tempfile::tempdir().unwrap().path().join("nope");
    let errs = load(&[(KB, &gone)]).unwrap_err();
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains(&format!("KB `{KB}`")), "{errs:?}");
    assert!(errs[0].contains("does not exist"), "{errs:?}");
    // A directory without its manifest is as missing.
    let empty = tempfile::tempdir().unwrap();
    let errs = load(&[(KB, empty.path())]).unwrap_err();
    assert!(
        errs[0].contains("components.toml is not in the directory"),
        "{errs:?}"
    );
}

#[test]
fn a_broken_manifest_refuses_startup_naming_the_kb_and_the_reason() {
    let d = dir_with("[[component]\nname = \"x\"", &[]);
    let errs = load(&[(KB, d.path())]).unwrap_err();
    assert!(
        errs.iter().all(|e| e.contains(&format!("KB `{KB}`"))),
        "{errs:?}"
    );
    assert!(
        errs.iter().any(|e| e.contains("components.toml")),
        "{errs:?}"
    );
}

#[test]
fn a_template_that_does_not_compile_refuses_startup_naming_the_component() {
    let d = dir_with(
        "[[component]]\nname = \"badge\"\nslot = \"header-row\"\ntemplate = \"b.html\"\n",
        &[("b.html", "{% if %}")],
    );
    let errs = load(&[(KB, d.path())]).unwrap_err();
    assert_eq!(errs.len(), 1, "{errs:?}");
    let e = &errs[0];
    assert!(e.contains(&format!("KB `{KB}`")), "{e}");
    assert!(e.contains("component `badge`"), "{e}");
    assert!(e.contains("does not compile"), "{e}");
}

#[test]
fn an_undeclared_data_key_refuses_startup_naming_the_component() {
    let d = dir_with(
        "[[component]]\nname = \"news\"\nslot = \"header-row\"\ntemplate = \"n.html\"\n\
         [component.data]\nfile = \"n.json\"\nfields = { label = \"string\" }\n",
        &[
            ("n.html", "{{ data.label }}"),
            ("n.json", r#"{ "label": "x", "extra": 1 }"#),
        ],
    );
    let errs = load(&[(KB, d.path())]).unwrap_err();
    assert!(
        errs.iter().any(|e| e.contains("component `news`")
            && e.contains("data.file")
            && e.contains("extra")),
        "{errs:?}"
    );
}

/// Every problem in every KB at once, and a good KB beside a bad one still
/// refuses: no partial site.
#[test]
fn every_kbs_problems_are_reported_at_once() {
    let gone = tempfile::tempdir().unwrap().path().join("nope");
    let bad = dir_with("x = 1\n", &[]);
    let errs = load(&[
        ("kb-a", &gone),
        ("kb-b", bad.path()),
        ("kb-good", &fixture_dir()),
        ("Not A KB", &fixture_dir()),
    ])
    .unwrap_err();
    for kb in ["kb-a", "kb-b", "Not A KB"] {
        assert!(
            errs.iter().any(|e| e.contains(&format!("KB `{kb}`"))),
            "{kb}: {errs:?}"
        );
    }
    assert!(!errs.iter().any(|e| e.contains("kb-good")), "{errs:?}");
}

// ---------------------------------------------------------------------------
// The shell's slots (P-153, P-158).

/// One component per slot, each writing a marker, so each slot's position in
/// the served page can be checked.
fn every_slot() -> (tempfile::TempDir, Arc<KbComponents>) {
    let mut manifest = String::new();
    let mut files = Vec::new();
    for slot in mae_site::components::Slot::ALL {
        let n = slot.name();
        manifest.push_str(&format!(
            "[[component]]\nname = \"{n}\"\nslot = \"{n}\"\ntemplate = \"{n}.html\"\n"
        ));
        let tmpl = if n == "document-head" {
            "<meta name=\"description\" content=\"SLOT-document-head\">".to_string()
        } else {
            format!("<p class=\"site-m\">SLOT-{n}</p>")
        };
        files.push((format!("{n}.html"), tmpl));
    }
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let d = dir_with(&manifest, &refs);
    let c = load(&[(KB, d.path())]).unwrap().for_kb(KB).unwrap();
    (d, c)
}

fn at(html: &str, needle: &str) -> usize {
    html.find(needle)
        .unwrap_or_else(|| panic!("no {needle:?} in {html}"))
}

#[tokio::test]
async fn each_slot_is_written_at_its_position() {
    let store = fixture_kb().await;
    let (_d, c) = every_slot();
    let page = view(&store, View::Page("guide".into()), READER, Some(c)).await;
    let (status, html) = served(page).await;
    assert_eq!(status, StatusCode::OK);
    let m = |n: &str| at(&html, &format!("SLOT-{n}"));
    // Head: after the router and before the title.
    assert!(at(&html, "router-") < m("document-head"));
    assert!(m("document-head") < at(&html, "<title>"));
    // Header row: inside the header, after the built-in controls.
    assert!(at(&html, "id=\"mae-palette-button\"") < m("header-row"));
    assert!(m("header-row") < at(&html, "</header>"));
    // Before the body: after the title, before the article's text; after the
    // body: the article's end.
    assert!(at(&html, "</h1>") < m("before-body"));
    assert!(m("before-body") < at(&html, "Intro."));
    assert!(at(&html, "Text.") < m("after-body"));
    assert!(m("after-body") < at(&html, "</article>"));
    // Left-panel footer: after the explorer, before main, in its own box.
    assert!(at(&html, "id=\"mae-explorer\"") < at(&html, "<div id=\"mae-left-footer\">"));
    assert!(m("left-panel-footer") < at(&html, "<main"));
    // Right panel: inside the aside (which the page's TOC alone would not
    // make: one heading). Page footer: last, in its own box.
    assert!(at(&html, "<aside id=\"mae-panels\">") < m("right-panel"));
    assert!(m("right-panel") < at(&html, "</aside>"));
    assert!(at(&html, "</aside>") < at(&html, "<footer id=\"mae-footer\">"));
    assert!(m("page-footer") < at(&html, "</body>"));
    // The page data a behaviour's ctx reads, on main.
    assert!(
        html.contains(&format!(
            "<main id=\"mae-main\" data-mae-kb=\"{KB}\" data-mae-base=\"/kb/{KB}\" \
         data-mae-url=\"/kb/{KB}/guide\" data-mae-page=\"guide\">"
        )),
        "{html}"
    );
}

/// P-158: empty slots add no element, and the page data marks only a site
/// with components.
#[tokio::test]
async fn empty_slots_write_nothing() {
    let store = fixture_kb().await;
    let fixture = load(&[(KB, &fixture_dir())]).unwrap().for_kb(KB).unwrap();
    let (_, html) =
        served(view(&store, View::Page("plain".into()), READER, Some(fixture)).await).await;
    // The fixture fills no page footer, right panel or head slot, and the
    // plain page has neither a state nor a date.
    for absent in [
        "<footer",
        "<aside",
        "site-state-badge",
        "site-last-updated",
        "<meta name=\"description\"",
    ] {
        assert!(!html.contains(absent), "{absent}: {html}");
    }
    let (_, bare) = served(view(&store, View::Page("plain".into()), READER, None).await).await;
    for absent in [
        "data-mae-",
        "mae-left-footer",
        "mae-footer",
        "class=\"site-",
        "/site-assets/c-",
    ] {
        assert!(!bare.contains(absent), "{absent}: {bare}");
    }
}

/// The examples in a served page, from a seeded node's own properties: the
/// state badge and the last-updated line (P-161, P-163), the copy link as a
/// plain link to the page (P-162, A2), the what's-new link to a published
/// node and the version footer (P-163). Their assets are linked: stylesheets
/// after the base stylesheet, behaviours after the router.
#[tokio::test]
async fn the_examples_render_in_a_served_page() {
    let store = fixture_kb().await;
    let set = load(&[(KB, &fixture_dir())]).unwrap();
    let c = set.for_kb(KB).unwrap();
    let (_, html) = served(view(&store, View::Page("guide".into()), READER, Some(c)).await).await;
    for want in [
        "<p class=\"site-state-badge site-state-draft\" data-site-state=\"draft\">Draft</p>",
        "<time datetime=\"2026-11-02\">2026-11-02</time>",
        &format!(
            "<a class=\"site-copy-link-anchor\" href=\"/kb/{KB}/guide\">Link to this page</a>"
        ),
        &format!("<a class=\"site-whats-new\" href=\"/kb/{KB}/release-notes\">What's new</a>"),
        &format!("<a href=\"/kb/{KB}/site/changes\">Version 2.0</a>"),
    ] {
        assert!(html.contains(want), "{want}: {html}");
    }
    // One stylesheet (three components share it) and one behaviour.
    let css = html
        .matches("<link rel=\"stylesheet\" href=\"/site-assets/c-")
        .count();
    let js = html
        .matches("<script type=\"module\" src=\"/site-assets/c-")
        .count();
    assert_eq!((css, js), (1, 1), "{html}");
    assert!(at(&html, "/site-assets/base-") < at(&html, "/site-assets/c-"));
    let router = at(&html, "/site-assets/router-");
    let script = at(&html, "<script type=\"module\" src=\"/site-assets/c-");
    assert!(router < script, "the router runs before any behaviour");
    assert!(script < at(&html, "<title>"));
}

/// D18: a component that fails while rendering leaves its slot without its
/// output; the page and its neighbours still render.
#[tokio::test]
async fn a_failing_component_leaves_its_slot_empty_and_the_page_renders() {
    let store = fixture_kb().await;
    let d = dir_with(
        "[[component]]\nname = \"boom\"\nslot = \"header-row\"\ntemplate = \"boom.html\"\n\
         [[component]]\nname = \"ok\"\nslot = \"header-row\"\ntemplate = \"ok.html\"\n\
         [[component]]\nname = \"alone\"\nslot = \"page-footer\"\ntemplate = \"alone.html\"\n",
        &[
            (
                "boom.html",
                "<p class=\"site-b\">PARTIAL</p>{{ fail(\"no\") }}",
            ),
            ("ok.html", "<p class=\"site-ok\">FINE</p>"),
            (
                "alone.html",
                "<p class=\"site-a\">BEFORE</p>{{ page.missing }}",
            ),
        ],
    );
    let c = load(&[(KB, d.path())]).unwrap().for_kb(KB).unwrap();
    let (status, html) =
        served(view(&store, View::Page("guide".into()), READER, Some(c)).await).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("FINE"), "{html}");
    assert!(html.contains("Intro."), "{html}");
    assert!(
        !html.contains("PARTIAL") && !html.contains("BEFORE"),
        "{html}"
    );
    // Its slot alone, the footer, is absent rather than empty (P-158).
    assert!(!html.contains("<footer"), "{html}");
}

/// P-155: a component's output is sanitised where it lands in the served
/// page, not only in the engine's own tests.
#[tokio::test]
async fn hostile_component_output_is_inert_in_the_served_page() {
    let store = fixture_kb().await;
    let hostile = "<script>alert(1)</script><img src=x onerror=alert(2)>\
         <a href=\"javascript:alert(3)\" class=\"mae-explorer site-ok\" id=\"mae-main\">x</a>\
         <style>body{}</style><iframe src=\"/\"></iframe><p onclick=\"alert(4)\" style=\"x\">y</p>";
    let mut manifest = String::new();
    let mut files = Vec::new();
    for slot in [
        "header-row",
        "before-body",
        "left-panel-footer",
        "page-footer",
    ] {
        manifest.push_str(&format!(
            "[[component]]\nname = \"h-{slot}\"\nslot = \"{slot}\"\ntemplate = \"{slot}.html\"\n"
        ));
        files.push((
            format!("{slot}.html"),
            format!("{{{{ \"{0}\" | safe }}}}{0}", hostile.replace('"', "\\\"")),
        ));
    }
    manifest.push_str(
        "[[component]]\nname = \"head\"\nslot = \"document-head\"\ntemplate = \"head.html\"\n",
    );
    files.push(("head.html".into(), "<meta http-equiv=\"refresh\" content=\"0;url=//evil\"><script>alert(5)</script><meta name=\"robots\" content=\"noindex\">".into()));
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let d = dir_with(&manifest, &refs);
    let c = load(&[(KB, d.path())]).unwrap().for_kb(KB).unwrap();
    let (_, html) = served(view(&store, View::Page("guide".into()), READER, Some(c)).await).await;
    for bad in [
        "<script>",
        "onerror",
        "javascript:",
        "onclick",
        "<style",
        "<iframe",
        "http-equiv",
        "alert(",
        "style=\"x\"",
    ] {
        assert!(!html.contains(bad), "{bad} survived: {html}");
    }
    // One main landmark and one explorer: a component wears neither id nor class.
    assert_eq!(html.matches("id=\"mae-main\"").count(), 1, "{html}");
    assert_eq!(html.matches("class=\"mae-explorer").count(), 1, "{html}");
    assert!(
        html.contains("<meta name=\"robots\" content=\"noindex\">"),
        "{html}"
    );
    assert!(
        html.contains("class=\"site-ok\""),
        "the allowed class is kept: {html}"
    );
}

// ---------------------------------------------------------------------------
// H2: the component page, under the reserved `site` segment.

#[tokio::test]
async fn a_reader_gets_the_site_changes_page_in_the_shell() {
    let store = fixture_kb().await;
    let c = load(&[(KB, &fixture_dir())]).unwrap().for_kb(KB).unwrap();
    let page = view(&store, View::Site("changes".into()), READER, Some(c)).await;
    let SitePage::Component {
        ref title,
        ref html,
        ref explorer,
        ref slots,
    } = page
    else {
        panic!("the changes page: {page:?}");
    };
    assert_eq!(title, "Site changes");
    let want = std::fs::read_to_string(fixture_dir().join("expected/changes.html")).unwrap();
    assert_eq!(html.trim_end(), want.trim_end());
    assert!(explorer.is_some() && slots.is_some());
    let (status, doc) = served(page.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(doc.contains("<title>Site changes</title>"), "{doc}");
    assert!(doc.contains("<h1>Site changes</h1>"), "{doc}");
    assert!(doc.contains("<nav id=\"mae-explorer\""), "{doc}");
    assert!(
        doc.contains(&format!("data-mae-url=\"/kb/{KB}/site/changes\"")),
        "{doc}"
    );
    assert!(
        !doc.contains("data-mae-page="),
        "a generated page has no ID: {doc}"
    );
}

/// A5: a stranger, an unknown name and a KB without components all get the
/// one not-found, byte for byte.
#[tokio::test]
async fn every_refusal_of_a_component_page_is_the_same_not_found() {
    let store = fixture_kb().await;
    let c = || load(&[(KB, &fixture_dir())]).unwrap().for_kb(KB);
    let refusals = [
        view(&store, View::Site("changes".into()), STRANGER, c()).await,
        view(&store, View::Site("nope".into()), READER, c()).await,
        view(&store, View::Site("Changes".into()), READER, c()).await,
        view(&store, View::Site("changes".into()), READER, None).await,
    ];
    let first = body(crate::oauth::site_answer(
        SitePage::NotFound,
        "en-US".into(),
    ))
    .await;
    for r in refusals {
        assert_eq!(r, SitePage::NotFound);
        assert_eq!(
            body(crate::oauth::site_answer(r, "en-US".into())).await,
            first
        );
    }
}

/// H2, A3: a node whose ID is `site` (any case) is withheld, so no node can
/// shadow the component pages' segment.
#[tokio::test]
async fn a_node_whose_id_is_site_is_withheld() {
    let store = fresh_doc_store().await;
    let files = [
        org("site", "", "Site", "A node at the reserved segment."),
        org("Site", "", "Site upper", "Its case variant."),
        org("ok", "", "Ok", "[[id:site][to site]]"),
    ];
    seed_files(&store, KB, &files).await;
    for id in ["site", "Site"] {
        assert_eq!(
            view(&store, View::Page(id.into()), READER, None).await,
            SitePage::NotFound,
            "{id}"
        );
    }
    let SitePage::Found { html, .. } = view(&store, View::Page("ok".into()), READER, None).await
    else {
        panic!("the control page");
    };
    assert!(!html.contains("href=\"/kb/kb-comp/site\""), "{html}");
}

// ---------------------------------------------------------------------------
// H1: behaviours and stylesheets, served by hash.

#[tokio::test]
async fn a_component_asset_is_served_at_its_hash_only() {
    let set = load(&[(KB, &fixture_dir())]).unwrap();
    let js = std::fs::read_to_string(fixture_dir().join("copy-link.js")).unwrap();
    let name =
        mae_daemon::site_components::asset_name(mae_site::components::AssetKind::Script, &js);
    let (status, headers, served_js) = body(crate::oauth::asset_response(
        &format!("/site-assets/{name}"),
        &set,
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(served_js, js);
    assert!(
        headers.contains("\"content-type\": \"text/javascript; charset=utf-8\""),
        "{headers}"
    );
    assert!(
        headers.contains(&format!("\"content-security-policy\": \"{}\"", site::CSP)),
        "{headers}"
    );
    assert!(headers.contains("immutable"), "{headers}");
    // A wrong hash, the file's own name, and the component's name are all
    // the site's not-found.
    let mut wrong = name.clone();
    wrong.replace_range(2..3, if &name[2..3] == "0" { "1" } else { "0" });
    for bad in [
        wrong.as_str(),
        "copy-link.js",
        "components.css",
        "copy-link",
    ] {
        let (s, _, _) = body(crate::oauth::asset_response(
            &format!("/site-assets/{bad}"),
            &set,
        ))
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{bad}");
    }
    // The runtime's own assets are still served beside them.
    let base = mae_site::assets::href(mae_site::assets::BASE);
    assert_eq!(
        body(crate::oauth::asset_response(&base, &set)).await.0,
        StatusCode::OK
    );
}

// ---------------------------------------------------------------------------
// P-158: without components, the shell is byte-identical to the one before
// components existed. The goldens in daemon/tests/data/site-shell were
// written by the daemon at d331e7dd, before slots were added, each asset's
// content hash replaced by zeros (the router itself has changed since).

/// Every part of the shell: the site title, the explorer, tags, a table of
/// contents, backlinks, a listing, and a fallback locale's marks.
fn shell_samples() -> Vec<(&'static str, String)> {
    let e = site::ExplorerRef {
        src: "/kb/k/explorer.json".into(),
        version: "0123456789abcdef".into(),
    };
    let toc = vec![
        mae_site::TocEntry {
            level: 2,
            text: "One".into(),
            anchor: "one".into(),
        },
        mae_site::TocEntry {
            level: 3,
            text: "Two".into(),
            anchor: "two".into(),
        },
    ];
    let back = vec![mae_site::Backlink {
        title: "Back <b>".into(),
        id: "b".into(),
        href: "/kb/k/b".into(),
    }];
    let tags = vec![("t".to_string(), "/kb/k/tags/t".to_string())];
    let titled = site::Chrome {
        lang: "en-US",
        site: Some(("Site & co", "/kb/k/")),
        slots: None,
    };
    let items = vec![("A".to_string(), "/kb/k/a".to_string())];
    vec![
        (
            "page.html",
            site::document(titled, "P", "<p>x</p>", &back, &tags, &toc, Some(&e)),
        ),
        (
            "bare.html",
            site::document("en-US", "P", "<p>x</p>", &[], &[], &[], None),
        ),
        (
            "fallback.html",
            site::document("de", "P", "<p>x</p>", &back, &tags, &toc, Some(&e)),
        ),
        (
            "tags.html",
            site::listing_document(titled, &site::Listing::Tags, &items, Some(&e)),
        ),
        (
            "tag.html",
            site::listing_document("fr-FR", &site::Listing::Tag("t".into()), &items, Some(&e)),
        ),
    ]
}

#[test]
fn without_components_the_shell_is_byte_identical_to_before() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/site-shell");
    for (name, mut html) in shell_samples() {
        for asset in mae_site::assets::ALL {
            let (stem, ext) = asset.name.rsplit_once('.').expect("name.ext");
            let stem = stem.rsplit_once('-').expect("stem-hash").0;
            html = html.replace(asset.name, &format!("{stem}-{}.{ext}", "0".repeat(16)));
        }
        let want = std::fs::read_to_string(dir.join(name)).unwrap();
        assert_eq!(html, want, "{name}");
    }
}
