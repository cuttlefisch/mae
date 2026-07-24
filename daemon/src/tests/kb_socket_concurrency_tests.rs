//! Measured (not merely structural) proof that `handler.rs`'s
//! snapshot-then-drop-then-`spawn_blocking` rewrite (ADR-054) actually
//! removes serialization: N concurrent real Unix-socket clients issuing
//! `kb/search` calls across TWO distinct KB stores must complete in close to
//! 1x a fixed artificial delay, not N x it. Before the rewrite, every read
//! arm held `DaemonState`'s single `Arc<Mutex<..>>` across the synchronous
//! query, so this would have measured ~N x the delay instead.

use super::*;
use mae_kb::query::KbQueryLayer;
use mae_kb::store::{HealthReport, Link, SearchHit, SubGraph};
use std::time::Instant;

/// A deliberately slow decorator around a real query layer — delegates every
/// call, but `get`/`search` first do a genuine blocking sleep (faithful,
/// since the real call path runs this inside `spawn_blocking` too). Makes
/// contention measurable without depending on real CozoDB query latency,
/// which is far too fast and noisy to assert a ceiling against directly.
struct SleepyQueryLayer {
    inner: Arc<dyn KbQueryLayer>,
    delay: Duration,
}

impl KbQueryLayer for SleepyQueryLayer {
    fn get(&self, id: &str) -> Option<Node> {
        std::thread::sleep(self.delay);
        self.inner.get(id)
    }
    fn contains(&self, id: &str) -> bool {
        self.inner.contains(id)
    }
    fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        std::thread::sleep(self.delay);
        self.inner.search(query, limit)
    }
    fn links_from(&self, id: &str) -> Vec<Link> {
        self.inner.links_from(id)
    }
    fn links_to(&self, id: &str) -> Vec<Link> {
        self.inner.links_to(id)
    }
    fn list_ids(&self, prefix: Option<&str>) -> Vec<String> {
        self.inner.list_ids(prefix)
    }
    fn id_title_pairs(&self, prefix: Option<&str>) -> Vec<(String, String)> {
        self.inner.id_title_pairs(prefix)
    }
    fn health_report(&self) -> Option<HealthReport> {
        self.inner.health_report()
    }
    fn neighborhood(&self, id: &str, depth: u32) -> Option<SubGraph> {
        self.inner.neighborhood(id, depth)
    }
}

const FIXED_DELAY: Duration = Duration::from_millis(150);

#[tokio::test]
async fn concurrent_reads_across_different_kbs_do_not_serialize() {
    let mut st = seeded_two_store_state();
    let inner = st.query_layer.take().expect("query layer built by seeding");
    st.query_layer = Some(Arc::new(SleepyQueryLayer {
        inner,
        delay: FIXED_DELAY,
    }));
    let state = Arc::new(Mutex::new(st));

    // No connection cap, no idle timeout — isolating the thing under test
    // (lock contention on the query path), not connection admission.
    let socket = spawn_kb_socket(Arc::clone(&state), 0, Duration::ZERO).await;

    // N=8 (>= 3 per principle #14), genuinely distinct queries spanning BOTH
    // the primary and the secondary store — the "different KBs" axis
    // `FederatedQuery` fans out across.
    const N: usize = 8;
    let queries = [
        "alpha", "beta", "gamma", "delta", "epsilon", "rust", "scheme", "mesh",
    ];

    let start = Instant::now();
    let mut handles = Vec::with_capacity(N);
    for query in queries.iter().take(N) {
        let path = socket.path.clone();
        let query = query.to_string();
        handles.push(tokio::spawn(async move {
            let mut stream = UnixStream::connect(&path).await.expect("connect");
            let resp = call(
                &mut stream,
                "kb/search",
                json!({"query": query, "limit": 10}),
            )
            .await;
            assert!(resp.get("error").is_none(), "search failed: {resp:?}");
        }));
    }
    for h in handles {
        h.await.expect("client task panicked");
    }
    let elapsed = start.elapsed();

    // Generous ceiling (< 3x) to avoid CI timing flakiness while still
    // falsifying serialized behavior, which would show ~N x FIXED_DELAY.
    assert!(
        elapsed < FIXED_DELAY * 3,
        "{N} concurrent kb/search calls across 2 KBs took {elapsed:?}; expected close to \
         1x{FIXED_DELAY:?} (ceiling 3x). Approaching {N}x{FIXED_DELAY:?} would mean the calls \
         are still serializing behind DaemonState's lock instead of running concurrently on \
         the blocking pool — the ADR-054 regression this test exists to catch."
    );
}
