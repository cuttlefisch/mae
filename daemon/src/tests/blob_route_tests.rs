//! ADR-115 P1's routes (`crate::oauth_blobs`) over a real HTTP/1.1 connection
//! to the real listener's service, with real self-issued tokens and a real
//! `DocStore`: what each caller gets, byte for byte where it matters.
//!
//! The client is a raw TCP socket, so a test can send exactly the framing an
//! attacker would: no `Content-Length`, a false one, a body cut short.

use std::net::SocketAddr;
use std::sync::Arc;

use mae_daemon::blob_store::{BlobStore, StorageQuota};
use mae_daemon::doc_store::DocStore;
use mae_daemon::quota::{QuotaCharger, QuotaLease};
use mae_mcp::identity::Identity;
use mae_sync::kb::{KbCollectionDoc, Role, TransportPolicy};
use sha2::Digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::kb_query_tests::{fresh_doc_store, seed_e2e_kb};
use crate::oauth::{Listener, ResourceServerConfig};
use crate::oauth_blobs::BlobRoutes;

const AUD: &str = "https://mae.test";
const BY: mae_daemon::doc_editors::Attribution<'static> =
    mae_daemon::doc_editors::Attribution::Principal("test");
/// The test cap: small, so "ten times the cap" is cheap to send.
const CAP: u64 = 4096;

/// A storage quota for every principal (or none), and request charging that
/// admits or refuses everything.
struct TestQuota {
    storage: Option<u64>,
    refuse_requests: bool,
}

impl QuotaCharger for TestQuota {
    fn charge(&self, _p: Option<&str>, _m: &str) -> Result<QuotaLease, String> {
        if self.refuse_requests {
            return Err("over the request budget".into());
        }
        Ok(QuotaLease::none())
    }

    fn storage_quota(&self, _principal: &str) -> Option<StorageQuota> {
        self.storage.map(|max_bytes| StorageQuota {
            tenant: "team".into(),
            max_bytes,
        })
    }
}

struct People {
    editor: Identity,
    viewer: Identity,
    /// An editor of `kb-b` only.
    other_editor: Identity,
    stranger: Identity,
    e2e_member: Identity,
}

struct Harness {
    addr: SocketAddr,
    /// Connections the server has finished with, everything they held dropped.
    closed: tokio::sync::watch::Receiver<usize>,
    daemon: Arc<Identity>,
    routes: Arc<BlobRoutes>,
    people: People,
    _dir: tempfile::TempDir,
}

async fn seed_kb(doc_store: &DocStore, owner: &Identity, kb_id: &str, members: &[(&str, Role)]) {
    let mut coll = KbCollectionDoc::new_owned(kb_id, &owner.fingerprint(), "owner");
    coll.set_transport_policy(TransportPolicy::Hub);
    for (p, role) in members {
        coll.upsert_member(p, "member", *role);
    }
    doc_store
        .share_doc(&format!("kbc:{kb_id}"), &coll.encode_state(), BY)
        .await
        .unwrap();
}

fn config() -> ResourceServerConfig {
    ResourceServerConfig {
        canonical_resource_uri: AUD.into(),
        principal_claim: "sub".into(),
        groups_claim: "groups".into(),
        allowed_groups: Vec::new(),
        jwks_url: String::new(),
        jwks_ca_file: None,
        issuer: None,
        kb_query_enabled: true,
        site_enabled: true,
        site_locale: "en-US".into(),
        site_hidden_namespaces: Vec::new(),
        site_home: Default::default(),
        site_title: Default::default(),
        max_request_body_bytes: 1024,
        kb_query_max_body_bytes: 65_536,
        kb_query_max_scan_nodes: 500,
        kb_query_max_search_results: 20,
        second_factor: Default::default(),
    }
}

