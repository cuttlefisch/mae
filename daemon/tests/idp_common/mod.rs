//! A mock identity provider for the daemon e2e suites (`identity_binding_e2e`,
//! `ws_bound_idp_e2e`): a real RSA key, a JWKS served over plain HTTP, real
//! RS256 tokens, and the real `mae-daemon binding` CLI against a serving daemon.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::Path;

use serde_json::{json, Value};

pub const KID: &str = "idp-key";
pub const AUD: &str = "https://127.0.0.1/mcp";
pub const ISS: &str = "https://idp.example.com";

fn b64url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A fresh RSA key: its PEM, and the JWKS publishing it under [`KID`].
pub fn idp_keys() -> (String, Value) {
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use rsa::traits::PublicKeyParts;
    let key = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap();
    let pem = key
        .to_pkcs1_pem(rsa::pkcs8::LineEnding::LF)
        .unwrap()
        .to_string();
    let p = key.to_public_key();
    let jwks = json!({"keys": [{"kid": KID, "kty": "RSA", "alg": "RS256", "use": "sig",
        "n": b64url(&p.n().to_bytes_be()), "e": b64url(&p.e().to_bytes_be())}]});
    (pem, jwks)
}

/// An RS256 token from [`ISS`] for [`AUD`], valid an hour, carrying `groups`.
pub fn idp_token(pem: &str, sub: &str, groups: &[&str]) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = json!({"sub": sub, "aud": AUD, "iss": ISS, "iat": now, "exp": now + 3600,
        "groups": groups});
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some(KID.into());
    let key = jsonwebtoken::EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap();
    jsonwebtoken::encode(&header, &claims, &key).unwrap()
}

/// Serve `jwks` on a fresh local port, to every request, until the test ends.
pub async fn spawn_jwks(jwks: &Value) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let body = jwks.to_string();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let body = body.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes()).await;
                let _ = s.shutdown().await;
            });
        }
    });
    addr
}

/// The administrator's `mae-daemon binding` CLI against the serving daemon
/// whose config is `config` and data dir `root`: `(success, stdout+stderr)`.
pub fn binding_cli(config: &Path, root: &Path, args: &[&str]) -> (bool, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .arg("--config")
        .arg(config)
        .arg("--data-dir")
        .arg(root)
        .arg("binding")
        .args(args)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_RUNTIME_DIR", root)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned()
        + String::from_utf8_lossy(&out.stderr).as_ref();
    (out.status.success(), text)
}
