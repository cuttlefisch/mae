//! `doctor`'s exit code must mean something.
//!
//! A diagnostic that prints "no client can connect" and exits 0 cannot be used
//! by anything automated: `HEALTHCHECK` reads the exit code and would call the
//! container healthy, and Ansible's `command` module — which fails on a
//! non-zero rc for free — has to scrape stdout instead.

use super::isolated;
use crate::run_doctor;

/// A configuration that cannot serve a client must exit non-zero.
#[test]
fn doctor_fails_when_the_configuration_stops_clients_connecting() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut config = isolated(tmp.path());
    // Two independently invalid settings, so the assertion cannot pass on a
    // single hard-coded branch.
    config.collab.storage.compact_threshold = 0;
    config.collab.storage.backend = "postgres".to_string();
    assert!(
        !config.check_collab().is_empty(),
        "fixture is vacuous: this config must actually be invalid, or the \
         exit-code assertion below proves nothing"
    );

    assert_eq!(
        run_doctor(&config, None),
        1,
        "doctor reported collab issues and still exited 0 — every automated \
         consumer of this command treats that as healthy"
    );
}

/// The healthy case must stay 0, or `doctor` becomes useless in the other
/// direction: a gate that always fails gets disabled.
#[test]
fn doctor_succeeds_on_a_configuration_with_no_problems() {
    let tmp = tempfile::TempDir::new().unwrap();
    let config = isolated(tmp.path());
    assert!(config.check_collab().is_empty(), "fixture assumption");
    assert_eq!(run_doctor(&config, None), 0);
}

/// A collab port already bound is the daemon ITSELF, not a fault.
///
/// This is the case that makes a naive "any bad news is a failure" rule wrong:
/// running `doctor` against a healthy, RUNNING instance finds its port taken.
/// If that counted, doctor would fail precisely when the service is working,
/// and `mae_daemon`'s own verify play asserts the opposite — it treats
/// `available` as the failure signal, because a bound port means the daemon is
/// up.
#[test]
fn a_port_already_bound_is_not_a_problem() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a throwaway port");
    let taken = listener.local_addr().unwrap();

    let tmp = tempfile::TempDir::new().unwrap();
    let mut config = isolated(tmp.path());
    config.collab.bind = taken;
    assert!(config.check_collab().is_empty(), "fixture assumption");

    assert_eq!(
        run_doctor(&config, None),
        0,
        "doctor treated an in-use collab port as a problem — that is the \
         signature of a RUNNING daemon, so this would fail whenever the \
         service is actually healthy"
    );
}

/// `doctor` runs beside a live daemon, so it must not write the store: where
/// there is none it creates none, and beside a live one (its WAL unchecked)
/// it leaves both files byte for byte. It used to open the store as the daemon
/// does, creating it, starting a second checkpointer and running schema DDL
/// outside the daemon's writer slot. Kills that open.
#[test]
fn doctor_creates_no_store_and_writes_nothing_beside_a_live_one() {
    use mae_daemon::storage::{CheckpointPolicy, SqliteBackend, StorageBackend};
    let tmp = tempfile::TempDir::new().unwrap();
    let config = isolated(tmp.path());
    let db = config.collab_data_dir().join("state.db");
    assert_eq!(run_doctor(&config, None), 0);
    assert!(!db.exists(), "doctor created the store");
    std::fs::create_dir_all(config.collab_data_dir()).unwrap();

    // A live store whose checkpointer never wakes, so only doctor could
    // change its files.
    let paused = CheckpointPolicy {
        interval: std::time::Duration::from_secs(3600),
        idle_truncate_after: std::time::Duration::from_secs(3600),
        ..CheckpointPolicy::PRODUCTION
    };
    let live = SqliteBackend::open_with_policy(&db, 4, paused).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    for i in 0..20 {
        rt.block_on(live.add_block(&format!("kb{i}"), "SHA256:p"))
            .unwrap();
    }
    let wal = config.collab_data_dir().join("state.db-wal");
    let files = || (std::fs::read(&db).unwrap(), std::fs::read(&wal).unwrap());
    let before = files();
    assert!(
        !before.1.is_empty(),
        "precondition: the live store has a WAL"
    );
    assert_eq!(run_doctor(&config, None), 0);
    assert!(files() == before, "doctor wrote to the live store");
    super::close_within_seconds(live);
}
