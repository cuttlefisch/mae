//! A browser editor's page lifecycle (ADR-036 Amendment 2, G1/G4), through the
//! real dispatcher on a KB with `require_signed_content_ops` on, as the
//! browser client's `/ws` relay sends it: create (the node's first
//! `kb/node_update`, then `kb/collection_node_add`), rename (the node's title,
//! then an add of the same node with the new title), delete
//! (`kb/collection_node_remove`), and what another session subscribed to the
//! KB's collection receives. `docs/verification/fixtures/kb-pages/` is pinned
//! against these exchanges.

use super::collab_handler_error_kind_tests::expect_kind;
use super::collab_handler_hub_verify_tests::{hv, Hv, Key, KB};
use super::*;
use mae_sync::content_ops::ContentOp;
use mae_sync::kb::KbNodeDoc;
use mae_sync::manifest_op::{ManifestAction, ManifestOp};

const PAGE: &str = "5f0c2a9e-8d1b-4c3e-9a7f-2b6d4e8c1a03";

/// The fixture this test pins.
fn fixture() -> serde_json::Value {
    let path = format!(
        "{}/../docs/verification/fixtures/kb-pages/page_lifecycle.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The fixture step named `name`'s result.
fn step_result(name: &str) -> serde_json::Value {
    fixture()["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("no step {name}"))["response"]["result"]
        .clone()
}

/// The same keys at every level, and the same JSON type at each.
fn same_shape(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value::*;
    match (a, b) {
        (Object(x), Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| same_shape(v, w)))
        }
        (Array(_), Array(_)) | (String(_), String(_)) | (Number(_), Number(_)) => true,
        (Bool(_), Bool(_)) | (Null, Null) => true,
        _ => false,
    }
}
const WATCHER: u64 = 77;

/// The signed header fields of a content op over `update` on `node`.
fn content_header(author: &Key, node: &str, epoch: u64, update: &[u8]) -> serde_json::Value {
    let op = ContentOp {
        kb_id: KB.to_string(),
        node_id: node.to_string(),
        base_sv: vec![],
        author: author.fp.clone(),
        epoch,
        issued_at: 1_790_000_000,
    };
    serde_json::json!({
        "author": op.author, "epoch": op.epoch.to_string(), "issued_at": op.issued_at,
        "base_sv": update_to_base64(&op.base_sv),
        "author_pubkey": update_to_base64(&author.pk),
        "sig": update_to_base64(&op.sign(&author.id.secret_bytes(), update)),
    })
}

fn node_update(author: &Key, node: &str, epoch: u64, update: &[u8]) -> serde_json::Value {
    let mut params = serde_json::json!({"kb_id": KB, "node_id": node,
        "update": update_to_base64(update)});
    for (k, v) in content_header(author, node, epoch, update)
        .as_object()
        .unwrap()
    {
        params[k] = v.clone();
    }
    serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "kb/node_update", "params": params})
}

/// `kb/collection_node_add` (with `title`) or `_remove` (`title` None), signed
/// by `author` at `epoch`, as `signManifestOp`'s result is sent.
fn manifest(author: &Key, node: &str, title: Option<&str>, epoch: u64) -> serde_json::Value {
    let op = ManifestOp {
        kb_id: KB.to_string(),
        node_id: node.to_string(),
        action: if title.is_some() {
            ManifestAction::Add
        } else {
            ManifestAction::Remove
        },
        title: title.unwrap_or_default().to_string(),
        author: author.fp.clone(),
        epoch,
        issued_at: 1_790_000_000,
    };
    let method = if title.is_some() {
        "kb/collection_node_add"
    } else {
        "kb/collection_node_remove"
    };
    let mut params = serde_json::json!({
        "kb_id": KB, "node_id": node,
        "author": op.author, "epoch": op.epoch.to_string(), "issued_at": op.issued_at,
        "author_pubkey": update_to_base64(&author.pk),
        "sig": update_to_base64(&op.sign(&author.id.secret_bytes())),
    });
    if let Some(t) = title {
        params["title"] = serde_json::json!(t);
    }
    serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
}

async fn listed(h: &Hv) -> Vec<(String, String)> {
    let state = h.store.encode_state(&format!("kbc:{KB}")).await.unwrap();
    KbCollectionDoc::from_bytes(&state).unwrap().list_nodes()
}

fn title_of(l: &[(String, String)], node: &str) -> Option<String> {
    l.iter().find(|(id, _)| id == node).map(|(_, t)| t.clone())
}

/// The events a watcher of the KB's collection received since the last call.
fn drain(rx: &mut tokio::sync::mpsc::Receiver<EditorEvent>) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(serde_json::to_value(&e).unwrap());
    }
    out
}

/// Create a page as the browser does, asserting each step against the
/// fixture: the content first (unlisted, unseen by the watcher), then the
/// listing (one notification). Answers the page's document.
async fn created(
    h: &mut Hv,
    bob: &Key,
    epoch: u64,
    rx: &mut tokio::sync::mpsc::Receiver<EditorEvent>,
) -> KbNodeDoc {
    let cid = mae_sync::kb::derive_kb_client_id(&bob.fp, epoch);
    // Create: the content first, then the listing.
    let page = KbNodeDoc::new_with_client_id(PAGE, "Release checklist", "Steps.\n", &[], cid);
    let first = page.encode_state();
    let r = h
        .call(bob.bearer(), node_update(bob, PAGE, epoch, &first))
        .await;
    assert!(
        r.error.is_none(),
        "node_update of an unlisted node: {:?}",
        r.error
    );
    assert_eq!(
        r.result.clone().unwrap(),
        step_result("create, 1: the node's content")
    );
    assert_eq!(
        title_of(&listed(h).await, PAGE),
        None,
        "content alone lists nothing"
    );
    assert!(
        drain(rx).is_empty(),
        "a node's own write is not a collection change"
    );
    let add = manifest(bob, PAGE, Some("Release checklist"), epoch);
    let r = h.call(bob.bearer(), add).await;
    assert!(r.error.is_none(), "add: {:?}", r.error);
    assert_eq!(r.result.clone().unwrap(), step_result("create, 2: list it"));
    assert_eq!(
        title_of(&listed(h).await, PAGE).as_deref(),
        Some("Release checklist")
    );
    let seen = drain(rx);
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(
        same_shape(&seen[0], &fixture()["notification"]),
        "{}",
        seen[0]
    );
    assert_eq!(
        seen[0]["data"]["buffer_name"],
        serde_json::json!(format!("kbc:{KB}"))
    );

    page
}

