//! ADR-120 P1a on the HTTPS listener, over real HTTP/1.1 connections to the
//! real service: completeness, fail-closed, health, the session header, the
//! address rule as the listener applies it, coalescing, and that the session
//! id is never a credential. The record's own tests are
//! `mae_daemon::activity::tests`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use mae_daemon::activity::event::{Event, EventKind};
use mae_daemon::activity::session::{self, SessionKey};
use mae_daemon::activity::store::ActivityDb;
use mae_daemon::activity::writer::ActivityRecord;
use mae_daemon::activity::ActivitySettings;
use mae_daemon::blob_store::BlobStore;
use mae_mcp::identity::Identity;
use mae_sync::kb::Role;
use rsa::pkcs1::EncodeRsaPrivateKey;
use rsa::traits::PublicKeyParts;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::blob_route_tests::{test_limits, Reply};
use super::kb_query_tests::{fresh_doc_store, seed_unencrypted_kb};
use crate::oauth::{JwkOwned, JwksCache, Listener, ResourceServerConfig, Route};
use crate::oauth_activity::classify_method;

const AUD: &str = "https://mae.test";
const ISS: &str = "https://idp.test";
const KID: &str = "activity-test";
/// The node's body: never in a refused answer.
const SECRET: &str = "the-body-only-a-reader-may-see";

struct Harness {
    addr: SocketAddr,
    daemon: Arc<Identity>,
    member: Identity,
    rsa_pem: String,
    record: Option<Arc<ActivityRecord>>,
    db: PathBuf,
    _dir: tempfile::TempDir,
}

fn config() -> ResourceServerConfig {
    ResourceServerConfig {
        canonical_resource_uri: AUD.into(),
        principal_claim: "sub".into(),
        groups_claim: "groups".into(),
        allowed_groups: Vec::new(),
        // Never fetched: the cache is preloaded with the test's key.
        jwks_url: "http://127.0.0.1:9/never".into(),
        jwks_ca_file: None,
        issuer: Some(ISS.into()),
        kb_query_enabled: true,
        site_enabled: true,
        site_locale: "en-US".into(),
        site_hidden_namespaces: Vec::new(),
        site_home: Default::default(),
        site_title: Default::default(),
        site_components: Default::default(),
        max_request_body_bytes: 65_536,
        kb_query_max_body_bytes: 65_536,
        kb_query_max_scan_nodes: 500,
        kb_query_max_search_results: 20,
        second_factor: Default::default(),
    }
}

fn rsa_key() -> (String, JwkOwned) {
    use base64::Engine;
    let key = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap();
    let pem = key
        .to_pkcs1_pem(rsa::pkcs8::LineEnding::LF)
        .unwrap()
        .to_string();
    let b64 = |b: Vec<u8>| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
    let jwk = JwkOwned {
        kid: KID.into(),
        n: b64(key.to_public_key().n().to_bytes_be()),
        e: b64(key.to_public_key().e().to_bytes_be()),
    };
    (pem, jwk)
}

fn record_settings(f: impl FnOnce(&mut ActivitySettings)) -> ActivitySettings {
    let mut s = ActivitySettings {
        enabled: true,
        commit_timeout_ms: 300,
        ..ActivitySettings::default()
    };
    f(&mut s);
    s
}

impl Harness {
    async fn new(settings: Option<ActivitySettings>) -> Harness {
        Harness::at(settings, None).await
    }

