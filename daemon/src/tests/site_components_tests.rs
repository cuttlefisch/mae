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

