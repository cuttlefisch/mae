//! Configuration guards that refuse a dangerous deployment shape.
//!
//! Separate from `config.rs` because that file is already a tracked
//! ceiling exception, and because these are *security* refusals rather than
//! field-validity checks — they answer "is this safe to expose", not "is this
//! well-formed".

use crate::config::CollabConfig;
use std::net::SocketAddr;

/// Whether a bind address is loopback-only, i.e. unreachable from another host.
/// `0.0.0.0`/`::` are explicitly NOT loopback — they are the wildcard binds,
/// which is exactly the case this exists to catch.
fn bind_is_loopback(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback()
}

/// Refuse an unauthenticated collab port that is reachable off-host.
///
/// @ai-caution: [security] `AuthConfig::default().mode` is `"none"`, and an
/// unauthenticated session reaches `kb_access_with_coll` with
/// `principal == None`, which returns `AccessDecision::Allow`. So
/// `--bind 0.0.0.0` on a stock config granted Manage on every KB to every host
/// that could reach the port, while `doctor` printed "collab config: OK".
/// `psk` is plaintext on the wire and no better off-host. Nor is `key` with
/// `tls = false`: its JSON handshake authenticates the peers, but everything
/// after it, every KB document included, crosses the network in the clear and
/// unprotected against tampering. Off-host, collab requires key auth over TLS.
///
/// This is an ERROR, not a warning: a warning gets read past, and the shipped
/// `assets/daemon-config.toml` has no `[collab.auth]` block at all while
/// DAEMON_ADMIN tells operators to start from it.
pub fn unauthenticated_bind_issues(c: &CollabConfig) -> Vec<String> {
    if !c.enabled || bind_is_loopback(&c.bind) {
        return Vec::new();
    }
    if c.auth.mode != "key" {
        return vec![format!(
            "collab.bind is {} (reachable off-host) but collab.auth.mode is \
             '{}' — that accepts any client that can reach the port. Set \
             [collab.auth] mode = \"key\" (Ed25519 mTLS), or bind to loopback \
             and put a reverse proxy or VPN in front.",
            c.bind, c.auth.mode
        )];
    }
    if !c.auth.tls {
        return vec![format!(
            "collab.bind is {} (reachable off-host) but collab.auth.tls is \
             false — sessions would cross the network unencrypted. Set \
             [collab.auth] tls = true, or bind to loopback and put a reverse \
             proxy or VPN in front.",
            c.bind
        )];
    }
    Vec::new()
}
