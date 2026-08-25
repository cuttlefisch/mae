//! ADR-066 Phase E: verifies a Windows-BUILT client (this binary, compiled
//! natively on `windows-latest`) reaches a genuinely Linux-built
//! `mae-daemon` correctly over both of ADR-066's named remote-connection
//! paths -- the collab sync transport (TCP) and the OAuth/HTTPS `kb/query.*`
//! surface (ADR-053) -- proving already-built protocols work on a Windows
//! client, not new engineering (Phase E's own framing: neither path depends
//! on Phase A's local-IPC fix at all).
//!
//! **Deliberate first-iteration scoping** (documented, not silent): the
//! collab check below uses `auth.mode = "none"` (plain TCP, no mTLS) rather
//! than a full mTLS handshake. Coordinating a shared CA/cert between a WSL2
//! Linux process and a native Windows process in one CI job is real added
//! complexity on top of an already-novel cross-OS CI setup with zero local
//! pre-verification possible; getting bare TCP connectivity proven first,
//! then layering mTLS in as a follow-up once the WSL2 networking itself is
//! confirmed to actually work as assumed, is the same incremental-proof
//! discipline this project's other from-scratch Windows CI work (ADR-066
//! Phase A/C) already used.
//!
//! This is a standalone example (not the full `mae`/`mae-mcp-shim` binary)
//! deliberately: it exercises the SAME underlying crates (`mae_mcp`,
//! `reqwest`) the real client code is built on, which is what Phase E's own
//! text actually cares about (protocol correctness on Windows), without
//! pulling in the full editor binary's GUI/LSP/DAP/AI dependency surface
//! for a check that only needs the network layer.
//!
//! Usage:
//!   `windows_remote_verify --collab-addr <ip:port> --oauth-url
//!   <https://host:port> --jwt <token>` -- run the verification checks.
//!
//!   `windows_remote_verify --gen-material <dir> --resource <uri>` -- write
//!   a self-signed TLS cert+key, a JWKS document, and a validly-signed RS256
//!   JWT (all real, freshly generated per invocation -- CLAUDE.md principle
//!   #14, never a shared/hardcoded test key) to `<dir>`, so the CI workflow
//!   can generate this material once on the Windows side (which already has
//!   the needed crates compiled) and hand the cert/key/JWKS files to the
//!   WSL2-hosted daemon via the filesystem bridge, while the signed JWT
//!   stays on the Windows side for this same binary's own `--jwt` use above.
//!   Prints the JWT to stdout (nothing else) so the calling shell can
//!   capture it directly.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use rsa::pkcs1::EncodeRsaPrivateKey;
use rsa::traits::PublicKeyParts;
use rsa::RsaPrivateKey;
use tokio::net::TcpStream;

const GEN_KID: &str = "windows-remote-verify-key";
const GEN_ISSUER: &str = "https://idp.example.com";

fn base64_url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Same shape as `daemon/tests/oauth_e2e.rs`'s own `generate_key_material`/
/// `sign_token`/`generate_self_signed_cert` -- this binary can't import
/// that test-only module directly (different crate-target boundary), so the
/// logic is duplicated rather than reached across; kept in sync by both
/// being small and stable.
fn gen_material(dir: &Path, resource: &str) {
    std::fs::create_dir_all(dir).expect("create material dir");

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
        "keys": [{"kid": GEN_KID, "n": n, "e": e, "kty": "RSA", "alg": "RS256", "use": "sig"}]
    });
    std::fs::write(dir.join("jwks.json"), jwks.to_string()).expect("write jwks.json");

    let cert_key = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()])
        .expect("rcgen self-signed cert");
    std::fs::write(dir.join("cert.pem"), cert_key.cert.pem()).expect("write cert.pem");
    std::fs::write(dir.join("key.pem"), cert_key.signing_key.serialize_pem())
        .expect("write key.pem");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "sub": "windows-remote-verify@example.com",
        "aud": resource,
        "iss": GEN_ISSUER,
        "iat": now,
        "exp": now + 3600,
    });
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(GEN_KID.to_string());
    let encoding_key = EncodingKey::from_rsa_pem(pem.as_bytes()).expect("valid PEM");
    let jwt = encode(&header, &claims, &encoding_key).expect("sign JWT");
    println!("{jwt}");
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

