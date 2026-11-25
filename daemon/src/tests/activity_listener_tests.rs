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
/// A browser page's origin, allowed to mint a ticket.
const APP_ORIGIN: &str = "https://app.test";
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
        Harness::build(settings, db_at, None).await
    }

    /// `blob_hook`: the blob store's commit seam, for a test that pauses an
    /// upload mid-commit.
    async fn build(
        settings: Option<ActivitySettings>,
        db_at: Option<PathBuf>,
        blob_hook: Option<mae_daemon::blob_store::FaultHook>,
    ) -> Harness {
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
            ws_allowed_origins: vec![APP_ORIGIN.into()],
            ..Default::default()
        };
        let ws = crate::ws::state_from_config(&ws_config, Some(deps), true);
        assert!(ws.is_some());
        let mut store = BlobStore::open(&dir.path().join("blobs")).unwrap();
        if let Some(hook) = blob_hook {
            store = store.with_fault_hook(hook);
        }
        let blobs = Arc::new(crate::oauth_blobs::BlobRoutes::new(
            Arc::new(store),
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

/// The listener's service over plain TCP (HTTP/1.1, or HTTP/2 with prior
/// knowledge), through the real accept loop's own per-connection function
/// (`oauth::serve_connection`), so a client hanging up or resetting a stream
/// is handled as it is live.
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
            tokio::spawn(crate::oauth::serve_connection(
                stream, peer, listener, guard,
            ));
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
    // A token that validated (within the JWT leeway) and was then refused as
    // expired by the route: a principal exists, and a 401 still has no id.
    let stale = h.token(0);
    let r = h.send("POST", "/ws/ticket", Some(&stale), "", "").await;
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
    for p in rust_sources(&root) {
        let text = std::fs::read_to_string(&p).unwrap().to_ascii_lowercase();
        for needle in ["mae-activity-session", "session::header"] {
            let rel = p.strip_prefix(&root).unwrap().display().to_string();
            uses.extend(std::iter::repeat_n(rel, text.matches(needle).count()));
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

/// Every `.rs` file under `root` but this one.
fn rust_sources(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for p in std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()) {
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e == "rs")
                && !p.ends_with("activity_listener_tests.rs")
            {
                out.push(p);
            }
        }
    }
    out
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
/// answer (it waits on a held writer, and is answered only once released),
/// the rest are answered at once and counted while it waits, and at a clean
/// shutdown the counts sum to the number sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bad_token_flood_is_coalesced_and_its_first_waits_for_the_commit() {
    let h = Harness::new(Some(record_settings(|s| s.commit_timeout_ms = 10_000))).await;
    let n = 300;
    let held = Held::new(&h.db);
    let started = Instant::now();
    let ((first, answered_at), released_at) = tokio::join!(
        async {
            let r = h.send("POST", "/", Some("not.a.token"), "", "").await;
            (r, started.elapsed())
        },
        async {
            // The first has opened its window and is waiting on its commit.
            tokio::time::sleep(Duration::from_millis(300)).await;
            let rest = Instant::now();
            for _ in 1..n {
                assert_eq!(
                    h.send("POST", "/", Some("not.a.token"), "", "")
                        .await
                        .status,
                    401
                );
            }
            assert!(
                rest.elapsed() < Duration::from_secs(9),
                "the rest did not wait"
            );
            let released_at = started.elapsed();
            drop(held);
            released_at
        }
    );
    assert_eq!(first.status, 401);
    assert!(
        answered_at >= released_at,
        "the first was answered only once its event could commit"
    );
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

// --- the /ws upgrade -----------------------------------------------------------

/// Send a WebSocket upgrade with `credential` (header lines) and return the
/// response head, without waiting for the socket to close.
async fn upgrade(h: &Harness, credential: &str) -> Reply {
    let mut s = tokio::net::TcpStream::connect(h.addr).await.unwrap();
    let req = format!(
        "GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n{credential}\r\n"
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let mut buf = [0u8; 1024];
    while !out.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = s.read(&mut buf).await.unwrap();
        assert!(n > 0, "closed before a response");
        out.extend_from_slice(&buf[..n]);
    }
    Reply::parse(&out)
}

/// D4: the `/ws` upgrade carries its connection's own id, not the bearer
/// token's, and that connection's start is recorded with the address; a
/// redeemed ticket is recorded as a sign-in the daemon performed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_upgrade_carries_the_connections_id_and_a_ticket_is_a_sign_in() {
    let h = Harness::new(Some(record_settings(|_| {}))).await;
    let token = h.token(3600);
    let bearer_id = h
        .rpc(Some(&token), "kb/query.my_kbs", json!({}))
        .await
        .header(session::HEADER)
        .unwrap()
        .to_string();
    let r = upgrade(
        &h,
        &format!("Authorization: Bearer {token}\r\nSec-WebSocket-Protocol: mae.v1\r\n"),
    )
    .await;
    assert_eq!(r.status, 101);
    let conn_id = r.header(session::HEADER).unwrap().to_string();
    assert!(session::is_session_id(&conn_id));
    assert_ne!(conn_id, bearer_id, "a connection is its own session");
    let all = h.bodies();
    let start = all
        .iter()
        .find(|e| e["kind"] == "session_start" && e["session"] == conn_id.as_str())
        .expect("the connection's start");
    assert_eq!(start["detail"]["credential"], "connection");
    assert!(!start["peer"].is_null());
    assert_eq!(own(&all).last().unwrap()["session"], conn_id.as_str());

    let ticket = h
        .send("POST", "/ws/ticket", Some(&token), "", "")
        .await
        .json()["ticket"]
        .as_str()
        .unwrap()
        .to_string();
    let before = h.bodies().len();
    let r = upgrade(
        &h,
        &format!("Sec-WebSocket-Protocol: mae.v1, mae.ticket.{ticket}\r\n"),
    )
    .await;
    assert_eq!(r.status, 101);
    let all = h.bodies();
    let new = &all[before..];
    let sign_in = new
        .iter()
        .find(|e| e["kind"] == "sign_in")
        .expect("a sign-in");
    assert_eq!(sign_in["detail"]["method"], "ws_ticket");
    assert_eq!(sign_in["principal"]["id"], h.member.fingerprint());
    assert_eq!(sign_in["principal"]["kind"], "service");
}

// --- a client that hangs up mid-request (D5/D6) -------------------------------

/// A client that hangs up while its upload is mid-commit. The commit runs on a
/// blocking thread, so the blob lands whatever the connection does; its event
/// must land with it, as the success it was, never be lost with the
/// connection (D6: no effect without its event).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_hanging_up_mid_upload_leaves_the_effect_and_its_event() {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let seam = std::sync::Mutex::new((entered_tx, go_rx));
    let hook: mae_daemon::blob_store::FaultHook = Arc::new(move |step| {
        if step == mae_daemon::blob_store::CommitStep::BeforeRename {
            let seam = seam.lock().unwrap();
            let _ = seam.0.send(());
            let _ = seam.1.recv_timeout(Duration::from_secs(20));
        }
        false
    });
    let settings = record_settings(|s| s.commit_timeout_ms = 10_000);
    let h = Harness::build(Some(settings), None, Some(hook)).await;
    let body = super::blob_route_tests::png(4, 4, 7);
    let head = format!(
        "POST /kb/kb-a/blobs HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer {}\r\n\
         Content-Length: {}\r\n\r\n",
        h.token(3600),
        body.len()
    );
    let mut s = tokio::net::TcpStream::connect(h.addr).await.unwrap();
    s.write_all(head.as_bytes()).await.unwrap();
    s.write_all(&body).await.unwrap();
    tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(20)))
        .await
        .unwrap()
        .expect("the upload reached its commit");
    // The client hangs up while the handler is paused mid-commit.
    drop(s);
    tokio::time::sleep(Duration::from_millis(300)).await;
    go_tx.send(()).unwrap();

    let rows_at = h._dir.path().join("blobs").join("blobs.db");
    let deadline = Instant::now() + Duration::from_secs(10);
    let (rows, uploads) = loop {
        let rows = mae_daemon::blob_store::list_rows(&rows_at).unwrap_or_default();
        let uploads: Vec<Value> = h
            .bodies()
            .into_iter()
            .filter(|e| e["kind"] == "upload")
            .collect();
        if (!rows.is_empty() && !uploads.is_empty()) || Instant::now() > deadline {
            break (rows, uploads);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(rows.len(), 1, "the effect landed: {rows:?}");
    assert_eq!(uploads.len(), 1, "the effect's event: {uploads:#?}");
    let e = &uploads[0];
    assert_eq!(e["outcome"]["result"], "ok", "{e}");
    assert_eq!(e["blob"].as_str(), Some(rows[0].1.as_str()), "{e}");
}

// --- node_ids is bounded (D2) -------------------------------------------------

/// A result naming 10,000 nodes (each twice) records a bounded body: the first
/// `NODE_IDS_MAX` ids in order, and `node_ids_total` with the true number.
/// The draft is bounded too, not only the encoder's output.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_result_of_ten_thousand_ids_records_a_bounded_body_and_the_true_count() {
    use mae_daemon::activity::event::NODE_IDS_MAX;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join(mae_daemon::activity::DB_FILE);
    let record = ActivityRecord::start(&db, record_settings(|_| {}), SessionKey::generate());
    let ids: Vec<String> = (0..10_000).map(|i| format!("node-{i:05}")).collect();
    let answer = json!({"jsonrpc": "2.0", "id": 1, "result": {
        "results": ids.iter().map(|i| json!({"id": i})).collect::<Vec<_>>(),
        "links_to": ids,
    }});
    let (entry, ev) = crate::oauth_activity::built_in_scope(Arc::clone(&record), || {
        crate::oauth_activity::rpc("kb/query.search", &json!({"kb_id": "kb-a"}));
        crate::oauth_activity::rpc_answer(&answer);
    });
    let kept = ev.node_ids.clone().unwrap();
    assert_eq!(kept.len(), NODE_IDS_MAX, "the draft itself is bounded");
    assert_eq!(kept, ids[..NODE_IDS_MAX].to_vec());
    assert_eq!(ev.node_ids_total, Some(10_000));
    record.commit(entry).await.unwrap();
    let stored = bodies_at(&db);
    let body = stored
        .iter()
        .find(|e| e["route"] == "kb/query.search")
        .expect("the event committed");
    assert_eq!(body["node_ids"].as_array().unwrap().len(), NODE_IDS_MAX);
    assert_eq!(body["node_ids"][0], "node-00000");
    assert_eq!(body["node_ids_total"], 10_000);
    let size = serde_json::to_vec(body).unwrap().len();
    assert!(size < 8_192, "a bounded body, {size} bytes");

    // Under the cap: every id, and no total.
    let small = json!({"result": {"results": [{"id": "a"}, {"id": "b"}, {"id": "a"}]}});
    let (_, ev) = crate::oauth_activity::built_in_scope(Arc::clone(&record), || {
        crate::oauth_activity::rpc("kb/query.get", &json!({"kb_id": "kb-a", "node_id": "a"}));
        crate::oauth_activity::rpc_answer(&small);
    });
    assert_eq!(ev.node_ids, Some(vec!["a".to_string(), "b".to_string()]));
    assert_eq!(ev.node_ids_total, None);
}

