//! Real TLS + real HTTP + real signed-JWT e2e for the OAuth resource server
//! (ADR-052, Phase F) and `kb/query.*` (ADR-053, Phase G).
//!
//! Every existing test for these phases (`daemon/src/oauth.rs`'s own
//! `#[cfg(test)] mod tests`, `daemon/src/tests/kb_query_tests.rs`) drives
//! internal Rust functions in-process — real crypto and a real `DocStore`,
//! but never a real TCP+TLS handshake or a real HTTP request over the wire.
//! A QA pass on this epic flagged that gap explicitly. This test spawns the
//! real `mae-daemon` binary (`env!("CARGO_BIN_EXE_mae-daemon")`) with a real
//! self-signed TLS cert (`rcgen`, the same crate `shared/mcp/src/tls.rs`
//! already uses for mTLS test certs), a real local mock JWKS HTTP server,
//! and real RS256-signed JWTs (the same token-generation approach
//! `oauth.rs`'s own unit tests use, just carried over the real wire this
//! time instead of validated in-process).
//!
//! `daemon/tests/*.rs` integration tests only see `mae_daemon`'s public LIB
//! re-exports (`oauth`/`kb_query`/`handler` are bin-crate-private by design
//! — see `daemon/src/tests/mod.rs`'s own doc comment) — this is a genuine
//! black-box test over the real wire protocol, not a workaround for a
//! missing export.
//!
//! **Scope**: proves the TRANSPORT layer this epic's existing tests
//! structurally cannot (a real TLS handshake succeeds, real bearer-token-
//! over-HTTPS parsing, the real 401/413/PRM-endpoint responses over the
//! wire). Deliberately does NOT re-seed a real KB over the wire to re-prove
//! `kb_query`'s own business logic — that's already thoroughly covered
//! in-process by `kb_query_tests.rs` with real `DocStore` and crypto;
//! requesting a nonexistent KB here still proves the auth layer accepted
//! the token (a non-401 response reaching `kb_query::dispatch`), which is
//! the actual, previously-unproven thing.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use rsa::pkcs1::EncodeRsaPrivateKey;
use rsa::traits::PublicKeyParts;
use rsa::RsaPrivateKey;

const TEST_KID: &str = "e2e-test-key";
const CANONICAL_RESOURCE: &str = "https://127.0.0.1/mcp";
const TEST_ISSUER: &str = "https://idp.example.com";

fn base64_url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Generate a fresh RSA keypair (real, per-run — CLAUDE.md principle #14,
/// never a shared/hardcoded test key) plus the JWKS document and signing
/// PEM matching it.
fn generate_key_material() -> (String, serde_json::Value) {
    let mut rng = rand::thread_rng();
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

/// Minimal raw-TCP mock JWKS server: any request gets the same fixed JSON
/// body. No framework needed for something this simple — this test isn't
/// exercising the mock server itself, so a hand-rolled response beats
/// pulling a `hyper::service` stack into a harness whose only job is
/// standing in for an external IdP's JWKS endpoint.
async fn spawn_mock_jwks_server(jwks: &serde_json::Value) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let body = jwks.to_string();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let body = body.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    addr
}

/// Generate a self-signed TLS cert+key (rcgen — the same crate
/// `shared/mcp/src/tls.rs` uses for mTLS test certs) for `127.0.0.1`,
/// PEM-encoded to `cert_path`/`key_path`.
fn generate_self_signed_cert(cert_path: &Path, key_path: &Path) {
    let cert_key = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()])
        .expect("rcgen self-signed cert");
    std::fs::write(cert_path, cert_key.cert.pem()).unwrap();
    std::fs::write(key_path, cert_key.signing_key.serialize_pem()).unwrap();
}

fn free_tcp_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

struct DaemonGuard {
    _child: tokio::process::Child,
    _tmp: tempfile::TempDir,
    oauth_addr: SocketAddr,
}

