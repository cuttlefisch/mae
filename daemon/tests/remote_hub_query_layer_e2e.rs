//! ADR-062 Phase D e2e: drives a real `mae_kb::remote_hub::RemoteHubQueryLayer` (the
//! client side, blocking HTTP) against a real spawned `mae-daemon` OAuth+`kb/query.*`
//! listener (the server side) — the actual bridge this ADR's Phase D exists to build,
//! proven over the real wire rather than in-process.
//!
//! Reuses `daemon/tests/oauth_e2e.rs`'s exact daemon-spawning shape (real self-signed TLS
//! via `rcgen`, a real mock JWKS HTTP server, real RS256-signed JWTs via `jsonwebtoken` —
//! CLAUDE.md principle #14: real crypto/keys per run, never a shared hardcoded test
//! token) — duplicated rather than factored into a shared `tests/common` module so this
//! file stays a self-contained addition with zero risk to the already-passing
//! `oauth_e2e.rs` suite.
//!
//! `RemoteHubQueryLayer` is a *blocking* client (its whole point is to be a drop-in
//! `KbQueryLayer`, whose trait methods are synchronous by design — see the module's own
//! doc comment), so this file uses plain `#[test]` + `std::process::Command`/
//! `std::net::TcpStream`, not `#[tokio::test]` — calling a blocking client from inside a
//! tokio runtime without `spawn_blocking` is exactly the kind of mismatch that produces
//! confusing panics, so the whole test stays off the async runtime entirely.
//!
//! Scope: the ADR's own named adversarial bar for Phase D — an expired/revoked token
//! produces a clean, *observable* auth failure (never a silent empty result
//! indistinguishable from "the hub has nothing"), and the Hard Rule (a `RemoteHubQueryLayer`
//! never caches — two calls against changed hub state always reflect the latest). Response-
//! parsing/translation-boundary correctness (malformed/oversized payloads, well-formed
//! get/search round-trips) is covered separately in `shared/kb/src/remote_hub.rs`'s own
//! unit tests against a minimal protocol-accurate mock, which doesn't need a real daemon.

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use mae_kb::federation::{RemoteHubAuth, RemoteHubConfig};
use mae_kb::query::KbQueryLayer;
use mae_kb::remote_hub::{LastOutcome, RemoteHubQueryLayer};
use rsa::pkcs1::EncodeRsaPrivateKey;
use rsa::traits::PublicKeyParts;
use rsa::RsaPrivateKey;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::time::Duration;

const TEST_KID: &str = "e2e-remote-hub-key";
const CANONICAL_RESOURCE: &str = "https://127.0.0.1/mcp";
const TEST_ISSUER: &str = "https://idp.example.com";

fn base64_url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Fresh per-run RSA keypair (CLAUDE.md principle #14 — never a shared/hardcoded key).
fn generate_key_material() -> (String, serde_json::Value) {
    // Use the RNG from `rsa`'s OWN `rand_core`, not the workspace `rand`.
    // The daemon is on rand 0.10 (rand_core 0.10) while `rsa` still wants
    // rand_core 0.6's traits, so a `rand::rng()` handle does not satisfy
    // `CryptoRngCore` -- two rand_core versions in one graph. `OsRng` is also
    // the right choice on merit for key generation.
    let mut rng = rsa::rand_core::OsRng;
    let private_key = RsaPrivateKey::new(&mut rng, 2048).expect("RSA keygen");
    let pem = private_key
        .to_pkcs1_pem(rsa::pkcs8::LineEnding::LF)
        .expect("PEM encode")
        .to_string();
    let public_key = private_key.to_public_key();
    let n = base64_url(&public_key.n().to_bytes_be());
    let e = base64_url(&public_key.e().to_bytes_be());
    let jwks = serde_json::json!({
        "keys": [{"kid": TEST_KID, "n": n, "e": e, "kty": "RSA", "alg": "RS256", "use": "sig"}]
    });
    (pem, jwks)
}

fn sign_token(private_key_pem: &str, claims: &serde_json::Value) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(TEST_KID.to_string());
    let encoding_key = EncodingKey::from_rsa_pem(private_key_pem.as_bytes()).expect("valid PEM");
    encode(&header, claims, &encoding_key).expect("sign")
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn valid_claims() -> serde_json::Value {
    let now = now_unix();
    serde_json::json!({
        "sub": "alice@example.com",
        "aud": CANONICAL_RESOURCE,
        "iss": TEST_ISSUER,
        "iat": now,
        "exp": now + 3600,
    })
}

/// Minimal synchronous raw-TCP mock JWKS server (mirrors `oauth_e2e.rs`'s async version,
/// but blocking to match this file's fully-synchronous test style).
fn spawn_mock_jwks_server(jwks: &serde_json::Value) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let body = jwks.to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let body = body.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.shutdown(std::net::Shutdown::Both);
            });
        }
    });
    addr
}