/// Create, rename and delete, each as the browser sends it, each seen by
/// another session as one sync update of `kbc:<kb>`. Kills: a node_update that
/// lists the node, an add that refuses an existing id instead of retitling it,
/// a remove that leaves the node listed.
#[tokio::test]
async fn a_browser_creates_renames_and_deletes_a_page() {
    let (mut h, k) = hv(true, false, true).await;
    let mut rx = {
        let mut b = h.bc.lock().unwrap();
        let rx = b.subscribe(WATCHER, vec!["sync_update".into()]);
        b.subscribe_doc(WATCHER, &format!("kbc:{KB}"));
        rx
    };
    let epoch = h.epoch(&k.bob).await;
    let mut page = created(&mut h, &k.bob, epoch, &mut rx).await;

    // Rename: the node's title, then an add of the same id (an upsert).
    let retitle = page.set_title("Release checklist (2027)");
    let r = h
        .call(k.bob.bearer(), node_update(&k.bob, PAGE, epoch, &retitle))
        .await;
    assert!(r.error.is_none(), "retitle content: {:?}", r.error);
    assert_eq!(
        title_of(&listed(&h).await, PAGE).as_deref(),
        Some("Release checklist"),
        "the listed title does not follow the content"
    );
    let r = h
        .call(
            k.bob.bearer(),
            manifest(&k.bob, PAGE, Some("Release checklist (2027)"), epoch),
        )
        .await;
    assert!(
        r.error.is_none(),
        "an add of an existing id retitles it: {:?}",
        r.error
    );
    assert_eq!(
        title_of(&listed(&h).await, PAGE).as_deref(),
        Some("Release checklist (2027)")
    );
    assert_eq!(
        listed(&h).await.iter().filter(|(id, _)| id == PAGE).count(),
        1
    );
    assert_eq!(drain(&mut rx).len(), 1);

    // Delete.
    let rm = manifest(&k.bob, PAGE, None, epoch);
    let r = h.call(k.bob.bearer(), rm).await;
    assert!(r.error.is_none(), "remove: {:?}", r.error);
    assert_eq!(r.result.clone().unwrap(), step_result("delete"));
    assert_eq!(title_of(&listed(&h).await, PAGE), None);
    assert_eq!(drain(&mut rx).len(), 1);
}

/// Each refusal a browser editor can meet on the node list, by kind, and none
/// changes the list. Kills each check of the path: the switch, the
/// signature, the author, the epoch, the role.
#[tokio::test]
async fn the_page_list_refusals_by_kind() {
    let (mut h, k) = hv(true, false, true).await;
    let viewer = super::collab_handler_hub_verify_tests::key("viewer");
    let add_viewer = kb_member_msg("kb/add_member", KB, &viewer.fp, Some("viewer"));
    assert!(h.call(k.owner.bearer(), add_viewer).await.error.is_none());
    let epoch = h.epoch(&k.bob).await;
    let before = listed(&h).await;

    let mut unsigned = manifest(&k.bob, PAGE, Some("T"), epoch);
    for f in ["author", "epoch", "issued_at", "author_pubkey", "sig"] {
        unsigned["params"].as_object_mut().unwrap().remove(f);
    }
    let mut bad = manifest(&k.bob, PAGE, Some("T"), epoch);
    bad["params"]["title"] = serde_json::json!("Not what was signed");
    let cases: Vec<(&str, Session<'_>, serde_json::Value, &str)> = vec![
        (
            "unsigned, the switch on",
            k.bob.bearer(),
            unsigned,
            "signature_required",
        ),
        (
            "a field changed after signing",
            k.bob.bearer(),
            bad,
            "signature_invalid",
        ),
        (
            "another person's op",
            k.carol.bearer(),
            manifest(&k.bob, PAGE, Some("T"), epoch),
            "not_author",
        ),
        (
            "a stale epoch",
            k.bob.bearer(),
            manifest(&k.bob, PAGE, Some("T"), epoch.wrapping_add(1)),
            "epoch_fenced",
        ),
        (
            "a viewer",
            viewer.bearer(),
            manifest(&viewer, PAGE, Some("T"), 0),
            "role_forbidden",
        ),
    ];
    for (case, session, msg, kind) in cases {
        let r = h.call(session, msg).await;
        let pinned = fixture()["refusals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["case"] == case)
            .unwrap_or_else(|| panic!("no refusal {case} in the fixture"))["response"]["error"]
            .clone();
        let got = serde_json::to_value(r.error.as_ref().unwrap()).unwrap();
        assert_eq!(got["data"]["kind"], pinned["data"]["kind"], "{case}");
        assert!(same_shape(&got["data"], &pinned["data"]), "{case}: {got}");
        expect_kind(&r, kind, case);
        assert_eq!(listed(&h).await, before, "{case}");
    }
}