async fn harness_with(quota: TestQuota, max_pixels: u64) -> Harness {
    let doc_store = fresh_doc_store().await;
    let owner = Arc::new(Identity::generate("owner"));
    doc_store.set_signer(Arc::clone(&owner));
    let people = People {
        editor: Identity::generate("editor"),
        viewer: Identity::generate("viewer"),
        other_editor: Identity::generate("other"),
        stranger: Identity::generate("stranger"),
        e2e_member: Identity::generate("e2e"),
    };
    seed_kb(
        &doc_store,
        &owner,
        "kb-a",
        &[
            (&people.editor.fingerprint(), Role::Editor),
            (&people.viewer.fingerprint(), Role::Viewer),
        ],
    )
    .await;
    seed_kb(
        &doc_store,
        &owner,
        "kb-b",
        &[
            (&people.editor.fingerprint(), Role::Editor),
            (&people.other_editor.fingerprint(), Role::Editor),
        ],
    )
    .await;
    let e2e_fp = people.e2e_member.fingerprint();
    seed_e2e_kb(
        &doc_store,
        &owner,
        "kb-e2e",
        &e2e_fp,
        &people.e2e_member,
        "n1",
        "T",
        "b",
    )
    .await;

    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(BlobStore::open(&dir.path().join("blobs")).unwrap());
    let routes = Arc::new(BlobRoutes::new(store, CAP, max_pixels));
    let daemon = Arc::new(Identity::generate("daemon"));
    let self_issue = mae_daemon::oauth_self_issue::SelfIssueConfig {
        identity: Arc::clone(&daemon),
        audience: AUD.into(),
        ttl_secs: 3600,
    };
    let listener = Listener::new(
        config(),
        Some(doc_store),
        Some(self_issue),
        Arc::new(quota),
        None,
    )
    .unwrap()
    .with_blobs(Some(Arc::clone(&routes)));
    let (addr, closed) = serve(Arc::new(listener)).await;
    Harness {
        addr,
        closed,
        daemon,
        routes,
        people,
        _dir: dir,
    }
}

async fn harness() -> Harness {
    harness_with(
        TestQuota {
            storage: None,
            refuse_requests: false,
        },
        40_000_000,
    )
    .await
}

/// The listener's own service, over plain HTTP/1.1 on a loopback port.
async fn serve(listener: Arc<Listener>) -> (SocketAddr, tokio::sync::watch::Receiver<usize>) {
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp.local_addr().unwrap();
    let limiter = crate::conn_limit::ConnLimiter::new(1024);
    let (closed_tx, closed) = tokio::sync::watch::channel(0usize);
    let closed_tx = Arc::new(closed_tx);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = tcp.accept().await else {
                return;
            };
            let guard = Arc::new(limiter.try_acquire().unwrap());
            let listener = Arc::clone(&listener);
            let closed_tx = Arc::clone(&closed_tx);
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |req| {
                    crate::oauth::serve(req, Arc::clone(&listener), Arc::clone(&guard))
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                    .await;
                closed_tx.send_modify(|n| *n += 1);
            });
        }
    });
    (addr, closed)
}

impl Harness {
    fn token(&self, who: &Identity) -> String {
        mae_daemon::oauth_self_issue::mint_self_token(&self.daemon, &who.fingerprint(), AUD, 3600)
            .unwrap()
    }

    /// Send `head` (request line and headers, without the blank line), then
    /// `body`, then half-close; return the whole response.
    async fn raw(&self, head: &str, body: &[u8]) -> Reply {
        self.raw_then(head, body, false).await
    }

    /// As [`Harness::raw`]; `hang_up` closes the client's sending side after
    /// the body (hyper then drops the connection, as for a client cut off).
    async fn raw_then(&self, head: &str, body: &[u8], hang_up: bool) -> Reply {
        let mut s = tokio::net::TcpStream::connect(self.addr).await.unwrap();
        let req = format!("{head}\r\nHost: test\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).await.unwrap();
        let _ = s.write_all(body).await;
        if hang_up {
            let _ = s.shutdown().await;
        }
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out).await;
        Reply::parse(&out)
    }

    async fn upload(&self, who: &Identity, kb: &str, body: &[u8], extra: &str) -> Reply {
        let head = format!(
            "POST /kb/{kb}/blobs HTTP/1.1\r\nAuthorization: Bearer {}\r\nContent-Length: {}{extra}",
            self.token(who),
            body.len()
        );
        self.raw(&head, body).await
    }

    async fn fetch(&self, method: &str, who: &Identity, kb: &str, sha: &str, extra: &str) -> Reply {
        let head = format!(
            "{method} /kb/{kb}/blobs/{sha} HTTP/1.1\r\nAuthorization: Bearer {}{extra}",
            self.token(who)
        );
        self.raw(&head, b"").await
    }