// --- refusals during an outage (D5/D6) ----------------------------------------

/// Wait until `GET /api/health` answers 200, within five seconds.
async fn until_healthy(h: &Harness) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while health(h).await.0 != 200 {
        assert!(Instant::now() < deadline, "the record never recovered");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// With the writer held, a refusal whose first could not be recorded leaves
/// no window behind: the next bad token from the same address meets the
/// record's `503` too, never its real `401`. Recovered, it gets the `401`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_a_failed_first_refusal_the_next_meets_the_records_refusal_too() {
    let h = Harness::new(Some(record_settings(|_| {}))).await;
    let held = Held::new(&h.db);
    for label in ["first", "second"] {
        let r = h.send("POST", "/", Some("not.a.token"), "", "").await;
        assert_eq!(r.status, 503, "{label}");
        assert_eq!(r.kind(), "activity_record_unavailable", "{label}");
    }
    drop(held);
    until_healthy(&h).await;
    let r = h.send("POST", "/", Some("not.a.token"), "", "").await;
    assert_eq!(r.status, 401);
}

/// The record's `503` on `POST /ws/ticket` carries the CORS header that
/// route's own answers carry, so a browser page can read why; a disallowed
/// origin, and every other route, get none.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_records_503_carries_the_routes_cors_header() {
    let h = Harness::new(Some(record_settings(|_| {}))).await;
    let token = h.token(3600);
    let with_origin = |o: &str| format!("Origin: {o}\r\n");
    let held = Held::new(&h.db);
    let r = h
        .send(
            "POST",
            "/ws/ticket",
            Some(&token),
            "",
            &with_origin(APP_ORIGIN),
        )
        .await;
    assert_eq!(r.status, 503);
    assert_eq!(r.kind(), "activity_record_unavailable");
    assert_eq!(r.header("access-control-allow-origin"), Some(APP_ORIGIN));
    assert_eq!(r.header("vary"), Some("Origin"));
    let r = h
        .send(
            "POST",
            "/ws/ticket",
            Some(&token),
            "",
            &with_origin("https://elsewhere.test"),
        )
        .await;
    assert_eq!(r.status, 503);
    assert_eq!(r.header("access-control-allow-origin"), None);
    let r = h
        .send("POST", "/", Some(&token), "", &with_origin(APP_ORIGIN))
        .await;
    assert_eq!(r.status, 503);
    assert_eq!(r.header("access-control-allow-origin"), None);
    drop(held);
    // The route's own answer, for comparison: the same header.
    until_healthy(&h).await;
    let r = h
        .send(
            "POST",
            "/ws/ticket",
            Some(&token),
            "",
            &with_origin(APP_ORIGIN),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("access-control-allow-origin"), Some(APP_ORIGIN));
}

