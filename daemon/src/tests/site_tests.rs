//! The live KB site (ADR-119 L1a, `mae_daemon::site`) against a real
//! `DocStore`: what a reader may see, and that every refusal is the same
//! not-found (D7).

use std::sync::Arc;

use mae_daemon::doc_store::DocStore;
use mae_daemon::site::{self, SitePage};
use mae_mcp::identity::Identity;
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc, Role, TransportPolicy};

use super::kb_query_tests::{fresh_doc_store, seed_e2e_kb};

pub(super) const BY: mae_daemon::doc_editors::Attribution<'static> =
    mae_daemon::doc_editors::Attribution::Principal("test-editor");
pub(super) const READER: &str = "oauth:reader@example.com";
pub(super) const STRANGER: &str = "oauth:stranger@example.com";
pub(super) const LIMITS: site::SiteLimits = site::SiteLimits {
    max_scan_nodes: 500,
    max_body_bytes: 65_536,
};

fn nodes(n: usize) -> site::SiteLimits {
    site::SiteLimits {
        max_scan_nodes: n,
        ..LIMITS
    }
}

/// A plaintext KB with `READER` as a viewer and these `(id, title, body)` nodes.
pub(super) async fn seed(doc_store: &DocStore, kb_id: &str, nodes: &[(&str, &str, &str)]) {
    let owner = Arc::new(Identity::generate("owner"));
    doc_store.set_signer(Arc::clone(&owner));
    let mut coll = KbCollectionDoc::new_owned(kb_id, &owner.fingerprint(), "owner");
    coll.set_transport_policy(TransportPolicy::Hub);
    coll.upsert_member(READER, "reader", Role::Viewer);
    for (id, title, _) in nodes {
        let _ = coll.add_node(id, title);
    }
    doc_store
        .share_doc(&format!("kbc:{kb_id}"), &coll.encode_state(), BY)
        .await
        .unwrap();
    for (id, title, body) in nodes {
        let node = KbNodeDoc::new(id, title, body, &[]);
        doc_store
            .share_doc(
                &mae_sync::kb_node_doc_name(kb_id, id),
                &node.encode_state(),
                BY,
            )
            .await
            .unwrap();
    }
}

pub(super) async fn kb() -> Arc<DocStore> {
    let store = fresh_doc_store().await;
    seed(
        &store,
        "kb-site",
        &[
            (
                "alpha",
                "Alpha",
                "Alpha links to [[id:beta][the beta page]] and [[id:gone][a missing one]].\n",
            ),
            ("beta", "Beta", "Beta's body.\n"),
        ],
    )
    .await;
    store
}