    fn rows(&self) -> Vec<(String, String)> {
        self.routes.store.list_all().unwrap()
    }

    fn body_bytes(&self) -> u64 {
        self.routes
            .counters
            .body_bytes
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Reply {
    status: u16,
    /// Lowercased names, in order, without `date` (the only header that varies).
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn parse(raw: &[u8]) -> Reply {
        if raw.is_empty() {
            // The server dropped the connection without answering.
            return Reply {
                status: 0,
                headers: Vec::new(),
                body: Vec::new(),
            };
        }
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap_or_else(|| panic!("no response: {:?}", String::from_utf8_lossy(raw)));
        let head = String::from_utf8_lossy(&raw[..split]).to_string();
        let mut lines = head.split("\r\n");
        let status = lines
            .next()
            .unwrap()
            .split(' ')
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let headers = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
            .filter(|(k, _)| k != "date")
            .collect();
        Reply {
            status,
            headers,
            body: raw[split + 4..].to_vec(),
        }
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&self.body)))
    }

    fn kind(&self) -> String {
        self.json()["kind"].as_str().unwrap_or("").to_string()
    }
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(bytes))
}

fn png(w: u32, h: u32, seed: u8) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x as u8).wrapping_mul(seed), y as u8, seed])
    });
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

/// A PNG with a text chunk after IHDR, so what is stored differs from what
/// was sent.
fn png_with_text(seed: u8) -> Vec<u8> {
    let plain = png(8, 8, seed);
    let data = b"Comment\0secret";
    let mut chunk = (data.len() as u32).to_be_bytes().to_vec();
    chunk.extend_from_slice(b"tEXt");
    chunk.extend_from_slice(data);
    let crc_in = &chunk[4..];
    let mut c = 0xFFFF_FFFFu32;
    for &b in crc_in {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    chunk.extend_from_slice(&(!c).to_be_bytes());
    [&plain[..33], &chunk[..], &plain[33..]].concat()
}

// ---------------------------------------------------------------------------
// Access
// ---------------------------------------------------------------------------

/// D2: a caller the KB's gate does not admit gets one answer, byte for byte,
/// whether the blob is this KB's, another KB's, nobody's, or not an address,
/// and whether the KB exists at all.
#[tokio::test]
async fn a_non_reader_gets_one_identical_refusal_whatever_exists() {
    let h = harness().await;
    let in_a = h
        .upload(&h.people.editor, "kb-a", &png(4, 4, 1), "")
        .await
        .json();
    let in_b = h
        .upload(&h.people.other_editor, "kb-b", &png(4, 4, 2), "")
        .await
        .json();
    let (a, b) = (
        in_a["sha256"].as_str().unwrap(),
        in_b["sha256"].as_str().unwrap(),
    );
    let nobody = sha(b"never uploaded");
    for method in ["GET", "HEAD"] {
        let mut replies = Vec::new();
        for (who, kb, blob) in [
            (&h.people.stranger, "kb-a", a),
            (&h.people.stranger, "kb-a", b),
            (&h.people.stranger, "kb-a", nobody.as_str()),
            (&h.people.stranger, "kb-a", "not-an-address"),
            (&h.people.stranger, "kb-nowhere", a),
            (&h.people.stranger, "Not_A_Kb", a),
            (&h.people.other_editor, "kb-a", a),
            (&h.people.other_editor, "kb-a", nobody.as_str()),
        ] {
            replies.push(h.fetch(method, who, kb, blob, "").await);
        }
        assert_eq!(replies[0].status, 403, "{method}");
        for r in &replies[1..] {
            assert_eq!(r, &replies[0], "{method}: every refusal is the same");
        }
        if method == "GET" {
            assert_eq!(replies[0].kind(), "not_member");
        }
    }
}

#[tokio::test]
async fn a_member_of_one_kb_cannot_read_anothers_blob_by_its_hash() {
    let h = harness().await;
    let in_a = h
        .upload(&h.people.editor, "kb-a", &png(4, 4, 3), "")
        .await
        .json();
    let a = in_a["sha256"].as_str().unwrap();
    // Through their own KB: the address is not that KB's.
    let own = h.fetch("GET", &h.people.other_editor, "kb-b", a, "").await;
    assert_eq!((own.status, own.kind().as_str()), (404, "blob_missing"));
    // Through the other KB: not a reader.
    let other = h.fetch("GET", &h.people.other_editor, "kb-a", a, "").await;
    assert_eq!((other.status, other.kind().as_str()), (403, "not_member"));
    // A reader of kb-a does get it.
    let viewer = h.fetch("GET", &h.people.viewer, "kb-a", a, "").await;
    assert_eq!(viewer.status, 200);
}

#[tokio::test]
async fn a_viewer_cannot_upload_and_a_stranger_is_not_told_the_kb_exists() {
    let h = harness().await;
    let r = h.upload(&h.people.viewer, "kb-a", &png(4, 4, 4), "").await;
    assert_eq!((r.status, r.kind().as_str()), (403, "role_forbidden"));
    let s = h
        .upload(&h.people.stranger, "kb-a", &png(4, 4, 4), "")
        .await;
    let nowhere = h
        .upload(&h.people.stranger, "kb-nowhere", &png(4, 4, 4), "")
        .await;
    assert_eq!((s.status, s.kind().as_str()), (403, "not_member"));
    assert_eq!(s, nowhere);
    assert!(h.rows().is_empty());
    assert_eq!(h.body_bytes(), 0, "a refused caller's body is never read");
}

#[tokio::test]
async fn an_end_to_end_encrypted_kb_refuses_attachments() {
    let h = harness().await;
    let up = h
        .upload(&h.people.e2e_member, "kb-e2e", &png(4, 4, 5), "")
        .await;
    assert_eq!((up.status, up.kind().as_str()), (409, "e2e_unsupported"));
    let get = h
        .fetch("GET", &h.people.e2e_member, "kb-e2e", &sha(b"x"), "")
        .await;
    assert_eq!((get.status, get.kind().as_str()), (409, "e2e_unsupported"));
    assert!(h.rows().is_empty());
}

#[tokio::test]
async fn an_unauthenticated_or_over_budget_request_is_refused_before_the_gate() {
    let h = harness().await;
    let anon = h
        .raw("POST /kb/kb-a/blobs HTTP/1.1\r\nContent-Length: 3", b"abc")
        .await;
    assert_eq!(anon.status, 401);
    let h = harness_with(
        TestQuota {
            storage: None,
            refuse_requests: true,
        },
        40_000_000,
    )
    .await;
    let r = h.upload(&h.people.editor, "kb-a", &png(4, 4, 6), "").await;
    assert_eq!(
        (r.status, r.kind().as_str()),
        (429, "tenant_quota_exceeded")
    );
    assert!(h.rows().is_empty());
}

// ---------------------------------------------------------------------------
// Upload framing and integrity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_upload_without_a_content_length_is_refused_unread() {
    let h = harness().await;
    let head = format!(
        "POST /kb/kb-a/blobs HTTP/1.1\r\nAuthorization: Bearer {}\r\nTransfer-Encoding: chunked",
        h.token(&h.people.editor)
    );
    let r = h.raw(&head, b"5\r\nhello\r\n0\r\n\r\n").await;
    assert_eq!((r.status, r.kind().as_str()), (411, "invalid_params"));
    assert_eq!(h.body_bytes(), 0);
    assert!(h.rows().is_empty());
}