fn generate_self_signed_cert(cert_path: &Path, key_path: &Path) {
    let cert_key = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()])
        .expect("rcgen self-signed cert");
    std::fs::write(cert_path, cert_key.cert.pem()).unwrap();
    std::fs::write(key_path, cert_key.signing_key.serialize_pem()).unwrap();
}

fn free_tcp_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

struct DaemonGuard {
    child: std::process::Child,
    _tmp: tempfile::TempDir,
    oauth_addr: SocketAddr,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_daemon_with_oauth(jwks_addr: SocketAddr) -> DaemonGuard {
    let tmp = tempfile::tempdir().unwrap();
    let cert_path = tmp.path().join("oauth.crt");
    let key_path = tmp.path().join("oauth.key");
    generate_self_signed_cert(&cert_path, &key_path);

    let collab_port = free_tcp_port();
    let oauth_port = free_tcp_port();
    let oauth_addr: SocketAddr = format!("127.0.0.1:{oauth_port}").parse().unwrap();

    let config_toml = format!(
        r#"
[collab]
enabled = true
bind = "127.0.0.1:{collab_port}"

[oauth]
enabled = true
bind = "127.0.0.1:{oauth_port}"
canonical_resource_uri = "{CANONICAL_RESOURCE}"
jwks_url = "http://127.0.0.1:{jwks_port}/jwks"
issuer = "{TEST_ISSUER}"
principal_claim = "sub"
cert_path = "{cert_path}"
key_path = "{key_path}"
kb_query_enabled = true
max_request_body_bytes = 65536
kb_query_max_body_bytes = 65536
kb_query_max_scan_nodes = 500
kb_query_max_search_results = 20
max_connections = 0
"#,
        collab_port = collab_port,
        oauth_port = oauth_port,
        jwks_port = jwks_addr.port(),
        cert_path = cert_path.display(),
        key_path = key_path.display(),
    );
    let config_path = tmp.path().join("daemon.toml");
    std::fs::write(&config_path, config_toml).unwrap();

    let child = std::process::Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "--data-dir",
            tmp.path().to_str().unwrap(),
        ])
        .env("XDG_RUNTIME_DIR", tmp.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn mae-daemon");

    // Guard constructed the instant spawn() returns, BEFORE the polling loop
    // and its `assert!` below -- previously the bare `child` lived through
    // both, so a daemon that never accepted a connection (the exact case the
    // assert exists to catch) panicked with the process still unguarded and
    // leaked it. `DaemonGuard`'s `Drop` now covers this whole function from
    // here on, regardless of how it returns.
    let guard = DaemonGuard {
        child,
        _tmp: tmp,
        oauth_addr,
    };

    // Bounded by wall-clock, not by an iteration count — see `mae_mcp::ready`.
    // The *blocking* variant: this file is deliberately `#[test]`, not
    // `#[tokio::test]` (see the module doc), so it cannot use the async one.
    let connected = mae_mcp::ready::wait_until_blocking(|| TcpStream::connect(oauth_addr).is_ok());
    assert!(
        connected,
        "{}",
        mae_mcp::ready::timeout_message(&format!("mae-daemon's OAuth listener on {oauth_addr}"))
    );

    guard
}

fn remote_hub_config(base_url: String, auth: RemoteHubAuth) -> RemoteHubConfig {
    RemoteHubConfig {
        base_url,
        hub_kb_id: "nonexistent-kb".to_string(),
        auth,
    }
}

/// ADR-062 Phase D's own named adversarial bar: an expired/revoked token must produce a
/// clean, OBSERVABLE auth failure — never a silent empty result indistinguishable from
/// "the hub legitimately has nothing." Exercised over the real wire against the real
/// daemon's real token validation (same 401 path `oauth_e2e.rs` proves at the transport
/// level), then through the actual client this ADR ships.
#[test]
fn expired_token_produces_a_clean_observable_auth_failure_not_a_silent_empty_result() {
    let (private_key_pem, jwks) = generate_key_material();
    let jwks_addr = spawn_mock_jwks_server(&jwks);
    let daemon = spawn_daemon_with_oauth(jwks_addr);
    let base_url = format!("https://{}", daemon.oauth_addr);

    let mut expired_claims = valid_claims();
    expired_claims["exp"] = serde_json::json!(now_unix().saturating_sub(3600));
    let expired_token = sign_token(&private_key_pem, &expired_claims);

    // `RemoteHubAuth::Command` resolves the token by running a shell command — `echo` is
    // a real, minimal stand-in for "a command that prints a bearer token", proving the
    // Command auth-resolution path end to end, not just KeystoreKey.
    let config = remote_hub_config(
        base_url,
        RemoteHubAuth::Command(format!("echo {expired_token}")),
    );
    let layer = insecure_layer(config);

    let node = layer.get("some-node");
    assert!(
        node.is_none(),
        "an expired token must not return content (it never even reaches dispatch)"
    );
    assert!(
        matches!(layer.last_outcome(), LastOutcome::AuthFailed(_)),
        "expected AuthFailed, got {:?} — a silent-empty-result-with-no-diagnosis is exactly \
         the failure mode this test exists to catch",
        layer.last_outcome()
    );

    let hits = layer.search("anything", 10).unwrap();
    assert!(hits.is_empty());
    assert!(matches!(layer.last_outcome(), LastOutcome::AuthFailed(_)));
}