#[tokio::test]
async fn a_reader_gets_the_page_with_its_links_resolved_to_live_addresses() {
    let store = kb().await;
    let SitePage::Found { title, html, .. } =
        site::page(&store, "kb-site", "alpha", Some(READER), LIMITS).await
    else {
        panic!("the reader must get the page");
    };
    assert_eq!(title, "Alpha");
    assert!(html.contains(r#"href="/kb/kb-site/beta""#), "{html}");
    assert!(html.contains("org-unresolved"), "the missing id: {html}");
}

#[tokio::test]
async fn every_refusal_is_the_same_not_found() {
    let store = kb().await;
    let cases = [
        ("kb-site", "alpha", Some(STRANGER)),   // not a member
        ("kb-site", "missing", Some(READER)),   // no such node
        ("kb-nowhere", "alpha", Some(READER)),  // no such KB
        ("Not_A_Kb_Id", "alpha", Some(READER)), // not a KB identifier
        ("kb-site", "..", Some(READER)),        // not an address
    ];
    for (kb_id, id, who) in cases {
        assert_eq!(
            site::page(&store, kb_id, id, who, LIMITS).await,
            SitePage::NotFound,
            "{kb_id}/{id}"
        );
    }
}

#[tokio::test]
async fn an_end_to_end_encrypted_kb_is_not_found_even_for_a_member() {
    let store = fresh_doc_store().await;
    let owner = Arc::new(Identity::generate("owner"));
    let member = Identity::generate("member");
    let member_principal = member.fingerprint();
    seed_e2e_kb(
        &store,
        &owner,
        "kb-e2e",
        &member_principal,
        &member,
        "n1",
        "Secret",
        "secret body",
    )
    .await;
    let got = site::page(&store, "kb-e2e", "n1", Some(&member_principal), LIMITS).await;
    assert_eq!(got, SitePage::NotFound);
}

#[tokio::test]
async fn a_kb_larger_than_the_bound_is_refused_before_it_is_read() {
    let store = kb().await;
    assert_eq!(
        site::page(&store, "kb-site", "alpha", Some(READER), nodes(1)).await,
        SitePage::TooLarge
    );
    assert!(matches!(
        site::page(&store, "kb-site", "alpha", Some(READER), nodes(2)).await,
        SitePage::Found { .. }
    ));
}

#[tokio::test]
async fn a_hostile_body_or_title_reaches_the_reader_inert() {
    let store = fresh_doc_store().await;
    seed(
        &store,
        "kb-hostile",
        &[(
            "h",
            "</title><script>alert(1)</script>",
            "#+HTML: <script>alert(1)</script>\n@@html:<img src=x onerror=alert(1)>@@\n[[javascript:alert(1)][j]]\n",
        )],
    )
    .await;
    let SitePage::Found { title, html, .. } =
        site::page(&store, "kb-hostile", "h", Some(READER), LIMITS).await
    else {
        panic!("the page publishes, inert");
    };
    let doc = site::document("en-US", &title, &html, &[], &[], &[], None);
    // The page's two scripts are the core's pre-paint script (D17) and its
    // router (D16); remove exactly those tags, and nothing script-like may
    // remain.
    let router = format!(
        r#"<script type="module" src="{}"></script>"#,
        mae_site::assets::href(mae_site::assets::ROUTER)
    );
    let prepaint = format!(
        r#"<script src="{}"></script>"#,
        mae_site::assets::href(mae_site::assets::PREPAINT)
    );
    assert_eq!(doc.matches(&router).count(), 1, "{doc}");
    assert_eq!(doc.matches(&prepaint).count(), 1, "{doc}");
    let doc = doc
        .replace(&router, "")
        .replace(&prepaint, "")
        .to_lowercase();
    for bad in ["<script", "onerror", "javascript:"] {
        assert!(!doc.contains(bad), "{bad}: {doc}");
    }
}

#[tokio::test]
async fn a_node_body_over_the_byte_bound_makes_the_kb_too_large() {
    let store = fresh_doc_store().await;
    let big = "x".repeat(2_000);
    seed(&store, "kb-big", &[("a", "A", "small"), ("b", "B", &big)]).await;
    let tight = site::SiteLimits {
        max_scan_nodes: 500,
        max_body_bytes: 1_000,
    };
    assert_eq!(
        site::page(&store, "kb-big", "a", Some(READER), tight).await,
        SitePage::TooLarge
    );
    assert!(matches!(
        site::page(&store, "kb-big", "a", Some(READER), LIMITS).await,
        SitePage::Found { .. }
    ));
}

/// Properties count against the same bound as a title or body: they reach the
/// page source too, so a node with huge properties and a tiny body is as
/// large as one with a huge body.
#[tokio::test]
async fn a_nodes_properties_over_the_byte_bound_make_the_kb_too_large() {
    let store = fresh_doc_store().await;
    seed(
        &store,
        "kb-props",
        &[("a", "A", "small"), ("b", "B", "small")],
    )
    .await;
    let mut node = KbNodeDoc::new("b", "B", "small", &[]);
    let big = "y".repeat(2_000);
    let _ = node.set_properties(&[("NOTE".to_string(), big)].into_iter().collect());
    store
        .share_doc(
            &mae_sync::kb_node_doc_name("kb-props", "b"),
            &node.encode_state(),
            BY,
        )
        .await
        .unwrap();
    let tight = site::SiteLimits {
        max_scan_nodes: 500,
        max_body_bytes: 1_000,
    };
    assert_eq!(
        site::page(&store, "kb-props", "a", Some(READER), tight).await,
        SitePage::TooLarge
    );
    assert!(matches!(
        site::page(&store, "kb-props", "a", Some(READER), LIMITS).await,
        SitePage::Found { .. }
    ));
}

/// A body that declares another page's ID cannot take that page's address:
/// the core withholds every holder of a duplicated ID, so no page serves
/// the claimant's content at the victim's address.
#[tokio::test]
async fn a_body_declaring_another_pages_id_never_serves_at_its_address() {
    let store = fresh_doc_store().await;
    seed(
        &store,
        "kb-claim",
        &[
            (
                "alpha",
                "Alpha",
                "CLAIMANT-TEXT\n* Stolen\n:PROPERTIES:\n:ID: beta\n:END:\n",
            ),
            ("beta", "Beta", "VICTIM-TEXT\n"),
        ],
    )
    .await;
    for id in ["alpha", "beta"] {
        if let SitePage::Found { html, .. } =
            site::page(&store, "kb-claim", id, Some(READER), LIMITS).await
        {
            assert!(
                !(id == "beta" && html.contains("CLAIMANT-TEXT")),
                "beta served alpha: {html}"
            );
        }
    }
    assert_eq!(
        site::page(&store, "kb-claim", "beta", Some(READER), LIMITS).await,
        SitePage::NotFound
    );
}

#[test]
fn a_page_links_the_palette_by_its_content_addressed_name() {
    let html = site::document("en-US", "T", "<p>x</p>", &[], &[], &[], None);
    let href = mae_site::assets::href(mae_site::assets::PALETTE);
    assert!(
        html.contains(&format!(r#"<link rel="stylesheet" href="{href}">"#)),
        "{html}"
    );
    assert_eq!(mae_site::assets::at(&href), Some(mae_site::assets::PALETTE));
    // The base rules come after the palette, whose tokens they use (P-102).
    let base = mae_site::assets::href(mae_site::assets::BASE);
    let link = |h: &str| html.find(&format!(r#"<link rel="stylesheet" href="{h}">"#));
    assert!(link(&href) < link(&base) && link(&base).is_some(), "{html}");
    // The router, a module script (deferred), never inline (D16, D19).
    let router = mae_site::assets::href(mae_site::assets::ROUTER);
    let script = html.find(&format!(
        r#"<script type="module" src="{router}"></script>"#
    ));
    assert!(script.is_some() && link(&base) < script, "{html}");
    // The pre-paint script (D17): classic and render-blocking (no defer, no
    // async, no module), in <head> before the first stylesheet, so the
    // reader's choice is on <html> when the palette first applies (P-91).
    let prepaint = mae_site::assets::href(mae_site::assets::PREPAINT);
    let tag = format!(r#"<script src="{prepaint}"></script>"#);
    let at = html.find(&tag);
    assert!(at.is_some() && at < link(&href), "{html}");
    assert!(at < html.find("</head>"), "{html}");
    assert!(html.find("<link") > at, "nothing styles the page before it");
    assert_eq!(
        mae_site::assets::at(&prepaint),
        Some(mae_site::assets::PREPAINT)
    );
    assert_eq!(html.matches("<script").count(), 2, "{html}");
    for bad in ["defer", "async", "<script>"] {
        assert!(!html.contains(bad), "{bad}: {html}");
    }
}

/// The palette menu's golden fixture (ADR-119 A2) starts from this shell's
/// page: each input is the document for its fixed inputs in one locale, each
/// asset's content hash replaced by zeros, so a shell or catalogue change
/// cannot leave the fixture the browser client checks describing another page.
#[test]
fn the_palette_menu_fixture_is_the_daemons_page() {
    for (lang, file) in [
        ("en-US", "input.html"),
        ("es-ES", "input-es-ES.html"),
        ("fr-FR", "input-fr-FR.html"),
    ] {
        fixture_page(lang, file);
    }
}

/// The palette-menu fixture directory (the browser client vendors it).
const PALETTE_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../docs/verification/fixtures/components/palette-menu"
);

/// The daemon's page in `lang` must equal the fixture `file`. With
/// `MAE_BLESS_FIXTURES` set, the file is written from the page instead: the
/// one way to regenerate it, so it is never edited by hand.
fn fixture_page(lang: &str, file: &str) {
    let path = std::path::Path::new(PALETTE_FIXTURE).join(file);
    let chrome = site::Chrome {
        lang,
        site: Some(("Parity fixture", "/kb/parity-fixture/")),
        slots: None,
    };
    let mut html = site::document(
        chrome,
        "Fixture page",
        "<p>A page for the palette menu fixture.</p>",
        &[],
        &[],
        &[],
        None,
    );
    for asset in mae_site::assets::ALL {
        let (stem, ext) = asset.name.rsplit_once('.').expect("name.ext");
        let stem = stem.rsplit_once('-').expect("stem-hash").0;
        html = html.replace(asset.name, &format!("{stem}-{}.{ext}", "0".repeat(16)));
    }
    if std::env::var_os("MAE_BLESS_FIXTURES").is_some() {
        std::fs::write(&path, &html).unwrap();
        return;
    }
    let fixture = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        html, fixture,
        "{lang}: regenerate {file} with MAE_BLESS_FIXTURES=1"
    );
}

/// P-91, P-93: the mode toggle and the palette button sit in the header,
/// before the main landmark, in the locale's words, inside a box that ships
/// hidden (no JavaScript: absent, never broken); the box carries the menu's
/// words. Attribute values are escaped.
#[test]
fn the_theme_controls_are_in_the_header_in_the_locales_words() {
    for locale in ["en-US", "fr-FR", "es-ES", "eo"] {
        let words = mae_site::strings::for_locale(locale);
        let html = site::document(locale, "T", "<p>x</p>", &[], &[], &[], None);
        let (before_main, after) = html.split_once("<main").expect("a main landmark");
        assert!(!after.contains("mae-theme"), "{html}");
        let header = before_main
            .split_once(r#"<header id="mae-header">"#)
            .expect("a header")
            .1;
        assert!(
            header.starts_with(r#"<div id="mae-theme" class="mae-theme" hidden "#),
            "{header}"
        );
        let toggle = format!(
            r#"<button type="button" id="mae-mode-toggle" aria-pressed="false"><span class="mae-mode-icon" aria-hidden="true"></span>{}</button>"#,
            words.mode_toggle
        );
        let button = format!(
            r#"<button type="button" id="mae-palette-button" aria-haspopup="menu" aria-expanded="false" aria-controls="mae-palette-menu">{}</button>"#,
            words.palette_menu
        );
        assert!(header.contains(&toggle), "{header}");
        assert!(header.contains(&button), "{header}");
        assert!(header.find(&toggle) < header.find(&button), "{header}");
        assert!(
            header.contains(&format!(r#"data-default-label="{}""#, words.accent_default)),
            "{header}"
        );
        for (id, name) in words.accents {
            assert!(
                header.contains(&format!(r#" data-accent-{id}="{name}""#)),
                "{id}: {header}"
            );
        }
        assert!(!header.contains("title="), "no title attribute: {header}");
        // No menu in the markup: the behaviour builds it (with createElement).
        assert!(!html.contains(r#"id="mae-palette-menu""#), "{html}");
    }
}

#[tokio::test]
async fn a_page_lists_its_backlinks_in_the_configured_locale_and_escapes_them() {
    let store = fresh_doc_store().await;
    seed(
        &store,
        "kb-bl",
        &[
            ("target", "Target", "The page linked to.\n"),
            (
                "from",
                "<b>From</b> & co",
                "Links to [[id:target][the target]].\n",
            ),
        ],
    )
    .await;
    let SitePage::Found {
        title,
        html,
        backlinks,
        ..
    } = site::page(&store, "kb-bl", "target", Some(READER), LIMITS).await
    else {
        panic!("target publishes");
    };
    assert_eq!(backlinks.len(), 1);
    let en = site::document("en-US", &title, &html, &backlinks, &[], &[], None);
    assert!(
        en.contains(r#"<a href="/kb/kb-bl/from">&lt;b&gt;From&lt;/b&gt; &amp; co</a>"#),
        "{en}"
    );
    assert!(en.contains(mae_site::strings::for_locale("en-US").backlinks));
    let fr = site::document("fr-FR", &title, &html, &backlinks, &[], &[], None);
    assert!(fr.contains(mae_site::strings::for_locale("fr-FR").backlinks));
    assert!(!fr.contains(mae_site::strings::for_locale("en-US").backlinks));
}

#[tokio::test]
async fn the_tag_index_and_a_tag_page_list_what_the_kb_tags() {
    // Tags come from each page's #+filetags, carried here in the body.
    let store2 = fresh_doc_store().await;
    seed(
        &store2,
        "kb-tags",
        &[
            ("a", "Alpha", "#+filetags: :shared:only_a:\nx\n"),
            ("b", "Beta", "#+filetags: :shared:\ny\n"),
        ],
    )
    .await;
    let view = |v: site::View| {
        let s = store2.clone();
        async move {
            site::view(
                &s,
                "kb-tags",
                &v,
                Some(READER),
                LIMITS,
                &Default::default(),
                None,
            )
            .await
        }
    };
    let SitePage::Listing { kind, items, .. } = view(site::View::Tags).await else {
        panic!("the tag index");
    };
    assert_eq!(kind, site::Listing::Tags);
    let names: Vec<&str> = items.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(names, vec!["only_a", "shared"]);
    assert_eq!(items[1].1, "/kb/kb-tags/tags/shared");
    let SitePage::Listing { items, .. } = view(site::View::Tag("shared".into())).await else {
        panic!("a tag page");
    };
    let titles: Vec<&str> = items.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(titles, vec!["Alpha", "Beta"]);
    assert_eq!(
        view(site::View::Tag("nope".into())).await,
        SitePage::NotFound
    );
    // A stranger sees no tags either: the same not-found.
    let s = store2.clone();
    assert_eq!(
        site::view(
            &s,
            "kb-tags",
            &site::View::Tags,
            Some(STRANGER),
            LIMITS,
            &Default::default(),
            None,
        )
        .await,
        SitePage::NotFound
    );
    // The page itself links its tags.
    let SitePage::Found {
        title, html, tags, ..
    } = view(site::View::Page("a".into())).await
    else {
        panic!("page a");
    };
    let doc = site::document("en-US", &title, &html, &[], &tags, &[], None);
    assert!(
        doc.contains(r#"<a href="/kb/kb-tags/tags/only_a">only_a</a>"#),
        "{doc}"
    );
    let listing =
        site::listing_document("fr-FR", &site::Listing::Tag("shared".into()), &items, None);
    assert!(
        listing.contains("lang=\"fr-FR\"") && listing.contains("shared"),
        "{listing}"
    );
}

#[tokio::test]
async fn a_page_with_two_headings_has_a_toc_linking_to_their_anchors() {
    let store = fresh_doc_store().await;
    seed(
        &store,
        "kb-toc",
        &[
            ("two", "Two", "* First & <one>\ntext\n* Second\nmore\n"),
            ("one", "One", "* Only\ntext\n"),
        ],
    )
    .await;
    let page = |id: &'static str| {
        let s = store.clone();
        async move { site::page(&s, "kb-toc", id, Some(READER), LIMITS).await }
    };
    let SitePage::Found {
        title, html, toc, ..
    } = page("two").await
    else {
        panic!("two");
    };
    assert_eq!(toc.len(), 2);
    let doc = site::document("en-US", &title, &html, &[], &[], &toc, None);
    assert!(
        doc.contains(r#"<nav id="mae-toc" aria-labelledby="mae-toc-h">"#),
        "{doc}"
    );
    assert!(
        doc.contains(&format!(
            r##"<a href="#{}">First &amp; &lt;one&gt;</a>"##,
            toc[0].anchor
        )),
        "{doc}"
    );
    for e in &toc {
        assert!(
            doc.contains(&format!("id=\"{}\"", e.anchor)),
            "{} has no target",
            e.anchor
        );
    }
    let SitePage::Found {
        title, html, toc, ..
    } = page("one").await
    else {
        panic!("one");
    };
    assert!(!site::document("en-US", &title, &html, &[], &[], &toc, None).contains("mae-toc"));
}

#[tokio::test]
async fn the_content_index_lists_published_pages_without_hidden_references() {
    let store = fresh_doc_store().await;
    seed(
        &store,
        "kb-idx",
        &[
            (
                "a",
                "Alpha",
                "To [[id:b][Beta]] and internal:secret-7 and public:ok-1.\n",
            ),
            ("b", "Beta", "Beta's text.\n"),
            ("c", "Gamma", "A note[fn:missing] to [[id:a][Alpha]].\n"),
        ],
    )
    .await;
    let opts = mae_site::SiteOptions {
        hidden_namespaces: vec!["internal".into()],
        ..Default::default()
    };
    let SitePage::Index(entries) = site::view(
        &store,
        "kb-idx",
        &site::View::Index,
        Some(READER),
        LIMITS,
        &opts,
        None,
    )
    .await
    else {
        panic!("the index");
    };
    let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, vec!["a", "b"], "c is withheld (undefined footnote)");
    assert_eq!(entries[0].links, vec!["b".to_string()]);
    assert!(
        !entries[0].text.contains("internal:secret-7"),
        "{}",
        entries[0].text
    );
    assert!(entries[0].text.contains("public:ok-1"));
    assert_eq!(
        site::view(
            &store,
            "kb-idx",
            &site::View::Index,
            Some(STRANGER),
            LIMITS,
            &opts,
            None,
        )
        .await,
        SitePage::NotFound
    );
}

/// Amendment 2 B3: `/kb/<kb-id>/` is the node the operator declared, when the
/// reader has it as a page. Undeclared, unknown or unpublished all answer the
/// same tag index, so the home address says nothing about a node the reader
/// cannot see; a reader without access gets the not-found every route gives.
#[tokio::test]
async fn the_home_page_is_the_declared_node_or_else_the_tag_index() {
    let store = fresh_doc_store().await;
    seed(
        &store,
        "kb-home",
        &[
            ("home", "Welcome", "#+filetags: :t:\nhello\n"),
            ("other", "Other", "x\n"),
        ],
    )
    .await;
    let view = |v: site::View, who: &'static str| {
        let s = store.clone();
        async move {
            site::view(
                &s,
                "kb-home",
                &v,
                Some(who),
                LIMITS,
                &Default::default(),
                None,
            )
            .await
        }
    };
    let declared = view(site::View::Home(Some("home".into())), READER).await;
    let SitePage::Found { title, .. } = &declared else {
        panic!("the declared home page: {declared:?}");
    };
    assert_eq!(title, "Welcome");
    let tag_index = view(site::View::Tags, READER).await;
    assert!(matches!(tag_index, SitePage::Listing { .. }));
    for fallback in [None, Some("absent".to_string())] {
        assert_eq!(
            view(site::View::Home(fallback.clone()), READER).await,
            tag_index,
            "{fallback:?}"
        );
    }
    assert_eq!(
        view(site::View::Home(Some("home".into())), STRANGER).await,
        SitePage::NotFound
    );
}

/// P-165: one main landmark holding the article; the side panels (TOC,
/// backlinks) after it, never inside it.
#[test]
fn the_side_panels_sit_outside_the_main_landmark() {
    let toc = vec![
        mae_site::TocEntry {
            level: 2,
            text: "A".into(),
            anchor: "a".into(),
        },
        mae_site::TocEntry {
            level: 2,
            text: "B".into(),
            anchor: "b".into(),
        },
    ];
    let backlinks = vec![mae_site::Backlink {
        id: "x".into(),
        title: "X".into(),
        href: "/kb/k/x".into(),
    }];
    let doc = site::document("en-US", "T", "<p>body</p>", &backlinks, &[], &toc, None);
    assert_eq!(doc.matches("<main").count(), 1);
    let (main, after) = doc.split_once("</main>").expect("a main landmark");
    assert!(main.contains("mae-article") && main.contains("<p>body</p>"));
    for panel in ["mae-toc", "mae-backlinks"] {
        assert!(!main.contains(panel), "{panel} inside main");
        assert!(after.contains(panel), "{panel} missing");
    }
    assert!(after.starts_with("<aside id=\"mae-panels\">"));
    // A page with no panels has no empty aside.
    assert!(!site::document("en-US", "T", "<p>b</p>", &[], &[], &[], None).contains("<aside"));
}

/// The fixture's files as `kb seed` writes them: each through the seed's own
/// split and file-to-node mapping, so tags, properties and title sit in the
/// node's fields, not in its body. (The tests above put `#+filetags` in the
/// body, which the seed never does; that is how a site serving no tags at all
/// passed them.)
async fn seed_files(doc_store: &DocStore, kb_id: &str, files: &[&str]) -> Vec<String> {
    let nodes: Vec<KbNodeDoc> = files
        .iter()
        .map(|text| {
            let f = mae_daemon::seed::split(text).expect("a fixture file splits");
            mae_daemon::seed::node_from_file(&f, 0)
        })
        .collect();
    let owner = Arc::new(Identity::generate("owner"));
    doc_store.set_signer(Arc::clone(&owner));
    let mut coll = KbCollectionDoc::new_owned(kb_id, &owner.fingerprint(), "owner");
    coll.set_transport_policy(TransportPolicy::Hub);
    coll.upsert_member(READER, "reader", Role::Viewer);
    let mut ids = Vec::new();
    for n in &nodes {
        let id = n.id();
        let _ = coll.add_node(&id, &n.title());
        ids.push(id);
    }
    doc_store
        .share_doc(&format!("kbc:{kb_id}"), &coll.encode_state(), BY)
        .await
        .unwrap();
    for (n, id) in nodes.iter().zip(&ids) {
        doc_store
            .share_doc(
                &mae_sync::kb_node_doc_name(kb_id, id),
                &n.encode_state(),
                BY,
            )
            .await
            .unwrap();
    }
    ids
}

/// P-70, P-71, P-73 on seeded content: the tags `#+filetags` declared reach the
/// page, the tag index and each tag's page. At b122cc83 every one was empty,
/// because a page was rebuilt from the node's title and body alone.
#[tokio::test]
async fn a_seeded_pages_file_tags_reach_its_page_and_the_tag_pages() {
    let store = fresh_doc_store().await;
    let ids = seed_files(
        &store,
        "kb-seeded",
        &[
            include_str!("../../../docs/verification/fixtures/kb-site-parity/bundle/index.org"),
            include_str!(
                "../../../docs/verification/fixtures/kb-site-parity/bundle/guides/getting-started.org"
            ),
        ],
    )
    .await;
    let view = |v: site::View| {
        let s = store.clone();
        async move {
            site::view(
                &s,
                "kb-seeded",
                &v,
                Some(READER),
                LIMITS,
                &Default::default(),
                None,
            )
            .await
        }
    };
    let SitePage::Found { title, tags, .. } = view(site::View::Page(ids[0].clone())).await else {
        panic!("the fixture home page");
    };
    assert_eq!(title, "Fixture home");
    let names: Vec<&str> = tags.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(names, vec!["fixture", "home"]);
    let SitePage::Listing { items, .. } = view(site::View::Tags).await else {
        panic!("the tag index");
    };
    let all: Vec<&str> = items.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(all, vec!["fixture", "guides", "home"]);
    let SitePage::Listing { items, .. } = view(site::View::Tag("guides".into())).await else {
        panic!("the guides tag page");
    };
    let titles: Vec<&str> = items.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(titles, vec!["Getting started"]);
}

/// Amendment 2 B4: with ADR-073's view retired, a node whose ID is `view` is
/// served at its own address like any other.
#[tokio::test]
async fn a_node_whose_id_is_view_is_served_at_its_address() {
    let store = fresh_doc_store().await;
    seed(&store, "kb-view", &[("view", "A page named view", "x\n")]).await;
    let page = site::view(
        &store,
        "kb-view",
        &site::View::Page("view".into()),
        Some(READER),
        LIMITS,
        &Default::default(),
        None,
    )
    .await;
    let SitePage::Found { title, .. } = page else {
        panic!("the page at /kb/kb-view/view: {page:?}");
    };
    assert_eq!(title, "A page named view");
}

/// P-16, P-165: with a site title, every page's `<title>` names page and
/// site, and a header before the main landmark opens with a link to the KB's
/// home; both are escaped. Without one, neither appears (the header then holds
/// only the theme controls).
#[test]
fn the_site_title_heads_every_page_outside_main() {
    let chrome = site::Chrome {
        lang: "en-US",
        site: Some(("Parity <fixture>", "/kb/parity-fixture/")),
        slots: None,
    };
    for doc in [
        site::document(chrome, "Page & co", "<p>x</p>", &[], &[], &[], None),
        site::listing_document(chrome, &site::Listing::Tags, &[], None),
    ] {
        assert!(
            doc.contains("\u{2014} Parity &lt;fixture&gt;</title>"),
            "{doc}"
        );
        let header = r#"<header id="mae-header"><a href="/kb/parity-fixture/">Parity &lt;fixture&gt;</a><div id="mae-theme""#;
        let (before_main, after) = doc.split_once("<main").expect("a main landmark");
        let at = before_main
            .find(header)
            .expect("the header opens with the site link");
        assert!(before_main[at..].ends_with("</header>"), "{doc}");
        assert!(!after.contains("mae-header"), "{doc}");
    }
    let bare = site::document("en-US", "Page", "<p>x</p>", &[], &[], &[], None);
    assert!(bare.contains("<title>Page</title>"), "{bare}");
    let (head_of_body, _) = bare.split_once("<main").expect("a main landmark");
    assert!(
        head_of_body.contains(r#"<header id="mae-header"><div id="mae-theme""#)
            && !head_of_body.contains("<a "),
        "{bare}"
    );
}

/// Each text run and attribute value of a page the shell wrote, with the
/// language in effect there (the nearest `lang`) and whether it is in the
/// `<title>`. Enough of a parser for the shell's own output: every value is
/// quoted and escaped, and no element but the void ones goes unclosed.
fn scoped(html: &str) -> Vec<(String, String, bool)> {
    const VOID: [&str; 4] = ["meta", "link", "br", "img"];
    let mut stack: Vec<(String, String)> = Vec::new();
    let mut out = Vec::new();
    let mut rest = html;
    let now = |st: &[(String, String)]| st.last().map(|(_, l)| l.clone()).unwrap_or_default();
    let in_title = |st: &[(String, String)]| st.iter().any(|(t, _)| t == "title");
    while let Some(i) = rest.find('<') {
        if !rest[..i].trim().is_empty() {
            out.push((rest[..i].to_string(), now(&stack), in_title(&stack)));
        }
        let j = i + rest[i..].find('>').expect("a closed tag");
        let tag = &rest[i + 1..j];
        rest = &rest[j + 1..];
        if tag.starts_with('!') {
            continue;
        }
        if let Some(name) = tag.strip_prefix('/') {
            let at = stack.iter().rposition(|(t, _)| t == name).expect(name);
            stack.truncate(at);
            continue;
        }
        let (name, mut attrs) = tag.split_once(' ').unwrap_or((tag, ""));
        let mut lang = now(&stack);
        let mut values = Vec::new();
        while let Some(eq) = attrs.find("=\"") {
            let key = attrs[..eq].trim().rsplit(' ').next().unwrap_or_default();
            let end = eq + 2 + attrs[eq + 2..].find('"').expect("a quoted value");
            let value = &attrs[eq + 2..end];
            if key == "lang" {
                lang = value.to_string();
            }
            values.push(value.to_string());
            attrs = &attrs[end + 1..];
        }
        for v in values {
            out.push((v, lang.clone(), in_title(&stack)));
        }
        if !VOID.contains(&name) {
            stack.push((name.to_string(), lang));
        }
    }
    out
}

/// Every string of the en-US catalogue, each accent name included.
fn en_us_strings() -> Vec<&'static str> {
    let w = mae_site::strings::for_locale("en-US");
    let mut all = vec![
        w.backlinks,
        w.contents,
        w.tags,
        w.tag_page_open,
        w.tag_page_close,
        w.mode_toggle,
        w.palette_menu,
        w.accent_default,
        w.explorer,
        w.untagged,
    ];
    all.extend(w.accents.iter().map(|(_, n)| *n));
    all
}

/// ADR-119 A4: a locale with no catalogue gets en-US's words, and every
/// element holding them is marked `lang="en-US"` (a screen reader voices
/// them as English), while `<html lang>` stays the configured locale and the
/// content (titles, tags, headings, backlinks) keeps it. Checked over every
/// text and attribute of a page and both listings, not element by element,
/// so a new piece of chrome written unmarked fails here. The one exception
/// is the `<title>`: plain text that cannot mark part of itself.
#[test]
fn a_fallback_page_marks_its_chrome_en_us_and_keeps_the_configured_lang() {
    let toc = vec![
        mae_site::TocEntry {
            level: 2,
            text: "zeta heading".into(),
            anchor: "a".into(),
        },
        mae_site::TocEntry {
            level: 2,
            text: "eta heading".into(),
            anchor: "b".into(),
        },
    ];
    let backlinks = vec![mae_site::Backlink {
        id: "x".into(),
        title: "theta source".into(),
        href: "/kb/k/x".into(),
    }];
    let tags = vec![("iota".to_string(), "/kb/k/tags/iota".to_string())];
    let items = vec![("kappa page".to_string(), "/kb/k/kappa".to_string())];
    let explorer = site::ExplorerRef {
        src: "/kb/k/explorer.json".into(),
        version: "0123456789abcdef".into(),
    };
    let pages = [
        site::document(
            "xx-YY",
            "lambda title",
            "<p>mu body</p>",
            &backlinks,
            &tags,
            &toc,
            Some(&explorer),
        ),
        site::listing_document("xx-YY", &site::Listing::Tags, &tags, Some(&explorer)),
        site::listing_document(
            "xx-YY",
            &site::Listing::Tag("iota".into()),
            &items,
            Some(&explorer),
        ),
    ];
    let en = en_us_strings();
    let mut seen: Vec<&str> = Vec::new();
    for page in &pages {
        assert!(page.contains(r#"<html lang="xx-YY">"#), "{page}");
        for (text, lang, in_title) in scoped(page) {
            for word in en.iter().filter(|w| text.contains(**w)) {
                seen.push(word);
                assert!(
                    lang == "en-US" || in_title,
                    "{word:?} in {text:?} is in {lang:?}: {page}"
                );
            }
            let content = [
                "zeta heading",
                "eta heading",
                "theta source",
                "iota",
                "kappa page",
                "lambda title",
                "mu body",
            ];
            if content.contains(&text.as_str()) {
                assert_eq!(lang, "xx-YY", "{text:?}: {page}");
            }
        }
    }
    for word in &en {
        assert!(seen.contains(word), "{word:?} was never written");
    }
}

/// A found catalogue is in the page's own language: nothing is marked, and a
/// configured es-ES or eo page never carries en-US's words.
#[test]
fn a_catalogued_locale_marks_nothing() {
    let tags = vec![("iota".to_string(), "/kb/k/tags/iota".to_string())];
    for locale in ["en-US", "fr-FR", "es-ES", "eo"] {
        let pages = [
            site::document(locale, "T", "<p>x</p>", &[], &tags, &[], None),
            site::listing_document(locale, &site::Listing::Tag("iota".into()), &tags, None),
        ];
        for page in pages {
            assert_eq!(page.matches(" lang=").count(), 1, "{locale}: {page}");
            assert!(page.contains(&format!(r#"<html lang="{locale}">"#)));
            if locale != "en-US" {
                let w = mae_site::strings::for_locale("en-US");
                for word in [w.mode_toggle, w.palette_menu, w.tags] {
                    assert!(!page.contains(word), "{locale}: {word}: {page}");
                }
            }
        }
    }
}

/// The theme box, configured es-ES, carries the es-ES words the palette menu
/// reads, pinned as written rather than read back from the catalogue.
#[test]
fn the_theme_box_carries_the_es_es_labels() {
    let html = site::document("es-ES", "T", "<p>x</p>", &[], &[], &[], None);
    let box_ = html
        .split_once(r#"<div id="mae-theme""#)
        .expect("the box")
        .1
        .split_once("</div>")
        .expect("closed")
        .0;
    for want in [
        r#" data-default-label="Predeterminado""#,
        r#" data-accent-red="Rojo""#,
        r#" data-accent-orange="Naranja""#,
        r#" data-accent-yellow="Amarillo""#,
        r#" data-accent-green="Verde""#,
        r#" data-accent-teal="Verde azulado""#,
        r#" data-accent-cyan="Cian""#,
        r#" data-accent-blue="Azul""#,
        r#" data-accent-violet="Violeta""#,
        r#" data-accent-magenta="Magenta""#,
        "</span>Modo oscuro</button>",
        r#"aria-controls="mae-palette-menu">Paleta de colores</button>"#,
    ] {
        assert!(box_.contains(want), "{want}: {box_}");
    }
    assert!(!box_.contains(" lang="), "{box_}");
}

/// ADR-115 E3 on the live site: a NAME whose blob the KB's store holds is an
/// `<img>` at the blob's route; a NAME bound to a blob the store lacks, and a
/// link naming no attachment of the node, are the labelled `missing`
/// placeholder, which carries no hash. Kills: resolving from the node's map
/// without asking the store, a URL other than the blob route, or a page's
/// NAMEs leaking into another page.
#[tokio::test]
async fn an_attachment_the_store_holds_is_an_image_and_one_it_lacks_is_a_placeholder() {
    use mae_daemon::blob_store::{process, upload, BlobStore};
    use mae_sync::kb::AttachmentRef;

    let store = fresh_doc_store().await;
    let dir = tempfile::tempdir().unwrap();
    let blobs = Arc::new(BlobStore::open(&dir.path().join("blobs")).unwrap());
    store.set_blob_store(Arc::clone(&blobs));
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(6, 3, image::Rgb([9, 99, 199])))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let limits = process::ProcessLimits {
        max_pixels: 1_000_000,
        max_frames: 1,
    };
    let kb_id = "kb-img";
    let held = Arc::clone(&blobs);
    let meta = tokio::task::spawn_blocking(move || {
        upload::store_bytes(
            &held,
            kb_id,
            png,
            &limits,
            None,
            &process::process,
            &upload::Job::new(),
        )
    })
    .await
    .unwrap()
    .unwrap();
    let ghost = "ab".repeat(32);
    seed(
        &store,
        kb_id,
        &[
            (
                "pic",
                "Pic",
                "[[attachment:shown.png]] [[attachment:gone.png]] [[attachment:unbound.png]]\n",
            ),
            // Another page binds `unbound.png`: it must not resolve on `pic`.
            ("other", "Other", "[[attachment:unbound.png]]\n"),
        ],
    )
    .await;
    let bind = |id: &'static str, entries: Vec<(&'static str, String)>| {
        let store = Arc::clone(&store);
        async move {
            let name = mae_sync::kb_node_doc_name(kb_id, id);
            let mut doc = KbNodeDoc::from_bytes(&store.encode_state(&name).await.unwrap()).unwrap();
            for (n, sha) in entries {
                let update = doc
                    .set_attachment(n, &AttachmentRef::new(sha, 10, "image/png").unwrap())
                    .unwrap();
                store.apply_update(&name, &update, None, BY).await.unwrap();
            }
        }
    };
    bind(
        "pic",
        vec![
            ("shown.png", meta.sha256.clone()),
            ("gone.png", ghost.clone()),
        ],
    )
    .await;
    bind("other", vec![("unbound.png", meta.sha256.clone())]).await;

    let SitePage::Found { html, .. } = site::page(&store, kb_id, "pic", Some(READER), LIMITS).await
    else {
        panic!("the page is readable");
    };
    let img = format!("<img src=\"/kb/{kb_id}/blobs/{}\"", meta.sha256);
    assert_eq!(html.matches(&img).count(), 1, "{html}");
    assert!(html.contains("width=\"6\" height=\"3\""), "{html}");
    assert_eq!(html.matches("data-state=\"missing\"").count(), 2, "{html}");
    assert!(!html.contains(&ghost), "{html}");
}
