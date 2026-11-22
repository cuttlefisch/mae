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
    let SitePage::Found { title, html } =
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
    let SitePage::Found { title, html } =
        site::page(&store, "kb-hostile", "h", Some(READER), LIMITS).await
    else {
        panic!("the page publishes, inert");
    };
    let doc = site::document("en-US", &title, &html).to_lowercase();
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
    let html = site::document("en-US", "T", "<p>x</p>");
    let href = mae_site::assets::href(mae_site::assets::PALETTE);
    assert!(
        html.contains(&format!(r#"<link rel="stylesheet" href="{href}">"#)),
        "{html}"
    );
    assert_eq!(mae_site::assets::at(&href), Some(mae_site::assets::PALETTE));
}