    /// `db_at`: the record's path, when a test needs one that cannot open.
    async fn at(settings: Option<ActivitySettings>, db_at: Option<PathBuf>) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let db = db_at.unwrap_or_else(|| dir.path().join(mae_daemon::activity::DB_FILE));
        let record = settings.map(|s| ActivityRecord::start(&db, s, SessionKey::generate()));
        let doc_store = fresh_doc_store().await;
        let owner = Arc::new(Identity::generate("owner"));
        let member = Identity::generate("member");
        let fp = member.fingerprint();
        seed_unencrypted_kb(
            &doc_store,
            &owner,
            "kb-a",
            Some((&fp, Role::Editor)),
            "n1",
            "Title One",
            SECRET,
            &[],
        )
        .await;
        let daemon = Arc::new(Identity::generate("daemon"));
        let self_issue = mae_daemon::oauth_self_issue::SelfIssueConfig {
            identity: Arc::clone(&daemon),
            audience: AUD.into(),
            ttl_secs: 3600,
        };
        let quota: Arc<dyn mae_daemon::quota::QuotaCharger> = Arc::new(mae_daemon::quota::NoQuota);
        let deps = mae_daemon::collab_handler::SessionDeps {
            doc_store: Arc::clone(&doc_store),
            broadcaster: Arc::new(std::sync::Mutex::new(
                mae_mcp::broadcast::EventBroadcaster::new(),
            )),
            start_time: Instant::now(),
            artifact_store: Arc::new(mae_daemon::artifact_store::NoArtifactStore),
            quota: Arc::clone(&quota),
            kb_query_limits: Default::default(),
        };
        let ws_config = crate::oauth_config::OAuthConfig {
            ws_enabled: true,
            ..Default::default()
        };
        let ws = crate::ws::state_from_config(&ws_config, Some(deps), true);
        assert!(ws.is_some());
        let blobs = Arc::new(crate::oauth_blobs::BlobRoutes::new(
            Arc::new(BlobStore::open(&dir.path().join("blobs")).unwrap()),
            test_limits(),
        ));
        let bindings = Arc::new(mae_daemon::identity_binding::BindingStore::open_memory().unwrap());
        let (rsa_pem, jwk) = rsa_key();
        let mut listener = Listener::new(config(), Some(doc_store), Some(self_issue), quota, ws)
            .unwrap()
            .with_blobs(Some(blobs))
            .with_identity_binding(
                Some(bindings),
                Some(crate::oauth_binding::BindingRoutes::new(300, 4)),
            )
            .with_activity(record.clone());
        listener.jwks = Some(JwksCache::preloaded(vec![jwk]));
        let addr = serve(Arc::new(listener)).await;
        Harness {
            addr,
            daemon,
            member,
            rsa_pem,
            record,
            db,
            _dir: dir,
        }
    }

    fn token(&self, ttl: u64) -> String {
        mae_daemon::oauth_self_issue::mint_self_token(
            &self.daemon,
            &self.member.fingerprint(),
            AUD,
            ttl,
        )
        .unwrap()
    }

    /// An identity-provider token for `sub` (unbound: no binding exists).
    fn idp_token(&self, sub: &str, extra: Value) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut claims = json!({"iss": ISS, "sub": sub, "aud": AUD, "exp": now + 600});
        for (k, v) in extra.as_object().unwrap() {
            claims[k] = v.clone();
        }
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(KID.into());
        encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(self.rsa_pem.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    async fn send(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: &str,
        extra: &str,
    ) -> Reply {
        let mut s = tokio::net::TcpStream::connect(self.addr).await.unwrap();
        let auth = token.map_or(String::new(), |t| format!("Authorization: Bearer {t}\r\n"));
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: test\r\n{auth}{extra}Content-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out).await;
        Reply::parse(&out)
    }

    async fn rpc(&self, token: Option<&str>, method: &str, params: Value) -> Reply {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        self.send(
            "POST",
            "/",
            token,
            &body.to_string(),
            "Content-Type: application/json\r\n",
        )
        .await
    }

    /// Every stored body, in `gseq` order.
    fn bodies(&self) -> Vec<Value> {
        bodies_at(&self.db)
    }
}

fn bodies_at(db: &Path) -> Vec<Value> {
    let Ok(db) = ActivityDb::open_read_only(db) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    db.for_each_row(1, u64::MAX >> 2, |r| {
        out.push(serde_json::from_slice(&r.body).unwrap());
        Ok(())
    })
    .unwrap();
    out
}