/// Spawn a real `mae-daemon` with a real OAuth listener (collab enabled so
/// a `DocStore` exists for `kb/query.*`, TLS cert/key on disk, JWKS pointed
/// at the mock server above) and wait for it to actually accept TLS
/// connections before returning.
async fn spawn_daemon_with_oauth(jwks_addr: SocketAddr) -> DaemonGuard {
    spawn_daemon_with_oauth_capped(jwks_addr, 0).await
}

/// Same as `spawn_daemon_with_oauth`, with an explicit `max_connections`
/// (`0` = unlimited, matching `ConnLimiter`'s own convention).
async fn spawn_daemon_with_oauth_capped(
    jwks_addr: SocketAddr,
    max_connections: usize,
) -> DaemonGuard {
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
max_request_body_bytes = 200
kb_query_max_body_bytes = 65536
kb_query_max_scan_nodes = 500
kb_query_max_search_results = 20
max_connections = {max_connections}
"#,
        collab_port = collab_port,
        oauth_port = oauth_port,
        jwks_port = jwks_addr.port(),
        cert_path = cert_path.display(),
        key_path = key_path.display(),
        max_connections = max_connections,
    );
    let config_path = tmp.path().join("daemon.toml");
    std::fs::write(&config_path, config_toml).unwrap();

    let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "--data-dir",
            tmp.path().to_str().unwrap(),
        ])
        .env("XDG_RUNTIME_DIR", tmp.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to spawn mae-daemon");

    // Wait for the OAuth listener to actually accept a TCP connection
    // (TLS handshake happens per-request below, not needed for this probe).
    let mut connected = false;
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(oauth_addr).await.is_ok() {
            connected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        connected,
        "mae-daemon's OAuth listener never accepted a connection on {oauth_addr} within 10s"
    );

    DaemonGuard {
        _child: child,
        _tmp: tmp,
        oauth_addr,
    }
}

/// A `reqwest` client that trusts the test's own self-signed cert (via
/// `danger_accept_invalid_certs` — appropriate here since this test IS the
/// cert's issuer and there's no CA chain to validate against; a real
/// deployment uses a CA-issued cert).
fn insecure_https_client() -> reqwest::Client {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap()
}

#[tokio::test]
async fn oauth_and_kb_query_over_a_real_tls_connection() {
    let (private_key_pem, jwks) = generate_key_material();
    let jwks_addr = spawn_mock_jwks_server(&jwks).await;
    let daemon = spawn_daemon_with_oauth(jwks_addr).await;
    let base_url = format!("https://{}", daemon.oauth_addr);
    let client = insecure_https_client();

    // 1. The PRM document is served unauthenticated, over a real TLS
    // handshake.
    let prm_resp = client
        .get(format!("{base_url}/.well-known/oauth-protected-resource"))
        .send()
        .await
        .expect("PRM request over real TLS");
    assert_eq!(prm_resp.status(), 200);
    let prm_body: serde_json::Value = prm_resp.json().await.unwrap();
    assert_eq!(prm_body["resource"], CANONICAL_RESOURCE);

    // 2. Missing bearer token -> 401 + WWW-Authenticate, over the real wire.
    let no_token_resp = client
        .get(&base_url)
        .send()
        .await
        .expect("no-token request");
    assert_eq!(no_token_resp.status(), 401);
    assert!(
        no_token_resp
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .is_some(),
        "expected WWW-Authenticate on a real 401 response"
    );

    // 3. A validly-signed token reaches the real dispatch layer (not a
    // 401) — the actual, previously-unproven "real bearer-token-over-wire
    // parsing" property. The KB doesn't exist (no seeding over the wire —
    // see module doc), so the RESULT is an access-denied JSON-RPC error,
    // but getting THERE at all proves the token was accepted.
    let valid_token = sign_token(&private_key_pem, &valid_claims());
    let kb_query_body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "kb/query.capabilities",
        "params": {"kb_id": "nonexistent-kb"}
    });
    let valid_resp = client
        .post(&base_url)
        .bearer_auth(&valid_token)
        .json(&kb_query_body)
        .send()
        .await
        .expect("valid-token request");
    assert_eq!(
        valid_resp.status(),
        200,
        "a validly-signed token must reach dispatch (never a 401), regardless of the KB's own existence"
    );
    let valid_body: serde_json::Value = valid_resp.json().await.unwrap();
    assert!(
        valid_body.get("error").is_some(),
        "a nonexistent KB is a JSON-RPC error, but from dispatch, not an auth failure: {valid_body}"
    );

    // 4. Wrong-audience token -> 401, over the real wire (the confused-
    // deputy defense, RFC 8707, previously only proven in-process).
    let mut wrong_aud_claims = valid_claims();
    wrong_aud_claims["aud"] = serde_json::json!("https://a-different-mcp-server.example.com/mcp");
    let wrong_aud_token = sign_token(&private_key_pem, &wrong_aud_claims);
    let wrong_aud_resp = client
        .get(&base_url)
        .bearer_auth(&wrong_aud_token)
        .send()
        .await
        .expect("wrong-audience request");
    assert_eq!(wrong_aud_resp.status(), 401);

    // 5. Expired token -> 401, over the real wire.
    let mut expired_claims = valid_claims();
    expired_claims["exp"] = serde_json::json!(now_unix().saturating_sub(3600));
    let expired_token = sign_token(&private_key_pem, &expired_claims);
    let expired_resp = client
        .get(&base_url)
        .bearer_auth(&expired_token)
        .send()
        .await
        .expect("expired-token request");
    assert_eq!(expired_resp.status(), 401);

    // 6. An oversized request body from an authenticated caller -> 413,
    // over the real wire — the real regression test for the body-size-cap
    // fix (max_request_body_bytes = 200 above; this body is well over it).
    let oversized_body = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "kb/query.capabilities",
        "params": {"kb_id": "x".repeat(1000)}
    });
    let oversized_resp = client
        .post(&base_url)
        .bearer_auth(&valid_token)
        .json(&oversized_body)
        .send()
        .await
        .expect("oversized-body request");
    assert_eq!(
        oversized_resp.status(),
        413,
        "an authenticated caller sending a body over max_request_body_bytes must get a clean \
         413, never be allowed to force unbounded server-side buffering"
    );
}

