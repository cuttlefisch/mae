//! Local (Linux-to-Linux) smoke test for the `windows_remote_verify` binary
//! that ADR-066 Phase E's actual CI job runs Windows-native, pointed at a
//! WSL2-hosted daemon. This test cannot verify the cross-OS/WSL2 part (no
//! Windows environment here), but it DOES prove the binary's own protocol
//! logic (collab TCP round trip, OAuth/HTTPS bearer-token round trip)
//! against a real `mae-daemon` before ever trusting it in a much harder to
//! debug cross-OS CI job -- de-risking the actual Phase E CI leg by
//! catching ordinary logic bugs here first, cheaply and locally.
//!
//! Run (the example binary must be built first -- `cargo test` does not
//! build `examples/` targets automatically):
//!   `cargo build -p mae-daemon --example windows_remote_verify && \
//!    cargo test -p mae-daemon --test windows_remote_verify_smoke_test`

mod daemon_harness;

use std::net::SocketAddr;

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use rsa::pkcs1::EncodeRsaPrivateKey;
use rsa::traits::PublicKeyParts;
use rsa::RsaPrivateKey;

const TEST_KID: &str = "smoke-test-key";
const TEST_ISSUER: &str = "https://idp.example.com";

fn base64_url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

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

fn sign_token(private_key_pem: &str, resource: &str) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(TEST_KID.to_string());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "sub": "windows-remote-verify-smoke@example.com",
        "aud": resource,
        "iss": TEST_ISSUER,
        "iat": now,
        "exp": now + 3600,
    });
    let encoding_key = EncodingKey::from_rsa_pem(private_key_pem.as_bytes()).expect("valid PEM");
    encode(&header, &claims, &encoding_key).expect("sign")
}

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

struct DaemonGuard {
    _proc: daemon_harness::Process,
    _tmp: tempfile::TempDir,
    collab_addr: SocketAddr,
    oauth_url: String,
}

fn resource_for(oauth_port: u16) -> String {
    format!("https://127.0.0.1:{oauth_port}/mcp")
}

/// Ready means BOTH listeners proven up -- the binary under test talks to
/// both, and the OAuth one binds after collab. Waiting on collab alone handed
/// the binary a daemon whose OAuth port was not listening (CI: "[collab] PASS",
/// then "[oauth] FAIL ... Connection refused"); see `daemon_harness`.
async fn spawn_daemon_with_collab_and_oauth(jwks_addr: SocketAddr) -> (DaemonGuard, String) {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();

    let cert_path = tmp.path().join("cert.pem");
    let key_path = tmp.path().join("key.pem");
    let cert_der = daemon_harness::write_self_signed_cert(&cert_path, &key_path);

    let root = tmp.path().to_path_buf();
    let proc = daemon_harness::spawn_ready(|ports| {
        let config = format!(
            r#"
[collab]
enabled = true
bind = "127.0.0.1:{collab_port}"

[collab.auth]
mode = "none"

[oauth]
enabled = true
bind = "127.0.0.1:{oauth_port}"
canonical_resource_uri = "{resource}"
jwks_url = "http://{jwks_addr}/jwks.json"
issuer = "{TEST_ISSUER}"
cert_path = "{cert}"
key_path = "{key}"
kb_query_enabled = true
"#,
            collab_port = ports.collab,
            oauth_port = ports.oauth,
            resource = resource_for(ports.oauth),
            cert = cert_path.display(),
            key = key_path.display(),
        );
        let config_path = root.join("daemon.toml");
        std::fs::write(&config_path, config).unwrap();

        let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_mae-daemon"));
        cmd.args([
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--config",
            config_path.to_str().unwrap(),
        ])
        .env("XDG_RUNTIME_DIR", &root)
        .env("XDG_CONFIG_HOME", root.join("config"));
        daemon_harness::Launch {
            cmd,
            status_socket: root.join("mae-daemon.sock"),
            log: root.join("daemon.log"),
            collab: true,
            oauth: Some(daemon_harness::OauthProbe {
                cert_der: cert_der.clone(),
                ws_ticket: false,
            }),
        }
    })
    .await;
    let ports = proc.ports;
    (
        DaemonGuard {
            _proc: proc,
            _tmp: tmp,
            collab_addr: ports.collab_addr(),
            oauth_url: format!("https://127.0.0.1:{}", ports.oauth),
        },
        resource_for(ports.oauth),
    )
}

/// `windows_remote_verify` is an `examples/` target (needs dev-dependencies
/// -- rsa/rcgen/rand/base64 -- for its `--gen-material` mode, which a
/// `[[bin]]` target can't see; a production `mae-daemon` binary has no
/// business linking test-cert-generation crates). Cargo does NOT set a
/// `CARGO_BIN_EXE_*` env var for examples (only real `[[bin]]` targets), so
/// the path is derived at runtime instead: examples land in a `examples/`
/// directory that's a sibling of the CURRENTLY-RUNNING test binary's own
/// `deps/` directory, both under the same `target/<profile>/`.
fn example_binary_path(name: &str) -> std::path::PathBuf {
    let mut path = std::env::current_exe().expect("current_exe");
    path.pop(); // this test binary's own filename
    path.pop(); // deps/
    path.push("examples");
    path.push(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    });
    assert!(
        path.exists(),
        "expected example binary at {path:?} -- run `cargo build --example {name}` first \
         if running this test directly rather than via `cargo test`"
    );
    path
}

#[tokio::test]
async fn windows_remote_verify_binary_succeeds_against_a_real_local_daemon() {
    let (pem, jwks) = generate_key_material();
    let jwks_addr = spawn_mock_jwks_server(&jwks).await;
    let (daemon, resource) = spawn_daemon_with_collab_and_oauth(jwks_addr).await;
    let jwt = sign_token(&pem, &resource);

    // tokio::process::Command, not std::process::Command -- the latter's
    // .output() blocks the current OS thread synchronously, which on
    // #[tokio::test]'s default single-threaded runtime starves the
    // mock JWKS server task above (a tokio::spawn on this SAME runtime),
    // hanging the daemon's own JWT verification indefinitely. Cost a real
    // debugging round to find -- documented here so it isn't reintroduced.
    let output = tokio::process::Command::new(example_binary_path("windows_remote_verify"))
        .args([
            "--collab-addr",
            &daemon.collab_addr.to_string(),
            "--oauth-url",
            &daemon.oauth_url,
            "--jwt",
            &jwt,
        ])
        .output()
        .await
        .expect("failed to run windows_remote_verify");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "windows_remote_verify must exit 0 against a real, correctly-configured daemon\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("[collab] PASS"), "stdout:\n{stdout}");
    assert!(stdout.contains("[oauth] PASS"), "stdout:\n{stdout}");
}