/// The listener's service over plain HTTP/1.1, with each request's peer
/// address put on it as the real accept loop does.
async fn serve(listener: Arc<Listener>) -> SocketAddr {
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp.local_addr().unwrap();
    let limiter = crate::conn_limit::ConnLimiter::new(4096);
    tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = tcp.accept().await else {
                return;
            };
            let guard = Arc::new(limiter.try_acquire().unwrap());
            let listener = Arc::clone(&listener);
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |mut req| {
                    req.extensions_mut()
                        .insert(crate::oauth_activity::PeerAddr(peer));
                    crate::oauth::serve(req, Arc::clone(&listener), Arc::clone(&guard))
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                    .with_upgrades()
                    .await;
            });
        }
    });
    addr
}

/// The request's own events: everything but the sign-ins and session starts
/// a request may open beside its own.
fn own(events: &[Value]) -> Vec<&Value> {
    events
        .iter()
        .filter(|e| !matches!(e["kind"].as_str(), Some("sign_in" | "session_start")))
        .collect()
}

/// Holds the record's write lock from another connection until dropped.
struct Held(ActivityDb);

impl Held {
    fn new(path: &Path) -> Held {
        let db = ActivityDb::open(path).unwrap();
        db.connection().execute("BEGIN IMMEDIATE").unwrap();
        Held(db)
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let _ = self.0.connection().execute("ROLLBACK");
    }
}

// --- completeness -----------------------------------------------------------

/// A request on each route. The `match` has no default arm: a route added to
/// [`Route`] does not compile until this test sends it.
fn samples(route: Route, token: &str) -> Vec<(&'static str, String, Option<String>, bool)> {
    let t = Some(token.to_string());
    match route {
        Route::Prm => vec![(
            "GET",
            "/.well-known/oauth-protected-resource".into(),
            None,
            false,
        )],
        Route::Health => vec![("GET", "/api/health".into(), None, false)],
        Route::SiteAsset => vec![(
            "GET",
            format!("{}x.css", mae_site::assets::PREFIX),
            None,
            false,
        )],
        Route::Blob => vec![
            ("POST", "/kb/kb-a/blobs".into(), t.clone(), true),
            (
                "GET",
                format!("/kb/kb-a/blobs/{}", "0".repeat(64)),
                t.clone(),
                true,
            ),
            (
                "HEAD",
                format!("/kb/kb-a/blobs/{}", "0".repeat(64)),
                t.clone(),
                true,
            ),
        ],
        Route::Ws => vec![
            ("POST", "/ws/ticket".into(), t.clone(), true),
            ("GET", "/ws".into(), t.clone(), true),
            ("OPTIONS", "/ws/ticket".into(), None, false),
        ],
        Route::Binding => vec![
            (
                "POST",
                crate::oauth_binding::CHALLENGE_PATH.into(),
                t.clone(),
                true,
            ),
            (
                "POST",
                crate::oauth_binding::BIND_PATH.into(),
                t.clone(),
                true,
            ),
        ],
        Route::Authenticated => vec![
            ("GET", "/kb/kb-a/n1".into(), t.clone(), true),
            ("GET", "/kb/kb-a/".into(), t.clone(), true),
            ("GET", "/kb/kb-a/tags".into(), t.clone(), true),
            ("GET", "/kb/kb-a/tags/x".into(), t.clone(), true),
            ("GET", "/kb/kb-a/index.json".into(), t.clone(), true),
            ("GET", "/kb/kb-a/explorer.json".into(), t.clone(), true),
            ("GET", "/kb/kb-a/site/x".into(), t.clone(), true),
            ("GET", "/kb/kb-a/a/b/c".into(), t.clone(), true),
            ("POST", "/".into(), t.clone(), true),
            ("POST", "/".into(), None, true),
        ],
    }
}

const ALL_ROUTES: [Route; 7] = [
    Route::Prm,
    Route::Health,
    Route::SiteAsset,
    Route::Blob,
    Route::Ws,
    Route::Binding,
    Route::Authenticated,
];

