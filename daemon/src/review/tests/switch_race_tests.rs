//! ADR-113 O17 under a race: a `kb/review.request` and a switch of the KB to
//! E2E, driven step by step, never leave a plaintext revision on an E2E KB.
//! The request takes the KB's switch gate across its E2E re-check, its freezes
//! and its commit, and every collection write takes it too. Two orders: the
//! request paused inside its freeze with the switch let loose (the switch
//! waits, then sees the revision and is refused), and the switch paused
//! mid-write with the request let loose (the request waits, then sees the KB
//! is E2E and is refused). Kills a request that does not hold the gate, and
//! one that does not re-check under it.

use std::time::Duration;

use mae_sync::kb::Encryption;

use super::*;
use crate::doc_store::ApplyResult;
use crate::storage::{PlaintextHeld, StorageError};

const WINDOW: Duration = Duration::from_millis(200);

/// The owner's flip of the KB to E2E, as a collection write.
fn flip(store: &Arc<DocStore>) -> tokio::task::JoinHandle<Result<ApplyResult, StorageError>> {
    let store = Arc::clone(store);
    tokio::spawn(async move {
        let kbc = format!("kbc:{KB}");
        let state = store.encode_state(&kbc).await.unwrap();
        let up = KbCollectionDoc::from_bytes(&state)
            .unwrap()
            .set_encryption(Encryption::E2e);
        store
            .apply_update(&kbc, &up, None, Attribution::Principal("owner"))
            .await
    })
}

fn ask(env: &Env, v: &str) -> tokio::task::JoinHandle<Result<Value, McpError>> {
    let (store, who, v) = (Arc::clone(&env.store), env.editor.clone(), v.to_string());
    tokio::spawn(async move {
        call_on(
            &store,
            &who,
            "kb/review.request",
            json!({"kb_id": KB, "nodes": [{"node_id": "n1", "version": v}], "reason": "r"}),
        )
        .await
    })
}

async fn is_e2e(env: &Env) -> bool {
    let state = env.store.encode_state(&format!("kbc:{KB}")).await.unwrap();
    matches!(
        KbCollectionDoc::from_bytes(&state).unwrap().encryption(),
        Encryption::E2e
    )
}

async fn revisions(env: &Env) -> usize {
    env.store
        .review_read(|c| crate::review::store::node_revisions(c, KB, "n1"))
        .await
        .unwrap()
        .len()
}

#[tokio::test]
async fn a_switch_racing_a_paused_request_waits_and_is_refused() {
    let env = env().await;
    enable(&env).await;
    let v = version(&env, "n1").await;
    let pause = env.store.pause_next(|p| &p.freeze);
    let req = ask(&env, &v);
    pause.reached.notified().await;
    let mut switch = flip(&env.store);
    let early = tokio::time::timeout(WINDOW, &mut switch).await;
    pause.proceed.notify_one();
    req.await.unwrap().unwrap();
    let finished_inside = early.is_ok();
    let switched = match early {
        Ok(r) => r.unwrap(),
        Err(_) => switch.await.unwrap(),
    };
    assert!(!finished_inside, "the switch landed inside the request");
    assert!(
        matches!(
            switched,
            Err(StorageError::E2eSwitchRefused {
                held: PlaintextHeld::Revisions,
                ..
            })
        ),
        "{switched:?}"
    );
    assert_eq!(revisions(&env).await, 1);
    assert!(!is_e2e(&env).await, "a plaintext revision on an E2E KB");
}

#[tokio::test]
async fn a_request_racing_a_paused_switch_waits_and_is_refused() {
    let env = env().await;
    enable(&env).await;
    let v = version(&env, "n1").await;
    let pause = env.store.pause_next(|p| &p.apply);
    let switch = flip(&env.store);
    pause.reached.notified().await;
    let mut req = ask(&env, &v);
    let early = tokio::time::timeout(WINDOW, &mut req).await;
    pause.proceed.notify_one();
    switch.await.unwrap().unwrap();
    let finished_inside = early.is_ok();
    let asked = match early {
        Ok(r) => r.unwrap(),
        Err(_) => req.await.unwrap(),
    };
    assert!(!finished_inside, "the request committed inside the switch");
    assert_eq!(err_kind(asked), "e2e_unsupported");
    assert!(is_e2e(&env).await);
    assert_eq!(
        revisions(&env).await,
        0,
        "a plaintext revision on an E2E KB"
    );
}