/// D3: a declared size over the cap stops the read before it starts, for a
/// body ten times the cap.
#[tokio::test]
async fn an_upload_declared_over_the_cap_is_refused_before_its_body_is_read() {
    let h = harness().await;
    let big = vec![0x41u8; (CAP * 10) as usize];
    let r = h.upload(&h.people.editor, "kb-a", &big, "").await;
    assert_eq!((r.status, r.kind().as_str()), (413, "quota_exceeded"));
    assert_eq!(r.json()["limit"], CAP);
    assert_eq!(h.body_bytes(), 0, "not one byte of the body was read");
    // At the cap exactly: accepted.
    let at = h
        .upload(&h.people.editor, "kb-a", &vec![0x42u8; CAP as usize], "")
        .await;
    assert_eq!(at.status, 200, "{:?}", at);
}

/// A body that ends before its declared length (the client stopped, or was
/// cut off) leaves no file and no row.
#[tokio::test]
async fn an_upload_cut_short_leaves_no_file_and_no_row() {
    let h = harness().await;
    let head = format!(
        "POST /kb/kb-a/blobs HTTP/1.1\r\nAuthorization: Bearer {}\r\nContent-Length: 2000",
        h.token(&h.people.editor)
    );
    let r = h.raw_then(&head, &[7u8; 500], true).await;
    assert_ne!(r.status, 200);
    // Wait (on an event, not a clock) for the server to finish the connection.
    let mut closed = h.closed.clone();
    closed.wait_for(|n| *n >= 1).await.unwrap();
    assert!(h.rows().is_empty());
    let tmp = h.routes.store.root().join(mae_daemon::blob_store::TMP_DIR);
    assert_eq!(
        std::fs::read_dir(tmp).unwrap().count(),
        0,
        "nothing left in .tmp"
    );
}