/// Every route and every JSON-RPC method the listener dispatches yields
/// exactly one event of its own with a classified outcome; the public surface
/// yields none. The methods come from the dispatchers' own lists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_route_and_method_yields_exactly_one_classified_event() {
    let h = Harness::new(Some(record_settings(|_| {}))).await;
    let token = h.token(3600);
    let mut seen = h.bodies().len();
    let check = |seen: &mut usize, label: String, recorded: bool, all: Vec<Value>| {
        let new: Vec<Value> = all[*seen..].to_vec();
        *seen = all.len();
        let mine = own(&new);
        if !recorded {
            assert!(new.is_empty(), "{label}: public, yet recorded {new:?}");
            return;
        }
        assert_eq!(mine.len(), 1, "{label}: {new:#?}");
        let e = mine[0];
        assert!(e["route"].is_string(), "{label}: {e}");
        assert!(EventKind::parse(e["kind"].as_str().unwrap()).is_some());
        assert!(matches!(
            e["outcome"]["result"].as_str(),
            Some("ok" | "refused" | "error")
        ));
        if e["outcome"]["result"] != "ok" {
            assert!(
                e["outcome"]["kind"].is_string(),
                "{label}: an unkinded refusal {e}"
            );
        }
    };
    for route in ALL_ROUTES {
        for (m, path, tok, recorded) in samples(route, &token) {
            let r = h.send(m, &path, tok.as_deref(), "", "").await;
            assert_ne!(r.status, 0, "{m} {path}");
            check(&mut seen, format!("{m} {path}"), recorded, h.bodies());
        }
    }
    let methods = mae_daemon::kb_query::METHODS
        .iter()
        .chain(mae_daemon::review::METHODS);
    for method in methods {
        let kind = classify_method(method).unwrap_or_else(|| panic!("{method} is not classed"));
        let params = json!({"kb_id": "kb-a", "node_id": "n1", "query": "Title"});
        h.rpc(Some(&token), method, params).await;
        let all = h.bodies();
        let e = own(&all[seen..])[0].clone();
        assert_eq!(e["route"], *method);
        assert_eq!(e["kind"], kind.as_str());
        check(&mut seen, method.to_string(), true, all);
    }
    // A method no dispatcher serves is recorded, as "unknown", never as its text.
    h.rpc(Some(&token), "kb/query.<script>", json!({})).await;
    let all = h.bodies();
    assert_eq!(own(&all[seen..])[0]["route"], "unknown");
}

/// The dispatchers' own match arms and their `METHODS` lists agree, both
/// ways: a method added to a dispatcher without its list (and so without a
/// class) fails here.
#[test]
fn the_method_lists_are_the_dispatchers_own_arms() {
    for (src, list, prefix) in [
        (
            include_str!("../kb_query.rs"),
            mae_daemon::kb_query::METHODS,
            "kb/query.",
        ),
        (
            include_str!("../review/mod.rs"),
            mae_daemon::review::METHODS,
            "kb/review.",
        ),
    ] {
        let mut arms: Vec<String> = Vec::new();
        for (i, _) in src.match_indices(&format!("\"{prefix}")) {
            let lit: String = src[i + 1..].chars().take_while(|c| *c != '"').collect();
            let after = src[i + 1 + lit.len() + 1..].trim_start();
            let before = src[..i].trim_end();
            let dispatched =
                after.starts_with("=>") || after.starts_with("if ") || before.ends_with("==");
            if dispatched && !arms.contains(&lit) {
                arms.push(lit);
            }
        }
        let mut listed: Vec<String> = list.iter().map(|s| s.to_string()).collect();
        arms.sort();
        listed.sort();
        assert_eq!(arms, listed, "{prefix}");
    }
}

// --- fail closed -------------------------------------------------------------

/// With the writer held, a read is refused `503 activity_record_unavailable`
/// with `Retry-After`, and nothing of the node reaches the caller; released,
/// the same read is answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_writer_refuses_a_read_and_sends_none_of_it() {
    let h = Harness::new(Some(record_settings(|_| {}))).await;
    let token = h.token(3600);
    let get = json!({"kb_id": "kb-a", "node_id": "n1"});
    let held = Held::new(&h.db);
    let r = h.rpc(Some(&token), "kb/query.get", get.clone()).await;
    assert_eq!(r.status, 503);
    assert_eq!(r.kind(), "activity_record_unavailable");
    assert!(r.header("retry-after").is_some());
    let text = String::from_utf8_lossy(&r.body);
    assert!(
        !text.contains(SECRET) && !text.contains("Title One"),
        "{text}"
    );
    drop(held);
    let r = h.rpc(Some(&token), "kb/query.get", get).await;
    assert_eq!(r.status, 200);
    assert!(String::from_utf8_lossy(&r.body).contains(SECRET));
}

