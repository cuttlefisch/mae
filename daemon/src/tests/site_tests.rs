//! The live KB site (ADR-119 L1a, `mae_daemon::site`) against a real
//! `DocStore`: what a reader may see, and that every refusal is the same
//! not-found (D7).

use std::sync::Arc;

use mae_daemon::doc_store::DocStore;
use mae_daemon::site::{self, SitePage};
use mae_mcp::identity::Identity;
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc, Role, TransportPolicy};

use super::kb_query_tests::{fresh_doc_store, seed_e2e_kb};

const BY: mae_daemon::doc_editors::Attribution<'static> =
    mae_daemon::doc_editors::Attribution::Principal("test-editor");
const READER: &str = "oauth:reader@example.com";
const STRANGER: &str = "oauth:stranger@example.com";
const LIMITS: site::SiteLimits = site::SiteLimits {
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
async fn seed(doc_store: &DocStore, kb_id: &str, nodes: &[(&str, &str, &str)]) {
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

async fn kb() -> Arc<DocStore> {
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
    let doc = site::document("en-US", &title, &html, &[], &[], &[]).to_lowercase();
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
    let html = site::document("en-US", "T", "<p>x</p>", &[], &[], &[]);
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
    let en = site::document("en-US", &title, &html, &backlinks, &[], &[]);
    assert!(
        en.contains(r#"<a href="/kb/kb-bl/from">&lt;b&gt;From&lt;/b&gt; &amp; co</a>"#),
        "{en}"
    );
    assert!(en.contains(mae_site::strings::for_locale("en-US").backlinks));
    let fr = site::document("fr-FR", &title, &html, &backlinks, &[], &[]);
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
        async move { site::view(&s, "kb-tags", &v, Some(READER), LIMITS, &Default::default()).await }
    };
    let SitePage::Listing { kind, items } = view(site::View::Tags).await else {
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
            &Default::default()
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
    let doc = site::document("en-US", &title, &html, &[], &tags, &[]);
    assert!(
        doc.contains(r#"<a href="/kb/kb-tags/tags/only_a">only_a</a>"#),
        "{doc}"
    );
    let listing = site::listing_document("fr-FR", &site::Listing::Tag("shared".into()), &items);
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
    let doc = site::document("en-US", &title, &html, &[], &[], &toc);
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
    assert!(!site::document("en-US", &title, &html, &[], &[], &toc).contains("mae-toc"));
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
    };
    let SitePage::Index(entries) = site::view(
        &store,
        "kb-idx",
        &site::View::Index,
        Some(READER),
        LIMITS,
        &opts,
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
            &opts
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
        async move { site::view(&s, "kb-home", &v, Some(who), LIMITS, &Default::default()).await }
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
    let doc = site::document("en-US", "T", "<p>body</p>", &backlinks, &[], &toc);
    assert_eq!(doc.matches("<main").count(), 1);
    let (main, after) = doc.split_once("</main>").expect("a main landmark");
    assert!(main.contains("mae-article") && main.contains("<p>body</p>"));
    for panel in ["mae-toc", "mae-backlinks"] {
        assert!(!main.contains(panel), "{panel} inside main");
        assert!(after.contains(panel), "{panel} missing");
    }
    assert!(after.starts_with("<aside id=\"mae-panels\">"));
    // A page with no panels has no empty aside.
    assert!(!site::document("en-US", "T", "<p>b</p>", &[], &[], &[]).contains("<aside"));
}
