//! Real TLS + real HTTP e2e for ADR-067 Phase D3's self-issued OAuth tokens
//! (`daemon/src/oauth_self_issue.rs`) — the same real-wire-transport gap
//! `oauth_e2e.rs`'s module doc explains for externally-issued tokens,
//! applied to the NEW `kid: "self"` short-circuit path this phase adds to
//! `oauth::handle_request`.
//!
//! `daemon/src/oauth_self_issue.rs`'s own `#[cfg(test)] mod tests` already
//! prove the crypto in-process (wrong-audience/expired/forged-signature/
//! wrong-issuer rejected, a valid token accepted) — this file's job is
//! narrower and does NOT re-prove that: it proves the header-peek
//! short-circuit in `oauth::handle_request` (an in-process function this
//! crate's own unit tests can't reach, since `daemon/tests/*.rs` only sees
//! `mae_daemon`'s public lib re-exports) actually routes correctly over a
//! real TLS+HTTP connection, and that the `self_issued_tokens_enabled`
//! config gate holds over the real wire too.
//!
//! Deliberately mints tokens by calling `mae_daemon::oauth_self_issue::
//! mint_self_token` directly (the same function `collab_handler`'s
//! `kb/query.self_token` mTLS RPC calls) against a REAL daemon identity
//! this test pre-seeds on disk before spawning the daemon -- not by driving
//! a real mTLS collab connection to request one, which would require
//! building a from-scratch mTLS TCP JSON-RPC client this crate has no
//! existing precedent for (only the editor's `collab_bridge` does). The RPC
//! itself (auth-required, feature-gated, `sub` bound to the connection's own
//! verified principal) is covered at dispatch level by
//! `collab_handler_self_issue_token_tests.rs`, mirroring this session's
//! established convention (ADR-053/Phase G's own OAuth-side tests took the
//! same "real wire for transport, dispatch-level for the RPC itself" split).
//! Also deliberately does NOT re-seed a real KB or re-prove `kb_query`'s own
//! Hard Rule (ADR-062) here -- that's identical code regardless of how the
//! token was validated, already proven in-process (`kb_query_tests.rs`) and
//! specifically over mTLS (`collab_handler_kb_query_mtls_tests.rs`,
//! ADR-067 Phase D2); re-implementing content-seeding-over-the-wire for a
//! third proof of the same property would be disproportionate to the actual
//! new risk surface this phase introduces.

use std::net::SocketAddr;
use std::time::Duration;

use mae_daemon::oauth_self_issue::{mint_self_token, SelfIssueConfig};
use mae_mcp::identity::Identity;

const CANONICAL_RESOURCE: &str = "https://127.0.0.1/mcp";

fn free_tcp_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn generate_self_signed_cert(cert_path: &std::path::Path, key_path: &std::path::Path) {
    let cert_key = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()])
        .expect("rcgen self-signed cert");
    std::fs::write(cert_path, cert_key.cert.pem()).unwrap();
    std::fs::write(key_path, cert_key.signing_key.serialize_pem()).unwrap();
}

fn insecure_https_client() -> reqwest::Client {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap()
}

struct DaemonGuard {
    _child: tokio::process::Child,
    _tmp: tempfile::TempDir,
    oauth_addr: SocketAddr,
    /// The REAL identity this daemon loaded (pre-seeded to disk before
    /// spawn, per this file's module doc) -- the sole valid signer for any
    /// self-issued token this daemon will accept.
    identity: std::sync::Arc<Identity>,
}