/// More bytes than declared: only the declared bytes are the body; the rest
/// is never stored.
#[tokio::test]
async fn bytes_past_the_declared_length_are_never_stored() {
    let h = harness().await;
    let declared = b"%PDF-1.7 the declared part".to_vec();
    let head = format!(
        "POST /kb/kb-a/blobs HTTP/1.1\r\nAuthorization: Bearer {}\r\nContent-Length: {}",
        h.token(&h.people.editor),
        declared.len()
    );
    let sent = [&declared[..], b"<html><script>alert(1)</script>"].concat();
    let r = h.raw(&head, &sent).await;
    assert_eq!(r.status, 200);
    let stored = r.json();
    assert_eq!(stored["sha256"], sha(&declared));
    assert_eq!(stored["size"], declared.len());
    assert_eq!(h.rows().len(), 1);
}

#[tokio::test]
async fn a_content_digest_mismatch_is_refused_and_leaves_no_file_or_row() {
    let h = harness().await;
    let body = png(4, 4, 7);
    let wrong = data_encoding::BASE64.encode(&sha2::Sha256::digest(b"other bytes"));
    let r = h
        .upload(
            &h.people.editor,
            "kb-a",
            &body,
            &format!("\r\nContent-Digest: sha-256=:{wrong}:"),
        )
        .await;
    assert_eq!((r.status, r.kind().as_str()), (400, "blob_hash_mismatch"));
    assert!(h.rows().is_empty());
    let right = data_encoding::BASE64.encode(&sha2::Sha256::digest(&body));
    let ok = h
        .upload(
            &h.people.editor,
            "kb-a",
            &body,
            &format!("\r\nContent-Digest: sha-512=:AAAA:, sha-256=:{right}:"),
        )
        .await;
    assert_eq!(ok.status, 200);
    let bad = h
        .upload(
            &h.people.editor,
            "kb-a",
            &body,
            "\r\nContent-Digest: sha-256=notbase64",
        )
        .await;
    assert_eq!((bad.status, bad.kind().as_str()), (400, "invalid_params"));
}

/// D3: the server picks the address. The answer names the stored bytes, a
/// repeat is the same answer, and the same file in another KB is read and
/// processed in full and stored again.
#[tokio::test]
async fn the_answer_is_the_stored_bytes_address_and_another_kb_stores_its_own_copy() {
    let h = harness().await;
    let sent = png_with_text(9);
    let first = h.upload(&h.people.editor, "kb-a", &sent, "").await;
    assert_eq!(first.status, 200);
    let answer = first.json();
    let address = answer["sha256"].as_str().unwrap().to_string();
    assert_ne!(
        address,
        sha(&sent),
        "the client's bytes are not the address"
    );
    let got = h.fetch("GET", &h.people.viewer, "kb-a", &address, "").await;
    assert_eq!(sha(&got.body), address);
    assert_eq!(answer["size"], got.body.len());
    assert_eq!(answer["media_type"], "image/png");
    assert_eq!(
        (answer["width"].as_u64(), answer["height"].as_u64()),
        (Some(8), Some(8))
    );

    let again = h.upload(&h.people.editor, "kb-a", &sent, "").await;
    assert_eq!(again.body, first.body);
    let before = (
        h.body_bytes(),
        h.routes
            .counters
            .processed
            .load(std::sync::atomic::Ordering::SeqCst),
    );
    let other = h.upload(&h.people.editor, "kb-b", &sent, "").await;
    assert_eq!(other.body, first.body, "an identical answer");
    let after = (
        h.body_bytes(),
        h.routes
            .counters
            .processed
            .load(std::sync::atomic::Ordering::SeqCst),
    );
    assert_eq!(after.0 - before.0, sent.len() as u64, "read in full");
    assert_eq!(after.1 - before.1, 1, "processed, not looked up");
    let rows = h.rows();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|(_, s)| *s == address));
}