// --- health (D13) ----------------------------------------------------------

async fn health(h: &Harness) -> (u16, Value) {
    let r = h.send("GET", "/api/health", None, "", "").await;
    (r.status, r.json())
}

/// `200 {"status":"ok"}` while the record takes events; `503
/// {"status":"unavailable"}` once the writer fails (a real SQLite error on
/// every insert); back to `200` by itself once a commit succeeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_fails_with_the_writer_and_recovers_by_itself() {
    let h = Harness::new(Some(record_settings(|_| {}))).await;
    let token = h.token(3600);
    assert_eq!(health(&h).await, (200, json!({"status": "ok"})));
    let saboteur = ActivityDb::open(&h.db).unwrap();
    saboteur
        .connection()
        .execute("CREATE TRIGGER fail BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT, 'disk I/O error'); END;")
        .unwrap();
    assert_eq!(
        h.rpc(Some(&token), "kb/query.my_kbs", json!({}))
            .await
            .status,
        503
    );
    assert_eq!(health(&h).await, (503, json!({"status": "unavailable"})));
    saboteur.connection().execute("DROP TRIGGER fail").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while health(&h).await.0 != 200 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(health(&h).await, (200, json!({"status": "ok"})));
}

/// A record that cannot be opened answers `503` from the first request, with
/// no recorded request needed to notice; a disabled record answers `200`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_is_503_from_the_start_when_the_record_cannot_open() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("a-file");
    std::fs::write(&blocked, b"").unwrap();
    let h = Harness::at(
        Some(record_settings(|_| {})),
        Some(blocked.join("activity.db")),
    )
    .await;
    assert_eq!(health(&h).await, (503, json!({"status": "unavailable"})));
    let off = Harness::new(None).await;
    assert_eq!(health(&off).await, (200, json!({"status": "ok"})));
}

// --- the session -----------------------------------------------------------