/// Spawn a real `mae-daemon` with `collab.auth.mode = "key"` (a pre-seeded
/// identity this test also holds, so it can mint tokens the running daemon
/// will recognize as its own) and the OAuth listener's self-issued-token
/// support enabled. `jwks_url`/`issuer` are still set (the listener won't
/// even start without them, an existing, unrelated config wrinkle -- see
/// `OAuthConfig::self_issued_tokens_enabled`'s own doc comment) but point
/// nowhere real: a `kid: "self"` token never reaches the JWKS fetch at all.
async fn spawn_daemon_with_self_issue(self_issued_tokens_enabled: bool) -> DaemonGuard {
    let tmp = tempfile::tempdir().unwrap();
    let cert_path = tmp.path().join("oauth.crt");
    let key_path = tmp.path().join("oauth.key");
    generate_self_signed_cert(&cert_path, &key_path);

    let identity_dir = tmp.path().join("identity");
    let identity = std::sync::Arc::new(Identity::generate("daemon-under-test"));
    identity.save(&identity_dir).unwrap();
    // `collab.auth.mode = "key"` refuses to start with an empty
    // authorized_keys (a key-mode listener with no authorized client is a
    // real misconfiguration this daemon correctly rejects) -- this test
    // doesn't need real collab TCP connectivity, but its "key"-mode setup
    // (and thus `daemon_identity_for_oauth`, which the OAuth listener's
    // self-issue support depends on) must still succeed as a whole, so seed
    // one throwaway authorized entry. `authorized_keys`'s own default (when
    // unset) resolves against `mae_mcp::identity::default_collab_dir()` --
    // a DIFFERENT path than `identity_dir` -- so it must be set explicitly
    // here to actually point at the file this test writes.
    let authorized_keys_path = identity_dir.join("authorized_keys");
    std::fs::write(
        &authorized_keys_path,
        format!(
            "{}\n",
            Identity::generate("throwaway-authorized-client")
                .public()
                .to_line()
        ),
    )
    .unwrap();

    let collab_port = free_tcp_port();
    let oauth_port = free_tcp_port();
    let oauth_addr: SocketAddr = format!("127.0.0.1:{oauth_port}").parse().unwrap();

    let config_toml = format!(
        r#"
[collab]
enabled = true
bind = "127.0.0.1:{collab_port}"

[collab.auth]
mode = "key"
identity_dir = "{identity_dir}"
authorized_keys = "{authorized_keys_path}"

[oauth]
enabled = true
bind = "127.0.0.1:{oauth_port}"
canonical_resource_uri = "{CANONICAL_RESOURCE}"
jwks_url = "http://127.0.0.1:1/unused-never-fetched-by-self-issued-tokens"
issuer = "https://idp.example.com"
principal_claim = "sub"
cert_path = "{cert_path}"
key_path = "{key_path}"
kb_query_enabled = true
max_request_body_bytes = 1048576
kb_query_max_body_bytes = 65536
kb_query_max_scan_nodes = 500
kb_query_max_search_results = 20
max_connections = 0
self_issued_tokens_enabled = {self_issued_tokens_enabled}
self_issued_token_ttl_secs = 3600
"#,
        collab_port = collab_port,
        oauth_port = oauth_port,
        identity_dir = identity_dir.display(),
        authorized_keys_path = authorized_keys_path.display(),
        cert_path = cert_path.display(),
        key_path = key_path.display(),
        self_issued_tokens_enabled = self_issued_tokens_enabled,
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
        identity,
    }
}

#[tokio::test]
async fn a_self_issued_token_is_accepted_over_the_real_wire_and_maps_the_principal() {
    let daemon = spawn_daemon_with_self_issue(true).await;
    let base_url = format!("https://{}", daemon.oauth_addr);
    let client = insecure_https_client();

    let token = mint_self_token(
        &daemon.identity,
        "SHA256:real-member-fp",
        CANONICAL_RESOURCE,
        3600,
    )
    .expect("mint");

    // The bare bearer-verification probe (no JSON-RPC body) -- mirrors
    // `oauth_e2e.rs`'s own externally-issued-token happy path exactly,
    // proving the `kid: "self"` short-circuit reaches the same principal-
    // mapping result over the real wire.
    let resp = client
        .get(&base_url)
        .bearer_auth(&token)
        .send()
        .await
        .expect("self-issued-token request");
    assert_eq!(
        resp.status(),
        200,
        "a valid self-issued token must be accepted over the real TLS connection"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["principal"], "SHA256:real-member-fp");
    assert_eq!(body["resource"], CANONICAL_RESOURCE);
}