async fn check_collab_tcp(addr: SocketAddr) -> Result<(), String> {
    let mut stream = TcpStream::connect(addr)
        .await
        .map_err(|e| format!("collab TCP connect to {addr} failed: {e}"))?;

    let req = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"clientInfo": {"name": "windows-remote-verify"}}
    });
    let body = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
    mae_mcp::write_framed(&mut stream, &body, Duration::from_secs(15))
        .await
        .map_err(|e| format!("write initialize: {e}"))?;
    let (r, _w) = stream.split();
    let mut reader = tokio::io::BufReader::new(r);
    let msg = tokio::time::timeout(Duration::from_secs(15), mae_mcp::read_message(&mut reader))
        .await
        .map_err(|_| "timed out reading initialize response".to_string())?
        .map_err(|e| format!("read initialize response: {e}"))?
        .ok_or("connection closed before initialize response")?;
    let resp: serde_json::Value = serde_json::from_str(&msg).map_err(|e| e.to_string())?;
    if resp.get("error").is_some() {
        return Err(format!("initialize returned an error: {resp}"));
    }
    println!("[collab] initialize OK: {resp}");

    // A second real round trip on the same connection, matching
    // network_e2e.rs's own tcp_initialize_and_ping shape.
    let ping = serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "$/ping"});
    let body = serde_json::to_vec(&ping).map_err(|e| e.to_string())?;
    mae_mcp::write_framed(&mut stream, &body, Duration::from_secs(15))
        .await
        .map_err(|e| format!("write ping: {e}"))?;
    let (r, _w) = stream.split();
    let mut reader = tokio::io::BufReader::new(r);
    let msg = tokio::time::timeout(Duration::from_secs(15), mae_mcp::read_message(&mut reader))
        .await
        .map_err(|_| "timed out reading ping response".to_string())?
        .map_err(|e| format!("read ping response: {e}"))?
        .ok_or("connection closed before ping response")?;
    let resp: serde_json::Value = serde_json::from_str(&msg).map_err(|e| e.to_string())?;
    if resp.get("result").and_then(|v| v.as_str()) != Some("pong") {
        return Err(format!("expected \"pong\", got: {resp}"));
    }
    println!("[collab] $/ping OK: pong");
    Ok(())
}

async fn check_oauth_https(oauth_url: &str, jwt: &str) -> Result<(), String> {
    // danger_accept_invalid_certs: this test's server presents a real but
    // self-signed cert (generated fresh per CI run, see the workflow) --
    // there is no CA to validate it against, by design, matching
    // oauth_e2e.rs's own in-process equivalent. Never appropriate outside a
    // test/verification harness talking to a deliberately self-signed test
    // endpoint.
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| format!("build reqwest client: {e}"))?;

    let resp = client
        .post(oauth_url)
        .bearer_auth(jwt)
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "kb/query.capabilities",
            "params": {"kb_id": "windows-remote-verify-probe"}
        }))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("HTTPS request to {oauth_url} failed: {e:?}"))?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    // A 401 means the bearer token / TLS handshake itself was rejected --
    // the actual thing this check exists to prove works. Any OTHER status
    // (200, 404, whatever kb/query.capabilities' real handler returns for
    // an unconfigured/absent KB) proves the token was accepted and the
    // request reached the real handler, which is what "the remote
    // connection works" means here -- matching oauth_e2e.rs's own
    // "a non-401 response reaching kb_query::dispatch" oracle.
    if status.as_u16() == 401 {
        return Err(format!("OAuth/HTTPS request was rejected (401): {body}"));
    }
    println!("[oauth] HTTPS request reached the daemon, status={status}: {body}");
    Ok(())
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    if let Some(dir) = arg_value(&args, "--gen-material") {
        let resource =
            arg_value(&args, "--resource").expect("--gen-material requires --resource <uri>");
        gen_material(Path::new(&dir), &resource);
        return;
    }

    let collab_addr = arg_value(&args, "--collab-addr");
    let oauth_url = arg_value(&args, "--oauth-url");
    let jwt = arg_value(&args, "--jwt");

    let mut failed = false;

    if let Some(addr) = collab_addr {
        let addr: SocketAddr = addr.parse().expect("--collab-addr must be ip:port");
        match check_collab_tcp(addr).await {
            Ok(()) => println!("[collab] PASS"),
            Err(e) => {
                eprintln!("[collab] FAIL: {e}");
                failed = true;
            }
        }
    } else {
        println!("[collab] skipped (no --collab-addr given)");
    }

    if let (Some(url), Some(jwt)) = (oauth_url, jwt) {
        match check_oauth_https(&url, &jwt).await {
            Ok(()) => println!("[oauth] PASS"),
            Err(e) => {
                eprintln!("[oauth] FAIL: {e}");
                failed = true;
            }
        }
    } else {
        println!("[oauth] skipped (no --oauth-url/--jwt given)");
    }

    if failed {
        std::process::exit(1);
    }
}