// --- detached requests stay bounded (D5) ---------------------------------------

/// A `kb/query.my_kbs` call over HTTP/2 on `sender`.
async fn h2_call(
    sender: &mut hyper::client::conn::http2::SendRequest<http_body_util::Full<bytes::Bytes>>,
    token: &str,
) -> hyper::Result<hyper::Response<hyper::body::Incoming>> {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "kb/query.my_kbs", "params": {}});
    let req = hyper::Request::post("http://test/")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(http_body_util::Full::new(bytes::Bytes::from(
            body.to_string(),
        )))
        .unwrap();
    sender.send_request(req).await
}

/// Wait until `f` holds, within five seconds.
async fn until(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f() {
        assert!(Instant::now() < deadline, "never: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// One HTTP/2 connection, a held writer: as many requests as the connection's
/// limit wait on their commits, then their streams are reset, which frees the
/// client's streams but not the requests, which run on to their commits. The
/// next requests on that connection are refused `429` at once instead of
/// piling up, the requests in flight never pass the limit, and every request
/// is recorded: the first as their own events, the refused in one aggregate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resetting_streams_never_holds_more_than_the_connection_limit() {
    let limit = crate::oauth::MAX_REQUESTS_PER_CONNECTION as usize;
    let h = Harness::new(Some(record_settings(|s| s.commit_timeout_ms = 10_000))).await;
    let record = h.record.clone().unwrap();
    let token = h.token(3600);
    let held = Held::new(&h.db);
    let tcp = tokio::net::TcpStream::connect(h.addr).await.unwrap();
    // Without it, each request's HEADERS and DATA frames meet Nagle's
    // algorithm and a delayed ACK, tens of milliseconds a request.
    tcp.set_nodelay(true).unwrap();
    let (sender, conn) = hyper::client::conn::http2::handshake(
        hyper_util::rt::TokioExecutor::new(),
        hyper_util::rt::TokioIo::new(tcp),
    )
    .await
    .expect("the listener speaks HTTP/2 with prior knowledge");
    tokio::spawn(conn);

    let waiting: Vec<_> = (0..limit)
        .map(|_| {
            let (mut sender, token) = (sender.clone(), token.clone());
            tokio::spawn(async move { h2_call(&mut sender, &token).await })
        })
        .collect();
    until("the first requests are all in flight", || {
        record.requests_in_flight() == limit
    })
    .await;
    // Reset every stream: the client may open as many again.
    for w in &waiting {
        w.abort();
    }
    for w in waiting {
        let _ = w.await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(record.requests_in_flight(), limit, "they run on, detached");

    for i in 0..limit {
        let mut sender = sender.clone();
        let r = tokio::time::timeout(Duration::from_secs(5), h2_call(&mut sender, &token))
            .await
            .unwrap_or_else(|_| panic!("request {i} was taken on, not refused"))
            .unwrap();
        assert_eq!(r.status(), 429, "request {i}");
        assert!(record.requests_in_flight() <= limit);
    }

    drop(held);
    until("the detached requests have committed", || {
        record.requests_in_flight() == 0
    })
    .await;
    record
        .shutdown(Event::daemon(
            EventKind::DaemonStop,
            mae_daemon::activity::now_us(),
        ))
        .await;
    let all = h.bodies();
    let answered = all
        .iter()
        .filter(|e| e["route"] == "kb/query.my_kbs" && e["outcome"]["result"] == "ok")
        .count();
    assert_eq!(answered, limit, "each reset request's own event");
    let refused: u64 = all
        .iter()
        .filter(|e| e["outcome"]["kind"] == crate::oauth_activity::TOO_MANY_ON_CONNECTION)
        .map(|e| e["count"].as_u64().unwrap())
        .sum();
    assert_eq!(refused, limit as u64, "every refusal counted");
}

/// A clean stop with an upload paused after its effect (the blob's row is in):
/// a request arriving meanwhile is refused `503` and counted into an
/// aggregate, the stop waits, and once the upload goes on its event is
/// committed before `daemon_stop`, as the success it was, and so is the
/// refusal's count.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_waits_for_a_request_in_flight_to_record_its_event() {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let seam = std::sync::Mutex::new((entered_tx, go_rx));
    let hook: mae_daemon::blob_store::FaultHook = Arc::new(move |step| {
        if step == mae_daemon::blob_store::CommitStep::AfterRow {
            let seam = seam.lock().unwrap();
            let _ = seam.0.send(());
            let _ = seam.1.recv_timeout(Duration::from_secs(20));
        }
        false
    });
    let h = Arc::new(Harness::build(Some(record_settings(|_| {})), None, Some(hook)).await);
    let record = h.record.clone().unwrap();
    let token = h.token(3600);
    let png = super::blob_route_tests::png(4, 4, 11);
    let upload = {
        let (h, token) = (Arc::clone(&h), token.clone());
        tokio::spawn(async move {
            let mut s = tokio::net::TcpStream::connect(h.addr).await.unwrap();
            let head = format!(
                "POST /kb/kb-a/blobs HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer {token}\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n",
                png.len()
            );
            s.write_all(head.as_bytes()).await.unwrap();
            s.write_all(&png).await.unwrap();
            let mut out = Vec::new();
            let _ = s.read_to_end(&mut out).await;
            Reply::parse(&out)
        })
    };
    tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(20)))
        .await
        .unwrap()
        .expect("the upload reached its effect");
    let stop = {
        let record = Arc::clone(&record);
        tokio::spawn(async move {
            record
                .shutdown(Event::daemon(
                    EventKind::DaemonStop,
                    mae_daemon::activity::now_us(),
                ))
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!stop.is_finished(), "the stop waits for the upload");
    let late = h.rpc(Some(&token), "kb/query.my_kbs", json!({})).await;
    assert_eq!(late.status, 503, "nothing new is taken once the stop began");
    assert_eq!(late.kind(), "activity_record_unavailable");
    go_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), stop)
        .await
        .expect("the stop ends once the upload has committed")
        .unwrap();
    assert_eq!(upload.await.unwrap().status, 200);

    let all = h.bodies();
    let at = |kind: &str| all.iter().position(|e| e["kind"] == kind);
    let upload_at = at("upload").expect("the upload's event");
    let stop_at = at("daemon_stop").expect("daemon_stop");
    assert!(upload_at < stop_at, "the upload's event comes first");
    assert_eq!(all[upload_at]["outcome"]["result"], "ok");
    let late_at = all
        .iter()
        .position(|e| {
            e["outcome"]["result"] == "refused"
                && e["outcome"]["kind"] == "activity_record_unavailable"
        })
        .expect("the refused late request is counted, never dropped");
    assert_eq!(all[late_at]["count"], 1);
    assert!(late_at < stop_at, "and counted before daemon_stop");
    assert!(
        !all.iter()
            .any(|e| e["route"] == "kb/query.my_kbs" && e["outcome"]["result"] == "ok"),
        "the late request was not taken on"
    );
}

