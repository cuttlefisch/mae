//! Proves issue #460's fix: a real, spawned `mae-daemon` binary actually
//! loads `kb-registry.toml` and opens each registered instance's store at
//! startup, making ADR-060 Phase A's `instance`-addressed RPCs reachable.
//!
//! Before this fix, `daemon/src/main.rs` never read a KB registry or opened
//! any store beyond its own primary — `state.registry` and
//! `state.instance_stores` stayed permanently empty for the lifetime of any
//! real daemon process, so an `instance`-addressed request always resolved
//! to `DaemonError::UnknownInstance` regardless of what was configured on
//! disk. Every existing test of Phase A-D's addressing/quota/isolation
//! machinery constructed `DaemonState` directly in Rust (synthetic,
//! in-process) rather than against a real spawned binary — this is the one
//! that closes that gap.
//!
//! No `MAE_TCP_E2E` gate: this only needs the KB Unix socket (no network
//! port), matching `daemon/benches/kb_dispatch_concurrency.rs`'s own
//! ungated real-daemon-spawn approach for the same socket.
//!
//! Run: `cargo test -p mae-daemon --test federated_instance_startup_e2e`

use std::path::PathBuf;
use std::time::Duration;

use mae_kb::federation::{KbInstance, KbRegistry};
use mae_kb::{CozoKbStore, KbStore, Node, NodeKind};
use serde_json::json;
use tokio::net::UnixStream;

struct DaemonHandle {
    child: std::process::Child,
    socket_path: PathBuf,
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Build a data dir with: a primary store (one distinctive node), a SEPARATE
/// federated instance store (a different distinctive node), and a
/// `kb-registry.toml` registering the instance — exactly the on-disk shape
/// `daemon/src/main.rs` must now read at startup.
fn prepare_multi_instance_data_dir() -> (tempfile::TempDir, String) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();

    // Primary store, opened at the exact path main.rs computes.
    let primary_path = data_dir.join("daemon-kb.cozo");
    let primary = CozoKbStore::open_with_engine(&primary_path, "sqlite").unwrap();
    primary
        .insert_node(&Node::new(
            "primary-only",
            "Primary-only node",
            NodeKind::Note,
            "This node lives ONLY in the primary store.",
        ))
        .unwrap();
    drop(primary);

    // A separate, federated instance store with DIFFERENT content.
    let instances_dir = data_dir.join("instances");
    std::fs::create_dir_all(&instances_dir).unwrap();
    let instance_uuid = "instance-under-test-uuid".to_string();
    let instance_path = instances_dir.join(format!("{instance_uuid}.cozo"));
    let instance_store = CozoKbStore::open_with_engine(&instance_path, "sqlite").unwrap();
    instance_store
        .insert_node(&Node::new(
            "secondary-only",
            "Secondary-only node",
            NodeKind::Note,
            "This node lives ONLY in the federated instance store, never the primary.",
        ))
        .unwrap();
    drop(instance_store);

    // Register the instance -- this is the file main.rs must now read.
    let registry = KbRegistry {
        instances: vec![KbInstance {
            uuid: instance_uuid.clone(),
            name: "team-b-kb".to_string(),
            org_dir: PathBuf::new(),
            db_path: instance_path,
            primary: false,
            enabled: true,
            last_import: None,
            collab_id: None,
            shared: false,
            remote_peers: Vec::new(),
            last_sync: None,
            ai_residency: Default::default(),
            project_root: None,
            kind: Default::default(),
            ingest_policy: Default::default(),
            priority: 0,
            remote_hub: None,
        }],
        ..Default::default()
    };
    registry.save(&data_dir).expect("save kb-registry.toml");

    (tmp, instance_uuid)
}