/// Adversarial test (found via an independent security review of this
/// branch): `oauth.rs`'s own unit tests already prove a tampered/forged
/// signature is rejected (`daemon/src/oauth.rs`'s
/// `tampered_signature_is_rejected`), but only in-process -- never carried
/// over a real TLS+HTTP connection the way this file's other adversarial
/// cases (wrong-audience, expired) already are. Closes that gap: signs a
/// token with a SECOND, unrelated keypair (never published to the mock JWKS
/// server, so it's cryptographically indistinguishable from a legitimate
/// signature except for the fact that it doesn't verify against `alice`'s
/// registered key) and asserts the real wire response is a clean 401, not a
/// crash, a hang, or -- worse -- a response that reached dispatch.
#[tokio::test]
async fn forged_signature_token_is_rejected_over_the_real_wire() {
    let (_private_key_pem, jwks) = generate_key_material();
    let jwks_addr = spawn_mock_jwks_server(&jwks).await;
    let daemon = spawn_daemon_with_oauth(jwks_addr).await;
    let base_url = format!("https://{}", daemon.oauth_addr);
    let client = insecure_https_client();

    // A completely different keypair, never registered in the JWKS this
    // daemon trusts -- the token is well-formed and its claims are
    // otherwise valid, but the signature does not match any key the
    // server will accept.
    let (forger_private_key_pem, _unused_jwks) = generate_key_material();
    let forged_token = sign_token(&forger_private_key_pem, &valid_claims());

    let forged_resp = client
        .get(&base_url)
        .bearer_auth(&forged_token)
        .send()
        .await
        .expect("forged-signature request");
    assert_eq!(
        forged_resp.status(),
        401,
        "a token signed by a key not present in the trusted JWKS must be rejected over the \
         real wire, exactly like the in-process unit test already proves for the local case"
    );
    assert!(
        forged_resp
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .is_some(),
        "a rejected forged token must still get a spec-compliant WWW-Authenticate challenge"
    );
}