/// A validly-signed token against a KB the hub doesn't have must degrade gracefully
/// (dispatch's own JSON-RPC `error` response, not an auth failure, not a panic, not a
/// hang) — proves the two failure classes (auth-layer vs. dispatch-layer) are genuinely
/// distinguished by `last_outcome()`, not conflated into one generic "it didn't work".
#[test]
fn valid_token_against_a_nonexistent_kb_degrades_gracefully_and_is_distinct_from_auth_failure() {
    let (private_key_pem, jwks) = generate_key_material();
    let jwks_addr = spawn_mock_jwks_server(&jwks);
    let daemon = spawn_daemon_with_oauth(jwks_addr);
    let base_url = format!("https://{}", daemon.oauth_addr);

    let valid_token = sign_token(&private_key_pem, &valid_claims());
    let config = remote_hub_config(
        base_url,
        RemoteHubAuth::Command(format!("echo {valid_token}")),
    );
    let layer = insecure_layer(config);

    let node = layer.get("some-node");
    assert!(node.is_none());
    assert!(
        !matches!(layer.last_outcome(), LastOutcome::AuthFailed(_)),
        "a valid token reaching a real (if empty) dispatch layer must not be classified as \
         an auth failure — got {:?}",
        layer.last_outcome()
    );
}

/// The Hard Rule (ADR-062): a `RemoteHubQueryLayer` never caches — it queries live on
/// every call. Since standing up real collaboratively-seeded hub content over the wire
/// is out of scope for this focused e2e file (see module doc comment — covered by
/// `daemon/src/tests/kb_query_tests.rs` in-process instead), this proves the Hard Rule
/// the other direction: `last_outcome()` genuinely reflects the OUTCOME OF THE MOST
/// RECENT call, not a stale/cached value from an earlier one — an auth failure followed
/// by a successfully-dispatched (if content-empty) call must show the latest state, not
/// the first one, which would be the observable symptom of accidental caching.
#[test]
fn last_outcome_reflects_the_most_recent_call_never_a_stale_cached_one() {
    let (private_key_pem, jwks) = generate_key_material();
    let jwks_addr = spawn_mock_jwks_server(&jwks);
    let daemon = spawn_daemon_with_oauth(jwks_addr);
    let base_url = format!("https://{}", daemon.oauth_addr);

    let mut expired_claims = valid_claims();
    expired_claims["exp"] = serde_json::json!(now_unix().saturating_sub(3600));
    let expired_token = sign_token(&private_key_pem, &expired_claims);
    let valid_token = sign_token(&private_key_pem, &valid_claims());

    // Auth resolves via a script file whose content this test flips between calls —
    // proving the token (and therefore the outcome) is re-resolved fresh EVERY call,
    // never cached from the layer's construction.
    let tmp = tempfile::tempdir().unwrap();
    let script_path = tmp.path().join("get_token.sh");
    std::fs::write(&script_path, format!("#!/bin/sh\necho {expired_token}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let config = remote_hub_config(
        base_url,
        RemoteHubAuth::Command(script_path.to_string_lossy().to_string()),
    );
    let layer = insecure_layer(config);

    layer.get("x");
    assert!(matches!(layer.last_outcome(), LastOutcome::AuthFailed(_)));

    // Flip the script to emit a valid token, then call again.
    std::fs::write(&script_path, format!("#!/bin/sh\necho {valid_token}\n")).unwrap();
    layer.get("x");
    assert!(
        !matches!(layer.last_outcome(), LastOutcome::AuthFailed(_)),
        "after the auth command starts returning a valid token, the NEXT call must reflect \
         that immediately — a stale cached AuthFailed would mean the client is caching the \
         token or the outcome instead of re-resolving live, per call, every time"
    );
}

/// This test's daemon serves a locally-generated self-signed cert (`generate_self_signed_cert`
/// above) — `with_timeout_and_insecure_tls_for_testing` is `RemoteHubQueryLayer`'s
/// explicit, clearly-named test-only escape hatch for exactly this (see its doc comment);
/// this test is the cert's own issuer, so there's no CA chain to validate against.
fn insecure_layer(config: RemoteHubConfig) -> RemoteHubQueryLayer {
    RemoteHubQueryLayer::with_timeout_and_insecure_tls_for_testing(config, Duration::from_secs(5))
}