/// Two requests with one token share an id and one `session_start`; a new
/// token gets a new id and a new start; the header is the recorded id, of
/// D4's shape; a `401` carries none.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_session_header_is_the_recorded_id_and_follows_the_token() {
    let h = Harness::new(Some(record_settings(|_| {}))).await;
    let (a, refreshed) = (h.token(3600), h.token(1800));
    assert_ne!(a, refreshed);
    let ids: Vec<String> = {
        let mut v = Vec::new();
        for t in [&a, &a, &refreshed] {
            let r = h.rpc(Some(t), "kb/query.my_kbs", json!({})).await;
            v.push(r.header(session::HEADER).unwrap().to_string());
        }
        v
    };
    assert_eq!(ids[0], ids[1]);
    assert_ne!(ids[0], ids[2]);
    for id in &ids {
        assert!(session::is_session_id(id), "{id}");
    }
    let all = h.bodies();
    let starts: Vec<&Value> = all
        .iter()
        .filter(|e| e["kind"] == "session_start")
        .collect();
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0]["session"], ids[0]);
    assert_eq!(starts[1]["session"], ids[2]);
    let reads = own(&all);
    assert_eq!(
        reads
            .iter()
            .map(|e| e["session"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ids
    );
    let r = h
        .rpc(Some("not.a.token"), "kb/query.my_kbs", json!({}))
        .await;
    assert_eq!(r.status, 401);
    assert!(r.header(session::HEADER).is_none());
    // A refusal behind an established principal carries the id.
    let r = h
        .rpc(
            Some(&a),
            "kb/query.get",
            json!({"kb_id": "kb-zz", "node_id": "n"}),
        )
        .await;
    assert!(r.json()["error"]["data"]["kind"].is_string(), "a refusal");
    assert_eq!(r.header(session::HEADER), Some(ids[0].as_str()));
}

/// An identity-provider token: its `sid` is recorded beside the daemon's id
/// when present and `null` when not; a sign-in is recorded once per
/// `(iss, sub, auth_time)`, whatever the token's other claims.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idp_sign_in_is_recorded_once_and_sid_is_optional() {
    let h = Harness::new(Some(record_settings(|_| {}))).await;
    let first = h.idp_token(
        "alice",
        json!({"sid": "idp-session-1", "auth_time": 1_700_000_000, "amr": ["pwd", "mfa"]}),
    );
    let refreshed = h.idp_token("alice", json!({"auth_time": 1_700_000_000, "jti": "x"}));
    let again = h.idp_token("alice", json!({"auth_time": 1_700_009_999}));
    let mut ids = Vec::new();
    for t in [&first, &refreshed, &again] {
        let r = h.rpc(Some(t), "kb/query.my_kbs", json!({})).await;
        assert_eq!(r.status, 200);
        ids.push(r.header(session::HEADER).unwrap().to_string());
    }
    assert_ne!(ids[0], ids[1]);
    let all = h.bodies();
    let sign_ins: Vec<&Value> = all.iter().filter(|e| e["kind"] == "sign_in").collect();
    assert_eq!(sign_ins.len(), 2, "{sign_ins:#?}");
    assert_eq!(sign_ins[0]["detail"]["amr"], json!(["pwd", "mfa"]));
    assert_eq!(sign_ins[0]["detail"]["auth_time"], 1_700_000_000);
    assert!(
        !sign_ins[0]["peer"].is_null(),
        "a sign-in carries the address"
    );
    let reads = own(&all);
    assert_eq!(reads[0]["idp_sid"], "idp-session-1");
    assert!(reads[1]["idp_sid"].is_null());
    assert_eq!(reads[1]["session"], ids[1]);
    assert_eq!(reads[0]["principal"]["kind"], "unbound");
    assert_eq!(reads[0]["principal"]["iss"], ISS);
}

// --- the address -------------------------------------------------------------

fn fake_jwt(iss: &str, sub: &str) -> String {
    let b64 = |v: Value| data_encoding::BASE64URL_NOPAD.encode(v.to_string().as_bytes());
    format!(
        "{}.{}.sig",
        b64(json!({"alg": "RS256", "kid": KID})),
        b64(json!({"iss": iss, "sub": sub}))
    )
}

/// A refusal before authentication records the address and the token's
/// claims, labelled unverified; a successful read records neither. A
/// forwarded address is recorded only from a trusted proxy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_listener_records_the_address_only_where_the_rule_allows() {
    let h = Harness::new(Some(record_settings(|_| {}))).await;
    let xff = "X-Forwarded-For: 198.51.100.9\r\n";
    let bad = fake_jwt("https://evil.test", "mallory");
    let r = h.send("POST", "/", Some(&bad), "", xff).await;
    assert_eq!(r.status, 401);
    let ok = h.send("POST", "/", Some(&h.token(3600)), "", xff).await;
    assert_eq!(ok.status, 200);
    let all = h.bodies();
    let mine = own(&all);
    assert_eq!(mine[0]["outcome"]["result"], "refused");
    assert!(mine[0]["peer"]["addr"]
        .as_str()
        .unwrap()
        .starts_with("127.0.0.1:"));
    assert!(
        mine[0]["peer"]["forwarded"].is_null(),
        "untrusted peer's XFF recorded"
    );
    assert_eq!(
        mine[0]["unverified"],
        json!({"iss": "https://evil.test", "sub": "mallory"})
    );
    assert!(mine[0]["principal"]["id"].is_null());
    assert_eq!(mine[1]["outcome"]["result"], "ok");
    assert!(mine[1]["peer"].is_null());
    assert!(mine[1]["unverified"].is_null());

    let trusting = Harness::new(Some(record_settings(|s| {
        s.trusted_proxies = vec!["127.0.0.1".parse().unwrap()];
    })))
    .await;
    trusting.send("POST", "/", Some(&bad), "", xff).await;
    let all = trusting.bodies();
    assert_eq!(own(&all)[0]["peer"]["forwarded"], "198.51.100.9");
}