#[tokio::test]
async fn concurrent_identical_uploads_end_as_one_blob() {
    let h = Arc::new(harness().await);
    let body = png(16, 16, 11);
    let tasks: Vec<_> = (0..4)
        .map(|_| {
            let (h, body) = (Arc::clone(&h), body.clone());
            tokio::spawn(async move { h.upload(&h.people.editor, "kb-a", &body, "").await })
        })
        .collect();
    let mut answers = Vec::new();
    for t in tasks {
        let r = t.await.unwrap();
        assert_eq!(r.status, 200);
        answers.push(r.body);
    }
    assert!(answers.windows(2).all(|w| w[0] == w[1]));
    assert_eq!(h.rows().len(), 1);
}

// ---------------------------------------------------------------------------
// Storage quota
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_storage_quota_refuses_the_overshooting_upload_and_concurrent_ones_cannot_overshoot() {
    let one = png(8, 8, 1);
    let size = mae_daemon::blob_store::process::process(
        one.clone(),
        &mae_daemon::blob_store::process::ProcessLimits {
            max_pixels: 1 << 30,
            max_frames: 10,
        },
    )
    .unwrap()
    .bytes
    .len() as u64;
    // Room for exactly two blobs of about this size.
    let h = Arc::new(
        harness_with(
            TestQuota {
                storage: Some(size * 2 + size / 2),
                refuse_requests: false,
            },
            40_000_000,
        )
        .await,
    );
    let tasks: Vec<_> = (0..4u8)
        .map(|i| {
            let h = Arc::clone(&h);
            tokio::spawn(async move {
                h.upload(&h.people.editor, "kb-a", &png(8, 8, 1 + i * 2), "")
                    .await
            })
        })
        .collect();
    let mut statuses = Vec::new();
    for t in tasks {
        statuses.push(t.await.unwrap());
    }
    let ok = statuses.iter().filter(|r| r.status == 200).count();
    let full: Vec<_> = statuses.iter().filter(|r| r.status == 507).collect();
    assert_eq!(ok, 2, "{statuses:?}");
    assert_eq!(full.len(), 2);
    assert!(full.iter().all(|r| r.kind() == "tenant_quota_exceeded"));
    // A repeat of a stored blob is not charged again: it still fits.
    let stored = statuses.iter().position(|r| r.status == 200).unwrap();
    let repeat = h
        .upload(
            &h.people.editor,
            "kb-a",
            &png(8, 8, 1 + stored as u8 * 2),
            "",
        )
        .await;
    assert_eq!(repeat.status, 200);
    assert_eq!(h.rows().len(), 2);
}

// ---------------------------------------------------------------------------
// Processing refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn processing_refusals_have_their_kinds() {
    let h = harness_with(
        TestQuota {
            storage: None,
            refuse_requests: false,
        },
        100,
    )
    .await;
    let big = h
        .upload(&h.people.editor, "kb-a", &png(20, 20, 1), "")
        .await;
    assert_eq!(
        (big.status, big.kind().as_str()),
        (413, "attachment_too_many_pixels")
    );
    assert_eq!(big.json()["limit"], 100);
    let p = png(8, 8, 1);
    let broken = h
        .upload(&h.people.editor, "kb-a", &p[..p.len() - 20], "")
        .await;
    assert_eq!(
        (broken.status, broken.kind().as_str()),
        (422, "attachment_undecodable")
    );
    let avif = h
        .upload(
            &h.people.editor,
            "kb-a",
            b"\0\0\0\x1cftypavif\0\0\0\0avifmif1miaf",
            "",
        )
        .await;
    assert_eq!(
        (avif.status, avif.kind().as_str()),
        (415, "attachment_unsupported")
    );
    assert!(h.rows().is_empty());
}

