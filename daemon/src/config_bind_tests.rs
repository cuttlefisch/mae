//! ADVERSARIAL: an unauthenticated collab port that is reachable off-host.
//!
//! Split out of `config.rs` rather than blessing its growth — the structural
//! ratchet is doing its job, and a security test module is exactly the kind of
//! thing that should not push a config module past its ceiling.

use crate::config::DaemonConfig;

fn cfg(bind: &str, mode: &str) -> DaemonConfig {
    let mut c = DaemonConfig::default();
    c.collab.enabled = true;
    c.collab.bind = bind.parse().unwrap();
    c.collab.auth.mode = mode.to_string();
    c
}

/// An unauthenticated session reaches `kb_access_with_coll` with
/// `principal == None`, which returns `Allow`. So a non-loopback bind under
/// `mode = "none"` grants Manage on every KB to every host that can reach
/// the port — and `mode` DEFAULTS to "none".
///
/// The oracle is that the config is rejected, and that the message names
/// both halves: an operator who sees only "bad config" will re-read the
/// wrong line.
#[test]
fn an_unauthenticated_off_host_bind_is_refused() {
    for (bind, mode) in [
        ("0.0.0.0:9473", "none"),
        ("0.0.0.0:9473", "psk"),
        ("[::]:9473", "none"),
        ("10.0.0.5:9473", "none"),
        ("10.0.0.5:9473", "psk"),
    ] {
        let issues = cfg(bind, mode).check_collab();
        assert!(
            issues.iter().any(|i| i.contains("reachable off-host")),
            "bind={bind} mode={mode} must be refused, got: {issues:?}"
        );
    }
}

/// Key auth without TLS authenticates the handshake, then carries every KB
/// document in the clear: off-host it is refused too, for every off-host
/// address shape, and the message names the `tls` setting.
#[test]
fn key_auth_without_tls_off_host_is_refused() {
    for bind in [
        "0.0.0.0:9473",
        "[::]:9473",
        "10.0.0.5:9473",
        "[2001:db8::1]:9473",
    ] {
        let mut c = cfg(bind, "key");
        c.collab.auth.tls = false;
        let issues = c.check_collab();
        assert!(
            issues
                .iter()
                .any(|i| i.contains("reachable off-host") && i.contains("tls")),
            "bind={bind} key without TLS must be refused, got: {issues:?}"
        );
    }
}

/// On loopback, key auth without TLS stays allowed (local development,
/// tests), as do the other modes.
#[test]
fn key_auth_without_tls_on_loopback_is_accepted() {
    for bind in ["127.0.0.1:9473", "[::1]:9473"] {
        let mut c = cfg(bind, "key");
        c.collab.auth.tls = false;
        let issues = c.check_collab();
        assert!(
            !issues.iter().any(|i| i.contains("reachable off-host")),
            "bind={bind} must be accepted, got: {issues:?}"
        );
    }
}

/// The three configurations that must NOT be refused. Without these the
/// check above would be satisfied by a function that rejects everything,
/// and loopback development would be broken.
#[test]
fn loopback_and_key_mode_are_accepted() {
    for (bind, mode) in [
        ("127.0.0.1:9473", "none"),
        ("127.0.0.1:9473", "psk"),
        ("[::1]:9473", "none"),
        ("0.0.0.0:9473", "key"),
        ("10.0.0.5:9473", "key"),
    ] {
        let issues = cfg(bind, mode).check_collab();
        assert!(
            !issues.iter().any(|i| i.contains("reachable off-host")),
            "bind={bind} mode={mode} must be accepted, got: {issues:?}"
        );
    }
}

/// Disabling collab must not produce a bind complaint about a listener that
/// never starts.
#[test]
fn a_disabled_collab_listener_is_not_flagged() {
    let mut c = cfg("0.0.0.0:9473", "none");
    c.collab.enabled = false;
    assert!(!c
        .check_collab()
        .iter()
        .any(|i| i.contains("reachable off-host")));
}