/// Adversarial regression test (found via an independent security review of
/// this branch): the OAuth HTTPS listener was the one new network-facing
/// surface in this daemon that never got a `ConnLimiter` cap, unlike collab
/// TCP / KB Unix socket / P2P mesh which all already had one (ADR-054's
/// `#342` failure class). Proves the fix over the real wire: opens
/// `max_connections` real TCP connections and keeps them alive, then proves
/// the next one is rejected -- the server closes it immediately, before any
/// TLS handshake, rather than accepting an unbounded number of parked
/// connections.
#[tokio::test]
async fn oauth_listener_connection_cap_rejects_the_nplus1th_client() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (_private_key_pem, jwks) = generate_key_material();
    let jwks_addr = spawn_mock_jwks_server(&jwks).await;
    let daemon = spawn_daemon_with_oauth_capped(jwks_addr, 2).await;

    // Open exactly `max_connections` (2) real TCP connections, confirming
    // EACH ONE was genuinely accepted (its guard acquired) before opening
    // the next. A raw `connect()` succeeding only proves the OS completed
    // the TCP handshake and queued the connection in the kernel backlog --
    // NOT that the server's accept loop has processed it yet, and NOT that
    // processing order matches client-side connect() call order (a real
    // flake found on CI: the "3rd"/over-cap connection was sometimes the
    // one the server's accept loop actually serviced first, while one of
    // the ostensibly in-cap "kept" connections got silently rejected
    // instead -- the original version of this test never checked that).
    // Confirmation without disturbing the held guard: an accepted
    // connection stays open with no data at all (the server is waiting on
    // a ClientHello that never arrives, holding the guard for the whole
    // `HANDSHAKE_TIMEOUT_SECS` window) -- a short read-with-timeout that
    // itself times out (never EOFs) is proof of acceptance.
    let mut kept = Vec::new();
    for i in 0..2 {
        let mut conn = tokio::net::TcpStream::connect(daemon.oauth_addr)
            .await
            .expect("connection within the cap must be accepted");
        let mut probe = [0u8; 1];
        let probe_result =
            tokio::time::timeout(Duration::from_millis(750), conn.read(&mut probe)).await;
        assert!(
            probe_result.is_err(),
            "kept connection {i} (within the configured cap of 2) must stay open with no data \
             -- got {probe_result:?} instead, meaning it was unexpectedly closed/rejected \
             (an accept-order race, not a real cap-enforcement failure)"
        );
        kept.push(conn);
    }

    // The 3rd connection exceeds the cap -- the server drops it (via the
    // guard never being acquired, so the accepted `TcpStream` is dropped at
    // the end of that loop iteration with no task ever spawned for it)
    // before any TLS handshake is attempted. From the client's side: the
    // raw TCP connect can succeed (accept() already happened at the OS
    // level), but a subsequent read must hit EOF almost immediately, never
    // a real TLS ServerHello.
    let mut over_cap = tokio::net::TcpStream::connect(daemon.oauth_addr)
        .await
        .expect("raw TCP connect can still succeed even when the daemon-level cap is full");
    let _ = over_cap.write_all(b"\x16\x03\x01\x00\x00").await; // harmless TLS-shaped bytes
    let mut buf = [0u8; 16];
    let read_result = tokio::time::timeout(Duration::from_secs(5), over_cap.read(&mut buf)).await;
    match read_result {
        Ok(Ok(0)) => {} // EOF -- server closed it, exactly as expected
        Ok(Ok(n)) => panic!(
            "expected the over-cap connection to be closed with no data, got {n} bytes \
             (a real TLS response would mean the cap was NOT enforced)"
        ),
        Ok(Err(e)) => {
            // A reset (ECONNRESET) is also an acceptable "closed" signal
            // depending on OS/timing.
            assert!(
                matches!(e.kind(), std::io::ErrorKind::ConnectionReset),
                "expected a clean EOF or connection reset for the over-cap connection, got: {e}"
            );
        }
        Err(_) => panic!(
            "the over-cap connection was neither closed nor served within 5s -- the cap \
             appears to not be enforced at all (a stuck/parked connection is exactly the \
             #342 failure class this test exists to catch)"
        ),
    }

    drop(kept);
}