async fn spawn_daemon(tmp: &tempfile::TempDir) -> DaemonHandle {
    let data_dir = tmp.path().join("data");
    let socket_path = tmp.path().join("mae-daemon.sock");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .args(["--data-dir", data_dir.to_str().unwrap()])
        .env("XDG_RUNTIME_DIR", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn mae-daemon");

    for _ in 0..100 {
        if UnixStream::connect(&socket_path).await.is_ok() {
            return DaemonHandle { child, socket_path };
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Don't leave a zombie/orphaned process behind on the failure path.
    let _ = child.kill();
    let _ = child.wait();
    panic!("mae-daemon did not bind its KB socket within 10s");
}

async fn kb_get(
    socket_path: &std::path::Path,
    id: &str,
    instance: Option<&str>,
) -> serde_json::Value {
    let mut stream = UnixStream::connect(socket_path).await.expect("connect");
    let (r, mut w) = stream.split();
    let mut reader = tokio::io::BufReader::new(r);
    let mut params = json!({"id": id});
    if let Some(inst) = instance {
        params["instance"] = json!(inst);
    }
    let req = json!({"jsonrpc": "2.0", "id": 1, "method": "kb/get", "params": params});
    let body = serde_json::to_vec(&req).unwrap();
    mae_mcp::write_framed(&mut w, &body, Duration::from_secs(5))
        .await
        .unwrap();
    let msg = mae_mcp::read_message(&mut reader)
        .await
        .unwrap()
        .expect("response before EOF");
    serde_json::from_str(&msg).unwrap()
}

/// The core positive proof: an `instance`-addressed `kb/get` against a real
/// spawned daemon reaches the SEPARATE federated store's content, not the
/// primary's -- proving `main.rs` genuinely opened it at startup, not just
/// that the daemon didn't crash.
#[tokio::test]
async fn real_daemon_opens_federated_instances_from_kb_registry_toml_at_startup() {
    let (tmp, instance_uuid) = prepare_multi_instance_data_dir();
    let daemon = spawn_daemon(&tmp).await;

    // Unaddressed request (today's default behavior) reaches the primary.
    let primary_resp = kb_get(&daemon.socket_path, "primary-only", None).await;
    assert!(
        !primary_resp["result"].is_null(),
        "unaddressed kb/get must still find the primary-only node: {primary_resp:?}"
    );

    // Instance-addressed request for a node that ONLY exists in the
    // federated store -- this is the exact property that was broken before
    // the fix: it would have come back UnknownInstance (or Null, depending
    // on error shape), never the real node.
    let instance_resp = kb_get(&daemon.socket_path, "secondary-only", Some(&instance_uuid)).await;
    assert!(
        instance_resp.get("error").is_none(),
        "instance-addressed kb/get must not error -- the instance must resolve: {instance_resp:?}"
    );
    assert!(
        !instance_resp["result"].is_null(),
        "instance-addressed kb/get must find the secondary-only node: {instance_resp:?}"
    );
    assert_eq!(instance_resp["result"]["id"], "secondary-only");

    // Negative control: the primary-only node must NOT be visible when
    // scoped to the instance -- proves real per-instance isolation, not a
    // federated view that happens to see everything.
    let cross_resp = kb_get(&daemon.socket_path, "primary-only", Some(&instance_uuid)).await;
    assert!(
        cross_resp["result"].is_null(),
        "the instance-scoped store must never see the primary's own content: {cross_resp:?}"
    );

    // And the mirror: an unaddressed request DOES see the instance-only
    // node too, because `None` routes to the FEDERATED query layer
    // (rebuild_query_layer's FederatedQuery spanning the primary + every
    // instance_stores entry) -- Phase A's "None preserves today's exact
    // behavior" is about not requiring the new `instance` field, not about
    // limiting scope to the primary alone. This is, in fact, the other half
    // of what this fix makes real: before it, `instance_stores` was always
    // empty, so this "federated" layer only ever had one member and
    // federation was silently a no-op in production regardless of what a
    // deployment's kb-registry.toml said.
    let unaddressed_secondary = kb_get(&daemon.socket_path, "secondary-only", None).await;
    assert!(
        !unaddressed_secondary["result"].is_null(),
        "unaddressed kb/get must find instance-only content via the federated query layer, \
         now that instance_stores is genuinely populated: {unaddressed_secondary:?}"
    );
}