// ---------------------------------------------------------------------------
// Serving
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_blob_is_served_with_d3s_headers_and_only_raster_images_inline() {
    let h = harness().await;
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4"/></svg>"#;
    let cases: [(&[u8], &str, bool); 3] = [
        (&png(4, 4, 1), "image/png", false),
        (svg, "image/svg+xml", true),
        (
            b"<html><script>alert(1)</script></html>",
            "application/octet-stream",
            true,
        ),
    ];
    for (bytes, ctype, attachment) in cases {
        let up = h.upload(&h.people.editor, "kb-a", bytes, "").await.json();
        let address = up["sha256"].as_str().unwrap();
        let r = h.fetch("GET", &h.people.viewer, "kb-a", address, "").await;
        assert_eq!(r.status, 200);
        assert_eq!(r.header("content-type"), Some(ctype), "{ctype}");
        assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
        assert_eq!(
            r.header("content-security-policy"),
            Some("sandbox; default-src 'none'")
        );
        assert_eq!(r.header("etag"), Some(format!("\"{address}\"").as_str()));
        assert_eq!(
            r.header("cache-control"),
            Some("private, max-age=31536000, immutable")
        );
        assert_eq!(
            r.header("content-disposition"),
            attachment.then_some("attachment"),
            "{ctype}"
        );
        let head = h.fetch("HEAD", &h.people.viewer, "kb-a", address, "").await;
        assert_eq!(head.status, 200);
        assert_eq!(
            head.header("content-length"),
            Some(r.body.len().to_string().as_str())
        );
        assert!(head.body.is_empty());
    }
}

#[tokio::test]
async fn a_matching_if_none_match_answers_304_without_reading_the_file() {
    let h = harness().await;
    let up = h
        .upload(&h.people.editor, "kb-a", &png(4, 4, 2), "")
        .await
        .json();
    let address = up["sha256"].as_str().unwrap().to_string();
    // Remove the file behind the row: only a path that never opens it can
    // still answer.
    std::fs::remove_file(h.routes.store.blob_path("kb-a", &address)).unwrap();
    let opened = || {
        h.routes
            .counters
            .files_opened
            .load(std::sync::atomic::Ordering::SeqCst)
    };
    let before = opened();
    let r = h
        .fetch(
            "GET",
            &h.people.viewer,
            "kb-a",
            &address,
            &format!("\r\nIf-None-Match: W/\"x\", \"{address}\""),
        )
        .await;
    assert_eq!(r.status, 304);
    assert_eq!(r.header("etag"), Some(format!("\"{address}\"").as_str()));
    assert!(r.body.is_empty());
    assert_eq!(opened(), before);
    let miss = h
        .fetch(
            "GET",
            &h.people.viewer,
            "kb-a",
            &address,
            "\r\nIf-None-Match: \"other\"",
        )
        .await;
    assert_eq!((miss.status, miss.kind().as_str()), (404, "blob_missing"));
}

#[tokio::test]
async fn other_methods_get_405_and_the_routes_parse_only_their_shape() {
    let h = harness().await;
    let t = h.token(&h.people.editor);
    let put = h
        .raw(
            &format!(
                "PUT /kb/kb-a/blobs/{} HTTP/1.1\r\nAuthorization: Bearer {t}\r\nContent-Length: 0",
                sha(b"x")
            ),
            b"",
        )
        .await;
    assert_eq!((put.status, put.header("allow")), (405, Some("GET, HEAD")));
    let get = h
        .raw(
            &format!("GET /kb/kb-a/blobs HTTP/1.1\r\nAuthorization: Bearer {t}"),
            b"",
        )
        .await;
    assert_eq!((get.status, get.header("allow")), (405, Some("POST")));
    use crate::oauth_blobs::blob_path;
    assert_eq!(blob_path("/kb/kb-a/blobs"), Some(("kb-a", None)));
    assert_eq!(blob_path("/kb/kb-a/blobs/ab"), Some(("kb-a", Some("ab"))));
    assert_eq!(blob_path("/kb/kb-a/blobsx"), None);
    assert_eq!(blob_path("/kb/kb-a/page"), None);
    assert_eq!(blob_path("/kb/kb-a"), None);
}

