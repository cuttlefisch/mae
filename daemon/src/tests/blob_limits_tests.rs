//! ADR-115's upload bounds on the real listener: the idle timeout and the
//! whole-body deadline, the image worker's queue (its wait, each tenant's
//! share, and a processing timeout that cannot commit), and the storage budget
//! of principals in no tenant.
//!
//! The processor is replaced by one the test holds and releases
//! ([`Gate`]), so every interleaving below is forced, never timed. The only
//! clocks are the server's own timeouts, set small here, and an outer bound
//! that turns a server that never answers into a failure instead of a hang.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use mae_daemon::blob_store::process::{ProcessLimits, Processed};
use mae_daemon::blob_store::upload::Processor;
use mae_daemon::blob_store::{MediaType, StorageQuota, TMP_DIR, UNASSIGNED};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::blob_route_tests::{harness_limits, test_limits, Harness, Reply, TestQuota, AGENT};
use crate::oauth_blobs::{
    charge_for, BlobLimits, JOBS_PER_TENANT, MEASURED_WORST_JOB, PROCESS_TIMEOUT, RETRY_AFTER_SECS,
    WORKER_WAIT,
};

/// How long a test waits for the server before calling it hung.
const BOUND: Duration = Duration::from_secs(10);

/// A processor the test holds: an upload whose body starts with `HOLD` (or
/// every upload, with `hold_all`) waits until [`Gate::open`]. Every call is
/// recorded, in order, by its body's first byte.
struct Gate {
    open: Mutex<bool>,
    cv: Condvar,
    order: Mutex<Vec<u8>>,
    entered: tokio::sync::watch::Sender<usize>,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Gate {
            open: Mutex::new(false),
            cv: Condvar::new(),
            order: Mutex::new(Vec::new()),
            entered: tokio::sync::watch::channel(0).0,
        })
    }

    fn processor(self: &Arc<Self>, hold_all: bool) -> Arc<Processor> {
        let gate = Arc::clone(self);
        Arc::new(move |bytes: Vec<u8>, _: &ProcessLimits| {
            gate.order.lock().unwrap().push(bytes[0]);
            gate.entered.send_modify(|n| *n += 1);
            if hold_all || bytes.starts_with(b"HOLD") {
                // Bounded, so a failed test cannot leave a worker parked
                // forever and hang the runtime's shutdown.
                let open = gate.open.lock().unwrap();
                let _ = gate.cv.wait_timeout_while(open, BOUND, |o| !*o).unwrap();
            }
            Ok(Processed {
                bytes,
                media_type: MediaType::Other,
                width: None,
                height: None,
            })
        })
    }

    fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.cv.notify_all();
    }

    async fn entered(&self, n: usize) {
        let mut rx = self.entered.subscribe();
        tokio::time::timeout(BOUND, rx.wait_for(|e| *e >= n))
            .await
            .expect("the processor was reached")
            .unwrap();
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        self.open();
    }
}

async fn decisions(h: &Harness, n: u64) {
    let mut rx = h.routes.counters.queue_decisions.subscribe();
    tokio::time::timeout(BOUND, rx.wait_for(|d| *d >= n))
        .await
        .expect("the uploads reached the queue")
        .unwrap();
}

fn tmp_files(h: &Harness) -> usize {
    std::fs::read_dir(h.routes.store.root().join(TMP_DIR))
        .unwrap()
        .count()
}