#[tokio::test]
async fn an_expired_self_issued_token_is_rejected_over_the_real_wire() {
    let daemon = spawn_daemon_with_self_issue(true).await;
    let base_url = format!("https://{}", daemon.oauth_addr);
    let client = insecure_https_client();

    // `mint_self_token` only ever mints `exp = now + ttl_secs` (never in the
    // past), and jsonwebtoken's default 60s leeway would swallow a merely
    // few-seconds-past `exp` anyway -- construct the claims directly with a
    // safely-past `exp` (well beyond the leeway window), matching
    // `oauth_self_issue.rs`'s own in-process `expired_token_is_rejected`
    // unit test's approach.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "sub": "SHA256:real-member-fp",
        "aud": CANONICAL_RESOURCE,
        "iss": "self",
        "iat": now.saturating_sub(7200),
        "exp": now.saturating_sub(3600),
    });
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::EdDSA);
    header.kid = Some(mae_daemon::oauth_self_issue::SELF_ISSUED_KID.to_string());
    let der = daemon.identity.pkcs8_der().unwrap();
    let encoding_key = jsonwebtoken::EncodingKey::from_ed_der(&der);
    let token = jsonwebtoken::encode(&header, &claims, &encoding_key).unwrap();

    let resp = client
        .get(&base_url)
        .bearer_auth(&token)
        .send()
        .await
        .expect("expired self-issued-token request");
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn a_wrong_audience_self_issued_token_is_rejected_over_the_real_wire() {
    let daemon = spawn_daemon_with_self_issue(true).await;
    let base_url = format!("https://{}", daemon.oauth_addr);
    let client = insecure_https_client();

    let token = mint_self_token(
        &daemon.identity,
        "SHA256:real-member-fp",
        "https://a-different-daemon.example.com/mcp",
        3600,
    )
    .expect("mint");

    let resp = client
        .get(&base_url)
        .bearer_auth(&token)
        .send()
        .await
        .expect("wrong-audience self-issued-token request");
    assert_eq!(resp.status(), 401);
}

/// Adversarial: a token claiming `kid: "self"` but signed by a completely
/// DIFFERENT identity than this daemon's own -- proves the real wire path
/// verifies against the daemon's actual known pubkey, not merely trusting
/// the `kid` claim's presence. Mirrors `oauth_e2e.rs`'s own
/// `forged_signature_token_is_rejected_over_the_real_wire`.
#[tokio::test]
async fn a_forged_self_issued_token_is_rejected_over_the_real_wire() {
    let daemon = spawn_daemon_with_self_issue(true).await;
    let base_url = format!("https://{}", daemon.oauth_addr);
    let client = insecure_https_client();

    let attacker_identity = Identity::generate("attacker");
    let forged_token = mint_self_token(
        &attacker_identity,
        "SHA256:attacker-claims-to-be-a-real-member",
        CANONICAL_RESOURCE,
        3600,
    )
    .expect("mint (the forging itself succeeds -- it's the daemon's validation that must fail)");

    let resp = client
        .get(&base_url)
        .bearer_auth(&forged_token)
        .send()
        .await
        .expect("forged self-issued-token request");
    assert_eq!(
        resp.status(),
        401,
        "a token signed by ANY key other than this daemon's own real identity must be rejected, \
         even when it claims kid: \"self\" and a plausible-looking sub"
    );
}

/// The config gate (`self_issued_tokens_enabled = false`, the default):
/// a `kid: "self"` token falls through to the ordinary external-JWKS path
/// (pointed at an address nothing is listening on in this harness) and
/// fails there as a normal JWKS-fetch failure -- never silently accepted
/// because the header merely claims `kid: "self"`.
#[tokio::test]
async fn self_issued_tokens_are_inert_when_the_feature_is_disabled() {
    let daemon = spawn_daemon_with_self_issue(false).await;
    let base_url = format!("https://{}", daemon.oauth_addr);
    let client = insecure_https_client();

    let token = mint_self_token(
        &daemon.identity,
        "SHA256:real-member-fp",
        CANONICAL_RESOURCE,
        3600,
    )
    .expect("mint");

    let resp = client
        .get(&base_url)
        .bearer_auth(&token)
        .send()
        .await
        .expect("self-issued-token request against a daemon with the feature disabled");
    assert_ne!(
        resp.status(),
        200,
        "a self-issued token must never be accepted when self_issued_tokens_enabled is false, \
         even though the daemon holds the exact key that signed it"
    );
}

/// A real, direct sanity check that `SelfIssueConfig` (the struct
/// `collab_handler`'s `kb/query.self_token` RPC constructs, threaded from
/// `main.rs`) round-trips through `mint_self_token`/`validate_self_issued_token`
/// -- not exercised over the wire elsewhere in this file, since every other
/// test mints directly rather than via the struct.
#[test]
fn self_issue_config_shape_mints_a_token_validate_self_issued_token_accepts() {
    let identity = std::sync::Arc::new(Identity::generate("daemon-under-test"));
    let config = SelfIssueConfig {
        identity: identity.clone(),
        audience: CANONICAL_RESOURCE.to_string(),
        ttl_secs: 3600,
    };
    let token = mint_self_token(
        &config.identity,
        "SHA256:real-member-fp",
        &config.audience,
        config.ttl_secs,
    )
    .unwrap();
    let pubkey = identity.public().to_bytes();
    let result =
        mae_daemon::oauth_self_issue::validate_self_issued_token(&token, &pubkey, &config.audience);
    assert!(result.is_ok(), "{result:?}");
}