#[test]
fn the_content_digest_and_if_none_match_parsers() {
    use crate::oauth_blobs::{content_digest, none_match};
    let d = sha2::Sha256::digest(b"x");
    let b64 = data_encoding::BASE64.encode(&d);
    assert_eq!(content_digest(None), Ok(None));
    assert_eq!(
        content_digest(Some(&format!("sha-256=:{b64}:"))),
        Ok(Some(d.into()))
    );
    assert_eq!(content_digest(Some("sha-512=:AAAA:")), Ok(None));
    assert!(
        content_digest(Some("sha-256=:AAAA:")).is_err(),
        "wrong length"
    );
    assert!(content_digest(Some("garbage")).is_err());
    assert!(none_match(Some("*"), "\"a\""));
    assert!(none_match(Some("\"b\", W/\"a\""), "\"a\""));
    assert!(!none_match(Some("\"b\""), "\"a\""));
    assert!(!none_match(None, "\"a\""));
}

/// ADR-109's budgets, as the defaults spend them.
#[test]
fn the_attachment_defaults_fit_adr_109s_memory_budget() {
    let c = crate::config::DaemonConfig::default();
    // D2: 8 GB of RAM for T = 20 tenant processes.
    let per_tenant: u64 = 8_000_000_000 / 20;
    // One worker holds the upload and the decode's allocation bound.
    let worker = c.oauth.max_attachment_bytes + c.oauth.max_attachment_pixels * 8;
    assert!(
        crate::oauth_blobs::IMAGE_WORKERS as u64 * worker <= per_tenant,
        "{worker} bytes per worker"
    );
    assert_eq!(
        c.oauth.max_attachment_bytes,
        c.collab.sync.max_document_size_bytes as u64
    );
    assert!(
        !c.oauth.attachments_enabled,
        "off by default (principle #12)"
    );
}

#[test]
fn default_storage_quota_fits_adr_109s_disk_budget() {
    let q = crate::config::TenantQuotaConfig::default().max_storage_bytes;
    // D2: 100 GB of disk; D1: ~5.5 GB of nodes at T = 20. Each tenant's blobs
    // are held twice: live, and in a backup archive.
    let total = 20 * q * 2 + 5_500_000_000;
    assert!(
        total <= 100_000_000_000 / 2,
        "{total}: leave half for D3 to D5"
    );
    assert!(q > 0, "a configured tenant is bounded by default");
}

/// A PDF arrives as a download, byte for byte; the client's `Content-Type`
/// is never what types a blob, in either direction.
#[tokio::test]
async fn a_pdf_is_stored_as_sent_and_a_claimed_type_is_ignored() {
    let h = harness().await;
    let pdf = one_page_pdf();
    let as_pdf = "\r\nContent-Type: application/pdf";
    let up = h
        .upload(&h.people.editor, "kb-a", &pdf, as_pdf)
        .await
        .json();
    assert_eq!(up["media_type"], "application/pdf");
    assert_eq!(up["sha256"], sha(&pdf));
    assert!(up.get("width").is_none());
    let got = h
        .fetch(
            "GET",
            &h.people.viewer,
            "kb-a",
            up["sha256"].as_str().unwrap(),
            "",
        )
        .await;
    assert_eq!(got.body, pdf, "byte-identical");
    assert_eq!(got.header("content-type"), Some("application/pdf"));
    assert_eq!(got.header("content-disposition"), Some("attachment"));
    // Labelled a PDF, but HTML: never served as a PDF (or as HTML).
    let html = b"<html><script>alert(1)</script></html>";
    let fake = h
        .upload(&h.people.editor, "kb-a", html, as_pdf)
        .await
        .json();
    assert_eq!(fake["media_type"], "application/octet-stream");
    // Labelled a PDF, but a PNG: an image, processed as one.
    let img = h
        .upload(&h.people.editor, "kb-a", &png(4, 4, 3), as_pdf)
        .await
        .json();
    assert_eq!(img["media_type"], "image/png");
    // A PDF labelled an image: still a PDF, still a download.
    let swapped = h
        .upload(
            &h.people.editor,
            "kb-a",
            &pdf,
            "\r\nContent-Type: image/png",
        )
        .await
        .json();
    assert_eq!(swapped["media_type"], "application/pdf");
}

fn one_page_pdf() -> Vec<u8> {
    b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 72 72] >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n".to_vec()
}
