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
        site::page(&store, "kb-site", "alpha", Some(READER), 500).await
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
            site::page(&store, kb_id, id, who, 500).await,
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
    let got = site::page(&store, "kb-e2e", "n1", Some(&member_principal), 500).await;
    assert_eq!(got, SitePage::NotFound);
}

#[tokio::test]
async fn a_kb_larger_than_the_bound_is_refused_before_it_is_read() {
    let store = kb().await;
    assert_eq!(
        site::page(&store, "kb-site", "alpha", Some(READER), 1).await,
        SitePage::TooLarge
    );
    assert!(matches!(
        site::page(&store, "kb-site", "alpha", Some(READER), 2).await,
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
        site::page(&store, "kb-hostile", "h", Some(READER), 500).await
    else {
        panic!("the page publishes, inert");
    };
    let doc = site::document("en-US", &title, &html).to_lowercase();
    for bad in ["<script", "onerror", "javascript:"] {
        assert!(!doc.contains(bad), "{bad}: {doc}");
    }
}
