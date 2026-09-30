//! The OAuth HTTPS listener presents a renewed certificate without a restart
//! (ADR-111 P1).
//!
//! Certificates on the platform this listener sits behind live about a week,
//! and it used to read its PEM pair exactly once at startup -- so every renewal
//! meant a restart, and a missed restart meant an expired certificate. These
//! tests drive the real `mae-daemon` binary over real TLS and assert on the
//! certificate a CLIENT actually receives in the handshake (reqwest's
//! `TlsInfo`), never on server-internal state: the property that matters is
//! what a connecting client sees.
//!
//! The attacker's case -- here a clumsy renewal job rather than an adversary --
//! is a pair that is invalid or half-written at the moment a client connects:
//! garbage, an empty file, or a new certificate beside the OLD key. The
//! listener must keep serving the previous certificate, never fail handshakes.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn free_tcp_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// A fresh self-signed pair (distinct key per call -- principle #14).
struct Pair {
    cert_pem: String,
    key_pem: String,
    /// Short SHA-256 fingerprint of the DER certificate -- compared instead of
    /// the raw bytes so a failure prints two readable fingerprints.
    der: String,
}

fn fingerprint(der: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(der))[..16].to_string()
}

fn new_pair() -> Pair {
    let ck = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()]).unwrap();
    Pair {
        cert_pem: ck.cert.pem(),
        key_pem: ck.signing_key.serialize_pem(),
        der: fingerprint(ck.cert.der()),
    }
}

/// Write `contents` to `path`, then stamp it `offset_secs` into the future.
///
/// A real renewal lands hours or days after the previous one; the explicit
/// stamp reproduces that on filesystems whose mtime granularity is a whole
/// second or coarser, where two writes inside one test could otherwise share a
/// timestamp. Distinct offsets per rotation keep every rotation observable.
fn write_stamped(path: &Path, contents: &str, offset_secs: u64) {
    std::fs::write(path, contents).unwrap();
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(SystemTime::now() + Duration::from_secs(offset_secs))
        .unwrap();
}

struct DaemonGuard {
    _child: tokio::process::Child,
    _tmp: tempfile::TempDir,
    addr: SocketAddr,
    cert_path: PathBuf,
    key_path: PathBuf,
}

async fn spawn_daemon_with(initial: &Pair) -> DaemonGuard {
    let tmp = tempfile::tempdir().unwrap();
    let cert_path = tmp.path().join("oauth.crt");
    let key_path = tmp.path().join("oauth.key");
    std::fs::write(&cert_path, &initial.cert_pem).unwrap();
    std::fs::write(&key_path, &initial.key_pem).unwrap();

    let port = free_tcp_port();
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let config = format!(
        r#"
[collab]
enabled = false

[oauth]
enabled = true
bind = "127.0.0.1:{port}"
canonical_resource_uri = "https://127.0.0.1/mcp"
jwks_url = "http://127.0.0.1:1/unused-no-token-is-presented-here"
cert_path = "{cert}"
key_path = "{key}"
max_connections = 0
"#,
        cert = cert_path.display().to_string().replace('\\', "\\\\"),
        key = key_path.display().to_string().replace('\\', "\\\\"),
    );
    let config_path = tmp.path().join("daemon.toml");
    std::fs::write(&config_path, config).unwrap();

    let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "--data-dir",
            tmp.path().to_str().unwrap(),
            "--socket",
            tmp.path().join("kb.sock").to_str().unwrap(),
        ])
        .env("XDG_RUNTIME_DIR", tmp.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn mae-daemon");

    let up =
        mae_mcp::ready::wait_until(|| async { tokio::net::TcpStream::connect(addr).await.is_ok() })
            .await;
    assert!(
        up,
        "{}",
        mae_mcp::ready::timeout_message(&format!("OAuth listener on {addr}"))
    );
    DaemonGuard {
        _child: child,
        _tmp: tmp,
        addr,
        cert_path,
        key_path,
    }
}

/// Complete ONE fresh TLS handshake (a new client, so nothing is pooled or
/// resumed) and return the leaf certificate the server presented. Panics if the
/// handshake fails -- a failed handshake is itself a test failure here.
async fn presented_cert(addr: SocketAddr) -> String {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .tls_info(true)
        .build()
        .unwrap();
    let resp = client
        .get(format!(
            "https://{addr}/.well-known/oauth-protected-resource"
        ))
        .send()
        .await
        .expect("the TLS handshake must succeed");
    resp.extensions()
        .get::<reqwest::tls::TlsInfo>()
        .and_then(|i| i.peer_certificate())
        .map(fingerprint)
        .expect("peer certificate")
}

/// Several successive renewals, each observed by the very next handshake -- no
/// restart, no wait.
#[tokio::test]
async fn a_renewed_certificate_is_presented_without_a_restart() {
    let first = new_pair();
    let daemon = spawn_daemon_with(&first).await;
    assert_eq!(presented_cert(daemon.addr).await, first.der);

    for (round, offset) in [(1u64, 60u64), (2, 7_200), (3, 86_400)] {
        let next = new_pair();
        // Key first, then certificate: the order a renewal tool commonly uses.
        write_stamped(&daemon.key_path, &next.key_pem, offset);
        write_stamped(&daemon.cert_path, &next.cert_pem, offset);
        assert_eq!(
            presented_cert(daemon.addr).await,
            next.der,
            "renewal {round}: the next handshake must present the new certificate"
        );
    }
}

/// An invalid or half-written pair never breaks a handshake: the previous
/// certificate keeps being served, and a later valid pair is still picked up.
#[tokio::test]
async fn an_invalid_or_half_written_pair_keeps_the_previous_certificate() {
    let first = new_pair();
    let daemon = spawn_daemon_with(&first).await;
    assert_eq!(presented_cert(daemon.addr).await, first.der);

    // Garbage where the certificate should be.
    write_stamped(&daemon.cert_path, "not a certificate\n", 60);
    assert_eq!(presented_cert(daemon.addr).await, first.der, "garbage cert");

    // Truncated to nothing, as mid-write.
    write_stamped(&daemon.cert_path, "", 120);
    assert_eq!(presented_cert(daemon.addr).await, first.der, "empty cert");

    // Half-written renewal: the NEW certificate beside the OLD key. Serving it
    // would sign the handshake with a key that does not match the certificate,
    // so every client would fail -- it must be refused and the old pair kept.
    let second = new_pair();
    write_stamped(&daemon.cert_path, &second.cert_pem, 180);
    write_stamped(&daemon.key_path, &first.key_pem, 180);
    assert_eq!(
        presented_cert(daemon.addr).await,
        first.der,
        "new cert + old key"
    );

    // The renewal completes: the matching key lands, and the new pair is used.
    write_stamped(&daemon.key_path, &second.key_pem, 240);
    assert_eq!(
        presented_cert(daemon.addr).await,
        second.der,
        "a valid pair after a bad one must still be picked up"
    );

    // And a later bad write keeps the SECOND certificate, not the first: the
    // key file cut off halfway, as by an interrupted copy.
    let truncated = &second.key_pem[..second.key_pem.len() / 2];
    write_stamped(&daemon.key_path, truncated, 300);
    assert_eq!(
        presented_cert(daemon.addr).await,
        second.der,
        "truncated key"
    );
}