/// An outage (the writer held past the commit timeout) and a flood of bad
/// tokens from one address in waves: once the record is unhealthy, each is
/// answered `503` at once and nothing more is queued; after recovery the
/// refusals are a bounded number of rows whose counts sum to the flood.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bad_token_flood_during_an_outage_queues_a_bounded_number() {
    let (waves, per_wave) = (5, 10);
    let h = Harness::new(Some(record_settings(|s| s.commit_timeout_ms = 300))).await;
    let record = h.record.clone().unwrap();
    let held = Held::new(&h.db);
    let first = h.send("POST", "/", Some("not.a.token"), "", "").await;
    assert_eq!(first.status, 503);
    assert_eq!(health(&h).await.0, 503, "the record is unhealthy");
    for _ in 0..waves {
        let mut replies = Vec::new();
        for _ in 0..per_wave {
            replies.push(h.send("POST", "/", Some("not.a.token"), "", "").await);
        }
        for r in replies {
            assert_eq!(r.status, 503);
            assert_eq!(r.kind(), "activity_record_unavailable");
        }
    }
    let depth = record.status().queue_depth;
    assert!(depth <= 1, "only the first is queued, {depth} events");
    drop(held);
    until_healthy(&h).await;
    record
        .shutdown(Event::daemon(
            EventKind::DaemonStop,
            mae_daemon::activity::now_us(),
        ))
        .await;
    let refusals: Vec<Value> = h
        .bodies()
        .into_iter()
        .filter(|e| e["outcome"]["result"] == "refused")
        .collect();
    assert!(
        refusals.len() <= 2,
        "{} rows: {refusals:#?}",
        refusals.len()
    );
    let total: u64 = refusals.iter().map(|e| e["count"].as_u64().unwrap()).sum();
    assert_eq!(total, 1 + waves * per_wave);
    assert!(refusals
        .iter()
        .all(|e| e["outcome"]["kind"] == "activity_record_unavailable"));
}
