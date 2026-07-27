//! ADR-060 Phase G: the daemon's config-change contract, proven empirically
//! rather than assumed. `mae-daemon` has NO live-reload mechanism for any
//! `daemon.toml` section (confirmed directly: `DaemonConfig::load()` runs
//! once at startup in `main.rs` and nothing ever re-reads or watches the
//! file afterward) — this test proves that specifically for `[[tenant]]`
//! entries, the multi-tenancy-relevant config this phase's Verification
//! section names, rather than leaving it as an assumption a reader has to
//! trust. The specific failure mode this falsifies: a config change that
//! silently fails to take effect with no error anywhere, leaving an
//! operator believing a quota/tenant change is active when it is not.
//!
//! Run: `cargo test -p mae-daemon --test config_change_contract_e2e`

use std::path::PathBuf;
use std::time::Duration;

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

async fn spawn_daemon(tmp: &tempfile::TempDir, config_path: &std::path::Path) -> DaemonHandle {
    let data_dir = tmp.path().join("data");
    let socket_path = tmp.path().join("mae-daemon.sock");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .args([
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--config",
            config_path.to_str().unwrap(),
        ])
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
    let _ = child.kill();
    let _ = child.wait();
    panic!("mae-daemon did not bind its KB socket within 10s");
}

async fn kb_get(socket_path: &std::path::Path, id: &str, instance: Option<&str>) -> serde_json::Value {
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

/// The core proof: registering a NEW `[[tenant]]` (with a deliberately tiny
/// quota) in the on-disk config file, while the daemon keeps running, has
/// ZERO effect on that already-running process. A quota tight enough that a
/// live-reloading daemon would reject the 3rd+ request must instead admit
/// ALL of them, because the running process's `TenantRegistry` was built
/// once at startup and never rebuilt.
#[tokio::test]
async fn editing_tenant_config_on_disk_has_no_live_effect_on_a_running_daemon() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();

    // A federated instance to address -- registered via kb-registry.toml,
    // exercising issue #460's fix along the way (this test would not have
    // been meaningfully constructible before that fix: there was no way to
    // get a real daemon to recognize a named instance at all).
    let instances_dir = data_dir.join("instances");
    std::fs::create_dir_all(&instances_dir).unwrap();
    let uuid = "config-contract-instance".to_string();
    let inst_path = instances_dir.join(format!("{uuid}.cozo"));
    let store = CozoKbStore::open_with_engine(&inst_path, "sqlite").unwrap();
    store
        .insert_node(&Node::new(
            "probe-node",
            "Probe node",
            NodeKind::Note,
            "used to exercise repeated kb/get calls against this instance",
        ))
        .unwrap();
    drop(store);
    std::fs::write(
        data_dir.join("kb-registry.toml"),
        format!(
            r#"
[[instances]]
uuid = "{uuid}"
name = "probe-kb"
org_dir = ""
db_path = "{path}"
primary = false
enabled = true
"#,
            path = inst_path.display()
        ),
    )
    .unwrap();

    // Start with NO [[tenant]] entries at all -- zero-config, zero
    // restriction, matching Phase A's own backward-compatibility contract.
    let config_path = tmp.path().join("daemon.toml");
    std::fs::write(&config_path, "").unwrap();

    let daemon = spawn_daemon(&tmp, &config_path).await;

    // Baseline: with no tenant config, requests against the instance are
    // unrestricted (TenantOutcome::Unconfigured).
    for _ in 0..5 {
        let resp = kb_get(&daemon.socket_path, "probe-node", Some(&uuid)).await;
        assert!(resp.get("error").is_none(), "baseline request must succeed: {resp:?}");
    }

    // The config-change under test: register a tenant owning this instance
    // with a budget of exactly 1 point (kb/get costs 1 point per ADR-060
    // Phase C's cost table) -- if this took effect live, the 2nd+ request
    // in the next batch would be rejected with QuotaExceeded.
    std::fs::write(
        &config_path,
        format!(
            r#"
[[tenant]]
name = "late-registered-tenant"
instances = ["{uuid}"]

[tenant.quota]
max_connections = 0
budget_per_minute = 1
"#
        ),
    )
    .unwrap();

    // No restart -- the daemon process is untouched. If a real operator
    // believed this config change "just needs a moment to pick up", this
    // loop simulates them waiting well past any plausible poll interval.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // The property under test: ALL of these must still succeed. A
    // live-reloading daemon would reject request #2 onward (budget=1,
    // cost=1 each) -- if even one of these gets rejected, the config
    // change took effect live and this test (and the "no live-reload"
    // premise Phase G is built on) would be wrong.
    for i in 0..10 {
        let resp = kb_get(&daemon.socket_path, "probe-node", Some(&uuid)).await;
        assert!(
            resp.get("error").is_none(),
            "request {i} after the on-disk tenant-config edit must still succeed -- a running \
             daemon must not live-apply a [[tenant]] change with no restart; if this fails, \
             either live-reload was added (update this test and Phase G's documentation to \
             match) or something else changed the in-memory TenantRegistry unexpectedly: {resp:?}"
        );
    }
}
