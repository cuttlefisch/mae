//! ADR-098 Amendment 1, Phase C's read path, over the real wire: a real
//! `mae-daemon` with a real TLS listener, a mock JWKS issuing real RS256
//! tokens, the real `mae-daemon binding` CLI run against the serving daemon,
//! and real Ed25519 member keys.
//!
//! What the in-process tests (`src/oauth_binding_tests.rs`,
//! `src/tests/identity_binding_listener_tests.rs`, `src/identity_binding_tests.rs`)
//! cannot show and this does: the routes exist on the listener and refuse a
//! self-issued token; a binding made by proof is what the next request
//! resolves to; the CLI's `set-kind` and `unbind` reach a RUNNING daemon on its
//! next request; the group gate refuses before anything is written; and no
//! network method changes a binding or its kind, nor names another principal's
//! binding in its answer.

mod daemon_harness;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use mae_daemon::oauth_self_issue::mint_self_token;
use mae_mcp::identity::Identity;
use serde_json::{json, Value};

const KID: &str = "idp-key";
const AUD: &str = "https://127.0.0.1/mcp";
const ISS: &str = "https://idp.example.com";
const GROUP: &str = "mae-readers";
/// The second allowed group: Bob carries only this one, Alice only `GROUP`,
/// so each is admitted by ONE of the two (the gate is any-of).
const OTHER_GROUP: &str = "staff";

