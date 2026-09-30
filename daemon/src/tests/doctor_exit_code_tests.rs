//! `doctor`'s exit code must mean something.
//!
//! A diagnostic that prints "no client can connect" and exits 0 cannot be used
//! by anything automated: `HEALTHCHECK` reads the exit code and would call the
//! container healthy, and Ansible's `command` module — which fails on a
//! non-zero rc for free — has to scrape stdout instead.

use crate::config::DaemonConfig;
use crate::run_doctor;
use std::path::Path;

/// A config whose every path is inside `tmp`.
///
/// `DaemonConfig::default()` resolves socket, data dirs, identity, keystore and
/// authorized_keys from the real HOME/XDG, and `doctor` OPENS the collab SQLite
/// store it finds there. These tests therefore ran against the CI runner's —
/// and on a workstation, the live daemon's — store; one failed in CI with
/// "database is locked" while a parallel test held it. The isolation is
/// asserted, not assumed, so a new ambient path fails here instead of flaking.
fn isolated(tmp: &Path) -> DaemonConfig {
    let mut c = DaemonConfig::default();
    let s = |rel: &str| tmp.join(rel).to_string_lossy().into_owned();
    c.socket = tmp.join("daemon.sock");
    c.data_dir = Some(tmp.join("data"));
    c.collab.storage.data_dir = Some(tmp.join("collab"));
    c.collab.auth.identity_dir = Some(s("collab"));
    c.collab.auth.authorized_keys = Some(s("collab/authorized_keys"));
    c.collab.auth.keystore = Some(s("collab/trusted_keys"));
    let p = c.instance_paths();
    let mut touched = vec![p.socket, p.data_dir, p.collab_data_dir];
    touched.extend(p.identity_dir);
    touched.extend(p.authorized_keys);
    touched.extend(p.keystore);
    for path in touched {
        assert!(
            path.starts_with(tmp),
            "doctor would touch {} — outside the test's temp dir",
            path.display()
        );
    }
    c
}

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