fn sha(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

/// Read one response (head, then `Content-Length` bytes of body), without
/// waiting for the server to close: an unread request body may keep it open.
async fn read_reply(s: &mut (impl AsyncReadExt + Unpin)) -> Reply {
    let mut buf = Vec::new();
    let read = async {
        loop {
            let mut chunk = [0u8; 4096];
            let n = s.read(&mut chunk).await.unwrap_or(0);
            buf.extend_from_slice(&chunk[..n]);
            if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
                let len: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .map_or(0, |v| v.trim().parse().unwrap());
                if buf.len() >= end + 4 + len {
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
    };
    tokio::time::timeout(BOUND, read)
        .await
        .expect("the server answered before the bound");
    Reply::parse(&buf)
}

/// Open an upload declaring `declared` bytes and send `first` of them.
async fn start_upload(h: &Harness, declared: usize, first: &[u8]) -> tokio::net::TcpStream {
    let head = h.upload_head(&h.people.editor.fingerprint(), "kb-a", declared, "");
    let mut s = tokio::net::TcpStream::connect(h.addr).await.unwrap();
    let req = format!("{head}\r\nHost: test\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    s.write_all(first).await.unwrap();
    s
}

fn assert_nothing_kept(h: &Harness) {
    assert_eq!(tmp_files(h), 0, "the temporary file is deleted");
    assert!(h.rows().is_empty(), "nothing stored");
    assert_eq!(
        h.routes.store.usage(UNASSIGNED).unwrap(),
        0,
        "nothing charged"
    );
}

// ---------------------------------------------------------------------------
// Receiving
// ---------------------------------------------------------------------------

/// A body that stops arriving, with the connection left open, is refused at
/// the idle timeout: nothing but the server's clock could end this request.
#[tokio::test]
async fn an_upload_that_stalls_is_refused_at_the_idle_timeout_and_leaves_nothing() {
    let limits = BlobLimits {
        idle_timeout: Duration::from_millis(200),
        deadline: Duration::from_secs(60),
        ..test_limits()
    };
    let h = harness_limits(TestQuota::none(), limits, None).await;
    let mut s = start_upload(&h, 2000, &[7u8; 500]).await;
    let r = read_reply(&mut s).await;
    assert_eq!((r.status, r.kind().as_str()), (408, "upload_timeout"));
    assert_nothing_kept(&h);
}

/// A body that keeps trickling, never idle for long, is refused at the
/// whole-upload deadline.
#[tokio::test]
async fn an_upload_that_trickles_past_the_deadline_is_refused_and_leaves_nothing() {
    let limits = BlobLimits {
        idle_timeout: Duration::from_secs(5),
        deadline: Duration::from_millis(300),
        ..test_limits()
    };
    let h = harness_limits(TestQuota::none(), limits, None).await;
    let s = start_upload(&h, 2000, b"x").await;
    let (mut rd, mut wr) = s.into_split();
    let trickle = tokio::spawn(async move {
        for _ in 0..1999 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if wr.write_all(b"x").await.is_err() {
                return;
            }
        }
    });
    let r = read_reply(&mut rd).await;
    trickle.abort();
    assert_eq!((r.status, r.kind().as_str()), (408, "upload_timeout"));
    assert_nothing_kept(&h);
}

// ---------------------------------------------------------------------------
// Processing
// ---------------------------------------------------------------------------

/// A job past the processing timeout is refused; its worker stays held until
/// it really returns (so a second job cannot run beside it), and when it does
/// it stores and charges nothing; the next job then proceeds.
#[tokio::test]
async fn a_job_that_times_out_stores_and_charges_nothing_and_the_next_proceeds() {
    let gate = Gate::new();
    let limits = BlobLimits {
        process_timeout: Duration::from_millis(100),
        worker_wait: Duration::from_millis(300),
        ..test_limits()
    };
    let h = harness_limits(TestQuota::none(), limits, Some(gate.processor(false))).await;
    let slow = h
        .upload(&h.people.editor, "kb-a", b"HOLD slow upload", "")
        .await;
    assert_eq!(
        (slow.status, slow.kind().as_str()),
        (422, "attachment_undecodable")
    );
    // The cancelled job is still inside the processor, holding the worker.
    let beside = h.upload(&h.people.editor, "kb-a", b"beside", "").await;
    assert_eq!(
        (beside.status, beside.kind().as_str()),
        (503, "attachment_busy"),
        "the worker is held until the timed-out job returns"
    );
    gate.open();
    // One worker: this waits for the slow job's worker to return.
    let next = h.upload(&h.people.editor, "kb-a", b"next", "").await;
    assert_eq!(next.status, 200, "{next:?}");
    assert_eq!(
        h.rows(),
        vec![("kb-a".to_string(), sha(b"next"))],
        "the timed-out job stored nothing"
    );
    assert_eq!(h.routes.store.usage(UNASSIGNED).unwrap(), 4, "nor charged");
    assert_eq!(tmp_files(&h), 0);
}

/// One tenant uploading many at once holds at most its share of the queue:
/// the rest are refused at once, and another tenant's upload is processed
/// after at most that share.
#[tokio::test]
async fn one_tenants_flood_does_not_starve_another_tenants_upload() {
    let gate = Gate::new();
    let h = Arc::new(Harness::with_tenants(&gate).await);
    let flood = JOBS_PER_TENANT + 4;
    let a = h.people.editor.fingerprint();
    let mut a_jobs = Vec::new();
    for i in 0..flood {
        let (h, a) = (Arc::clone(&h), a.clone());
        a_jobs.push(tokio::spawn(async move {
            h.upload_as(&a, "kb-a", &[b'A', i as u8], "").await
        }));
        if i == 0 {
            gate.entered(1).await; // the first holds the worker
        }
    }
    decisions(&h, flood as u64).await;
    let b = {
        let (h, b) = (Arc::clone(&h), h.people.other_editor.fingerprint());
        tokio::spawn(async move { h.upload_as(&b, "kb-b", b"B", "").await })
    };
    decisions(&h, flood as u64 + 1).await;
    gate.open();

    assert_eq!(b.await.unwrap().status, 200);
    let mut refused = 0;
    for job in a_jobs {
        let r = job.await.unwrap();
        if r.status == 503 {
            refused += 1;
            assert_eq!(r.kind(), "attachment_busy");
            assert_eq!(
                r.header("retry-after"),
                Some(RETRY_AFTER_SECS.to_string().as_str())
            );
        } else {
            assert_eq!(r.status, 200, "{r:?}");
        }
    }
    assert_eq!(refused, flood - JOBS_PER_TENANT, "refused beyond the share");
    let order = gate.order.lock().unwrap().clone();
    let before_b = order.iter().take_while(|t| **t != b'B').count();
    assert!(
        before_b <= JOBS_PER_TENANT,
        "{before_b} of the flood were processed before the other tenant: {order:?}"
    );
}

/// A job that cannot get a worker within the wait is refused with
/// `Retry-After`, and nothing of it is stored or charged.
#[tokio::test]
async fn a_job_that_waits_past_the_worker_wait_is_refused_and_charged_nothing() {
    let gate = Gate::new();
    let limits = BlobLimits {
        worker_wait: Duration::from_millis(150),
        ..test_limits()
    };
    let h = Arc::new(harness_limits(TestQuota::none(), limits, Some(gate.processor(false))).await);
    let held = {
        let h = Arc::clone(&h);
        tokio::spawn(async move { h.upload(&h.people.editor, "kb-a", b"HOLD", "").await })
    };
    gate.entered(1).await;
    let late = h.upload(&h.people.other_editor, "kb-b", b"late", "").await;
    assert_eq!(
        (late.status, late.kind().as_str()),
        (503, "attachment_busy")
    );
    assert_eq!(late.json()["retry_after_secs"], RETRY_AFTER_SECS);
    assert_eq!(
        late.header("retry-after"),
        Some(RETRY_AFTER_SECS.to_string().as_str())
    );
    assert!(h.rows().is_empty());
    gate.open();
    assert_eq!(held.await.unwrap().status, 200);
    assert_eq!(h.rows(), vec![("kb-a".to_string(), sha(b"HOLD"))]);
    assert_eq!(tmp_files(&h), 0, "the refused upload's file is gone too");
}

impl Harness {
    /// The editor in tenant `a`, the other editor in tenant `b`; every upload
    /// held at the gate.
    async fn with_tenants(gate: &Arc<Gate>) -> Harness {
        let quota = TestQuota::none();
        let tenants = Arc::clone(&quota.tenants);
        let h = harness_limits(quota, test_limits(), Some(gate.processor(true))).await;
        let mut t = tenants.lock().unwrap();
        t.insert(h.people.editor.fingerprint(), ("a".to_string(), 0));
        t.insert(h.people.other_editor.fingerprint(), ("b".to_string(), 0));
        drop(t);
        h
    }
}

// ---------------------------------------------------------------------------
// The storage budget of principals in no tenant
// ---------------------------------------------------------------------------

/// With no tenant configured, uploads are still bounded: every principal,
/// an agent included, is charged to one shared default budget.
#[tokio::test]
async fn principals_in_no_tenant_share_the_default_budget_and_an_agent_is_counted() {
    let limits = BlobLimits {
        default_storage_bytes: 10,
        ..test_limits()
    };
    let h = harness_limits(TestQuota::none(), limits, None).await;
    let agent = h.upload_as(AGENT, "kb-a", b"agent!", "").await;
    assert_eq!(agent.status, 200, "{agent:?}");
    assert_eq!(
        h.routes.store.usage(UNASSIGNED).unwrap(),
        6,
        "the agent is charged"
    );
    let editor = h.upload(&h.people.editor, "kb-a", b"editor", "").await;
    assert_eq!(
        (editor.status, editor.kind().as_str()),
        (507, "tenant_quota_exceeded")
    );
    let fits = h.upload(&h.people.editor, "kb-a", b"four", "").await;
    assert_eq!(fits.status, 200, "up to the budget exactly");
    assert_eq!(h.routes.store.usage(UNASSIGNED).unwrap(), 10);
}

/// The charge resolution itself: a configured tenant's principals go to it,
/// everyone else (an ADR-118 agent, an unlisted key) to the default bucket,
/// which no tenant name can share, and `0` stays an explicit opt-out.
#[test]
fn every_principal_resolves_to_a_budget() {
    let tenant = crate::config::TenantConfig {
        name: UNASSIGNED.to_string(),
        instances: Vec::new(),
        principals: vec!["SHA256:member".to_string()],
        quota: Default::default(),
    };
    let registry = Arc::new(crate::tenant::TenantRegistry::from_config(&[tenant]));
    let q = crate::tenant::TenantQuota(registry);
    let member = charge_for(&q, "SHA256:member", 7);
    assert_ne!(
        member.quota.tenant, UNASSIGNED,
        "a tenant named like the bucket"
    );
    assert_eq!(
        member.quota.max_bytes,
        crate::config::DEFAULT_MAX_STORAGE_BYTES
    );
    for outsider in [AGENT, "SHA256:other"] {
        let c = charge_for(&q, outsider, 7);
        assert_eq!(c.quota, StorageQuota::unassigned(7), "{outsider}");
        assert_eq!(c.queue_key, format!("principal:{outsider}"));
    }
    assert_eq!(charge_for(&q, AGENT, 0).quota.max_bytes, 0);
}

// ---------------------------------------------------------------------------
// Why the defaults are what they are
// ---------------------------------------------------------------------------

#[test]
fn the_queue_bounds_follow_from_the_measured_worst_job() {
    let share = MEASURED_WORST_JOB * JOBS_PER_TENANT as u32;
    assert!(
        share <= WORKER_WAIT / 2,
        "another tenant waits behind one tenant's whole share well inside the wait"
    );
    // One processing and one more in line.
    const { assert!(JOBS_PER_TENANT >= 2) };
    assert_eq!(
        RETRY_AFTER_SECS, 10,
        "retry once a full share could have drained"
    );
    assert!(Duration::from_secs(RETRY_AFTER_SECS) >= share);
    assert!(
        WORKER_WAIT >= PROCESS_TIMEOUT,
        "a wait outlasts one slow job"
    );
    assert!(
        PROCESS_TIMEOUT >= MEASURED_WORST_JOB * 10,
        "headroom over the worst"
    );
}

#[test]
fn the_upload_timeouts_admit_a_slow_link_and_scale_with_the_cap() {
    use crate::oauth_config::{OAuthConfig, ATTACHMENT_FLOOR_BYTES_PER_SEC};
    let c = OAuthConfig::default();
    let d = c.attachment_upload_deadline();
    assert!(
        d * ATTACHMENT_FLOOR_BYTES_PER_SEC >= c.max_attachment_bytes,
        "a body at the cap arrives at the floor throughput"
    );
    assert_eq!(d, 160);
    // TCP's retransmission backoff (1 + 2 + 4 + 8 s) fits inside the idle
    // timeout, and the idle timeout inside the deadline.
    assert!(c.attachment_idle_timeout_secs >= 15);
    assert!(c.attachment_idle_timeout_secs < d);
    assert_eq!(
        c.attachment_idle_timeout_secs,
        PROCESS_TIMEOUT.as_secs(),
        "no longer on the network than on the CPU"
    );
    let bigger = OAuthConfig {
        max_attachment_bytes: c.max_attachment_bytes * 4,
        ..OAuthConfig::default()
    };
    assert_eq!(
        bigger.attachment_upload_deadline(),
        d * 4,
        "derived from the cap"
    );
    let set = OAuthConfig {
        attachment_upload_deadline_secs: Some(7),
        ..OAuthConfig::default()
    };
    assert_eq!(set.attachment_upload_deadline(), 7);
}

#[test]
fn one_worker_fits_the_tenant_memory_share_and_two_would_not() {
    let c = crate::config::DaemonConfig::default();
    // ADR-109 D2: 8 GB of RAM for T = 20 tenant processes.
    let share: u64 = 8_000_000_000 / 20;
    // blob_bench: a rotated 40-megapixel JPEG peaks at +229 MiB.
    let measured: u64 = 229 << 20;
    let workers = c.oauth.attachment_image_workers as u64;
    assert_eq!(workers, 1);
    assert!(workers * measured <= share);
    assert!((workers + 1) * measured > share, "so it is not two");
}

#[test]
fn zero_limits_are_configuration_errors_and_the_default_budget_matches_a_tenants() {
    use crate::oauth_config::OAuthConfig;
    for c in [
        OAuthConfig {
            attachment_idle_timeout_secs: 0,
            ..OAuthConfig::default()
        },
        OAuthConfig {
            attachment_upload_deadline_secs: Some(0),
            ..OAuthConfig::default()
        },
        OAuthConfig {
            attachment_image_workers: 0,
            ..OAuthConfig::default()
        },
    ] {
        assert!(BlobLimits::from_config(&c).is_err());
    }
    let c = OAuthConfig::default();
    assert_eq!(
        c.default_storage_bytes,
        crate::config::TenantQuotaConfig::default().max_storage_bytes,
        "the unassigned bucket is one tenant's share (ADR-108: a process is one tenant)"
    );
    assert!(c.default_storage_bytes > 0, "bounded by default");
}