fn b64url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn idp_keys() -> (String, Value) {
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

fn idp_token(pem: &str, sub: &str, groups: &[&str]) -> String {
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

async fn spawn_jwks(jwks: &Value) -> SocketAddr {
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

struct Daemon {
    _proc: daemon_harness::Process,
    tmp: tempfile::TempDir,
    base: String,
    identity: Identity,
    pem: String,
    http: reqwest::Client,
}

async fn spawn() -> Daemon {
    let (pem, jwks) = idp_keys();
    let jwks_addr = spawn_jwks(&jwks).await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let (cert, key) = (root.join("tls.crt"), root.join("tls.key"));
    let cert_der = daemon_harness::write_self_signed_cert(&cert, &key);
    let id_dir = root.join("identity");
    let identity = Identity::generate("daemon-under-test");
    identity.save(&id_dir).unwrap();
    let ak = id_dir.join("authorized_keys");
    std::fs::write(
        &ak,
        format!("{}\n", Identity::generate("c").public().to_line()),
    )
    .unwrap();
    let proc = daemon_harness::spawn_ready(|ports| {
        let config = format!(
            r#"
[collab]
enabled = true
bind = "127.0.0.1:{collab}"

[collab.auth]
mode = "key"
identity_dir = "{id_dir}"
authorized_keys = "{ak}"

[oauth]
enabled = true
bind = "127.0.0.1:{oauth}"
canonical_resource_uri = "{AUD}"
jwks_url = "http://127.0.0.1:{jwks}/jwks"
issuer = "{ISS}"
cert_path = "{cert}"
key_path = "{key}"
kb_query_enabled = true
self_issued_tokens_enabled = true
identity_binding_enabled = true
allowed_groups = ["{GROUP}", "{OTHER_GROUP}"]
"#,
            collab = ports.collab,
            oauth = ports.oauth,
            jwks = jwks_addr.port(),
            id_dir = id_dir.display(),
            ak = ak.display(),
            cert = cert.display(),
            key = key.display(),
        );
        std::fs::write(root.join("daemon.toml"), config).unwrap();
        let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_mae-daemon"));
        cmd.arg("--config")
            .arg(root.join("daemon.toml"))
            .arg("--data-dir")
            .arg(&root)
            .env("XDG_RUNTIME_DIR", &root);
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
    Daemon {
        base: format!("https://{}", proc.ports.oauth_addr()),
        _proc: proc,
        tmp,
        identity,
        pem,
        http: reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap(),
    }
}

impl Daemon {
    fn root(&self) -> &Path {
        self.tmp.path()
    }

    /// The administrator's CLI, against this serving daemon's own data dir.
    fn cli(&self, args: &[&str]) -> (bool, String) {
        let root = self.root();
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
            .arg("--config")
            .arg(root.join("daemon.toml"))
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

    /// Status and raw body of a POST to `path`.
    async fn post(&self, path: &str, token: &str, body: Value) -> (u16, String) {
        let r = self
            .http
            .post(format!("{}{path}", self.base))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .unwrap();
        (r.status().as_u16(), r.text().await.unwrap())
    }

    /// The bare probe: who the listener resolved the token to.
    async fn whoami(&self, token: &str) -> (u16, Value) {
        let r = self
            .http
            .get(&self.base)
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        (
            status,
            serde_json::from_str(&r.text().await.unwrap()).unwrap_or(Value::Null),
        )
    }

    fn self_token(&self, fp: &str) -> String {
        mint_self_token(&self.identity, fp, AUD, 3600).unwrap()
    }

    /// Rows the collab store holds: a CRDT write would change one of these.
    fn store_rows(&self) -> (i64, i64) {
        let path: PathBuf = self.root().join("collab").join("state.db");
        let conn =
            sqlite::Connection::open_with_flags(&path, sqlite::OpenFlags::new().with_read_only())
                .unwrap();
        let count = |sql: &str| {
            let mut st = conn.prepare(sql).unwrap();
            st.next().unwrap();
            st.read::<i64, _>(0).unwrap()
        };
        (
            count("SELECT COUNT(*) FROM wal"),
            count("SELECT COUNT(*) FROM snapshots"),
        )
    }
}

fn json_of(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or(Value::Null)
}

#[tokio::test]
async fn binding_by_proof_kind_group_gate_and_unbind_over_the_real_wire() {
    let d = spawn().await;
    let member = Identity::generate("alice-member-key");
    let fp = member.fingerprint();
    let alice = idp_token(&d.pem, "alice@idp", &[GROUP]);

    let (status, who) = d.whoami(&alice).await;
    assert_eq!(status, 200, "{who}");
    assert!(
        who["principal"]
            .as_str()
            .unwrap()
            .starts_with("idp-unbound:"),
        "{who}"
    );
    assert_eq!(who["kind"], "agent");

    // A self-issued token's principal already is a fingerprint.
    let (status, raw) = d
        .post(
            "/identity/challenge",
            &d.self_token(&fp),
            json!({"fingerprint": fp}),
        )
        .await;
    assert_eq!(
        (status, json_of(&raw)["kind"].clone()),
        (403, json!("issuer_not_supported")),
        "{raw}"
    );
    let get = d
        .http
        .get(format!("{}/identity/challenge", d.base))
        .bearer_auth(&alice)
        .send()
        .await
        .unwrap();
    assert_eq!(get.status().as_u16(), 405);

    let (status, raw) = d
        .post("/identity/challenge", &alice, json!({"fingerprint": fp}))
        .await;
    assert_eq!(status, 200, "{raw}");
    // Signed by the editor's own signer (`mae --sign-identity-challenge`),
    // given the daemon's answer exactly as received.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let me = mae_mcp::identity_challenge::Claimant {
        issuer: ISS,
        subject: "alice@idp",
    };
    let (_, bind) = mae_mcp::identity_challenge::sign_challenge(&member, me, &raw, now).unwrap();
    let (status, raw) = d.post("/identity/bind", &alice, bind.clone()).await;
    assert_eq!(status, 200, "{raw}");

    let (_, who) = d.whoami(&alice).await;
    assert_eq!(
        (who["principal"].as_str(), who["kind"].as_str()),
        (Some(fp.as_str()), Some("agent"))
    );
    let (ok, list) = d.cli(&["list"]);
    assert!(ok, "{list}");
    let row = list.lines().find(|l| l.contains("alice@idp")).expect(&list);
    assert!(
        row.contains(&fp) && row.contains("\tagent\tproof\t"),
        "{row}"
    );

    // C3: person only with an IdP session on a flagged binding.
    assert!(
        d.cli(&["set-kind", "--fingerprint", &fp, "--kind", "person"])
            .0
    );
    assert_eq!(d.whoami(&alice).await.1["kind"], "person");
    assert_eq!(d.whoami(&d.self_token(&fp)).await.1["kind"], "agent");

    // D4: the group removed at the IdP; refused before anything is written.
    let before = d.store_rows();
    let groupless = idp_token(&d.pem, "alice@idp", &["other"]);
    let (status, raw) = d
        .post(
            "/",
            &groupless,
            json!({"jsonrpc": "2.0", "id": 1,
        "method": "kb/query.my_kbs", "params": {}}),
        )
        .await;
    assert_eq!(
        (status, json_of(&raw)["kind"].clone()),
        (403, json!("group_required")),
        "{raw}"
    );
    let (status, _) = d
        .post(
            "/identity/challenge",
            &groupless,
            json!({"fingerprint": fp}),
        )
        .await;
    assert_eq!(status, 403);
    assert_eq!(
        d.store_rows(),
        before,
        "a refused session must write nothing"
    );
    assert!(d.cli(&["list"]).1.contains(&fp), "nor touch the binding");
    // Any ONE allowed group admits: the other of the two, alone.
    let other_group_only = idp_token(&d.pem, "alice@idp", &[OTHER_GROUP]);
    let (status, who) = d.whoami(&other_group_only).await;
    assert_eq!(
        (status, who["principal"].as_str()),
        (200, Some(fp.as_str())),
        "{who}"
    );

    // Replayed bind: the challenge is spent.
    let (status, raw) = d.post("/identity/bind", &alice, bind).await;
    assert_eq!(
        (status, json_of(&raw)["kind"].clone()),
        (403, json!("challenge_invalid")),
        "{raw}"
    );

    // Unbind on the running daemon holds on the next request.
    assert!(d.cli(&["unbind", "--fingerprint", &fp]).0);
    let (_, who) = d.whoami(&alice).await;
    assert!(
        who["principal"]
            .as_str()
            .unwrap()
            .starts_with("idp-unbound:"),
        "{who}"
    );
    assert_eq!(who["kind"], "agent");
}

#[tokio::test]
async fn no_network_method_changes_a_binding_or_names_another_principals() {
    let d = spawn().await;
    let (alice_fp, bob_fp) = (
        Identity::generate("a").fingerprint(),
        Identity::generate("b").fingerprint(),
    );
    assert!(
        d.cli(&[
            "bind",
            "--issuer",
            ISS,
            "--subject",
            "alice@idp",
            "--fingerprint",
            &alice_fp
        ])
        .0
    );
    assert!(
        d.cli(&[
            "set-kind",
            "--issuer",
            ISS,
            "--subject",
            "alice@idp",
            "--kind",
            "person"
        ])
        .0
    );
    assert!(
        d.cli(&[
            "bind",
            "--issuer",
            ISS,
            "--subject",
            "bob@idp",
            "--fingerprint",
            &bob_fp
        ])
        .0
    );
    let (_, before) = d.cli(&["list"]);
    assert!(
        before.contains("\tadmin\t"),
        "admin bindings are shown as such: {before}"
    );

    let bob = idp_token(&d.pem, "bob@idp", &[OTHER_GROUP]);
    let mut answers = Vec::new();
    for method in [
        "kb/query.my_kbs",
        "kb/query.capabilities",
        "kb/query.self_token",
        "binding/set_kind",
        "binding/unbind",
        "binding/bind",
        "identity/set_kind",
        "kb/query.set_kind",
    ] {
        let params = json!({"kb_id": "k", "fingerprint": bob_fp, "kind": "person",
            "issuer": ISS, "subject": "bob@idp"});
        answers.push(
            d.post(
                "/",
                &bob,
                json!({"jsonrpc": "2.0", "id": 1, "method": method,
            "params": params}),
            )
            .await,
        );
    }
    for (path, body) in [
        (
            "/identity/bind",
            json!({"fingerprint": bob_fp, "kind": "person"}),
        ),
        (
            "/identity/bind",
            json!({"challenge_id": "x", "public_key": "x", "signature": "x"}),
        ),
        ("/identity/unbind", json!({"fingerprint": bob_fp})),
        (
            "/identity/set_kind",
            json!({"fingerprint": bob_fp, "kind": "person"}),
        ),
        (
            "/identity/challenge",
            json!({"fingerprint": bob_fp, "kind": "person"}),
        ),
    ] {
        answers.push(d.post(path, &bob, body).await);
    }
    for (status, raw) in &answers {
        for other in [alice_fp.as_str(), "alice@idp"] {
            assert!(
                !raw.contains(other),
                "status {status} names another principal's binding: {raw}"
            );
        }
    }
    let (_, after) = d.cli(&["list"]);
    assert_eq!(after, before, "a network request changed the binding table");
    assert_eq!(d.whoami(&bob).await.1["kind"], "agent");
}