// --- not a credential --------------------------------------------------------

/// D4: no entry point reads the session header or accepts the id. Source:
/// the header's name appears only where it is defined and where it is set.
/// Behaviour: a valid id presented alone, or beside a bad token, is refused
/// like no credential at every authenticated route.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_entry_point_reads_the_session_header() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut uses = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e == "rs")
                && !p.ends_with("activity_listener_tests.rs")
            {
                let text = std::fs::read_to_string(&p).unwrap().to_ascii_lowercase();
                for needle in ["mae-activity-session", "session::header"] {
                    for _ in text.matches(needle) {
                        uses.push(p.strip_prefix(&root).unwrap().display().to_string());
                    }
                }
            }
        }
    }
    uses.sort();
    assert_eq!(
        uses,
        ["activity/session.rs", "activity/session.rs", "oauth_activity.rs", "oauth_activity.rs"],
        "the header is defined in session.rs (and named in its docs) and set, never read, in oauth_activity.rs"
    );

    let h = Harness::new(Some(record_settings(|_| {}))).await;
    let r = h
        .rpc(Some(&h.token(3600)), "kb/query.my_kbs", json!({}))
        .await;
    let id = r.header(session::HEADER).unwrap().to_string();
    let presented = format!("Mae-Activity-Session: {id}\r\n");
    for (m, path) in [
        ("POST", "/"),
        ("GET", "/kb/kb-a/n1"),
        ("POST", "/ws/ticket"),
        ("POST", "/kb/kb-a/blobs"),
        ("POST", crate::oauth_binding::CHALLENGE_PATH),
    ] {
        for token in [None, Some("not.a.token")] {
            let r = h.send(m, path, token, "", &presented).await;
            assert_eq!(r.status, 401, "{m} {path} with {token:?}");
            assert!(r.header(session::HEADER).is_none());
        }
    }
}

/// With the record off, no response carries a session id and nothing is
/// written: the listener is exactly what it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_the_record_off_nothing_is_recorded_or_sent() {
    let h = Harness::new(None).await;
    let r = h
        .rpc(
            Some(&h.token(3600)),
            "kb/query.get",
            json!({"kb_id": "kb-a", "node_id": "n1"}),
        )
        .await;
    assert_eq!(r.status, 200);
    assert!(r.header(session::HEADER).is_none());
    assert!(!h.db.exists());
}

// --- coalescing ----------------------------------------------------------------

/// A flood of bad tokens from one address: the first is committed before its
/// answer (it waits on a held writer), the rest are answered at once and
/// counted, and at a clean shutdown the counts sum to the number sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bad_token_flood_is_coalesced_and_its_first_waits_for_the_commit() {
    let h = Harness::new(Some(record_settings(|_| {}))).await;
    let n = 300;
    let held = Held::new(&h.db);
    let started = Instant::now();
    let first = h.send("POST", "/", Some("not.a.token"), "", "").await;
    assert_eq!(first.status, 503, "the first waited for its commit");
    assert!(started.elapsed() >= Duration::from_millis(280));
    let started = Instant::now();
    for _ in 1..n {
        assert_eq!(
            h.send("POST", "/", Some("not.a.token"), "", "")
                .await
                .status,
            401
        );
    }
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the rest did not wait"
    );
    drop(held);
    let record = h.record.clone().unwrap();
    record
        .shutdown(Event::daemon(
            EventKind::DaemonStop,
            mae_daemon::activity::now_us(),
        ))
        .await;
    let all = h.bodies();
    let refusals: Vec<&Value> = all
        .iter()
        .filter(|e| e["outcome"]["result"] == "refused")
        .collect();
    assert!(refusals.len() <= 3, "{} rows", refusals.len());
    let total: u64 = refusals.iter().map(|e| e["count"].as_u64().unwrap()).sum();
    assert_eq!(total, n);
}
