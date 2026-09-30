use super::*;
use crate::query::InMemoryQueryLayer;
use crate::{KnowledgeBase, NodeKind};

fn corpus() -> KnowledgeBase {
    let mut kb = KnowledgeBase::new();
    for (id, title, body) in [
        ("p:alpha", "Alpha", "links to [[p:beta]] and [[q:gamma]]"),
        ("p:beta", "Beta", "back to [[p:alpha]]"),
        ("q:gamma", "Gamma", "an alpha mention without a link"),
        ("q:delta", "Delta", ""),
    ] {
        kb.insert(Node::new(id, title, NodeKind::Note, body));
    }
    kb
}

/// Forwarding must be exact: the wrapper is a transport, so any difference from
/// the inner layer's own answer is a bug in the wrapper. Compared through `Debug`
/// because not every result type implements `PartialEq`.
#[test]
fn every_call_returns_what_the_inner_layer_returns() {
    let inner = InMemoryQueryLayer::new(corpus());
    let wrapped = OffThreadQueryLayer::spawn("fidelity", || InMemoryQueryLayer::new(corpus()))
        .expect("spawn");
    let same = |a: String, b: String, what: &str| assert_eq!(a, b, "{what} differs");

    for id in ["p:alpha", "q:delta", "missing"] {
        same(
            format!("{:?}", inner.get(id).map(|n| n.body)),
            format!("{:?}", wrapped.get(id).map(|n| n.body)),
            "get",
        );
        same(
            format!("{:?}", inner.contains(id)),
            format!("{:?}", wrapped.contains(id)),
            "contains",
        );
        same(
            format!("{:?}", inner.links_from(id)),
            format!("{:?}", wrapped.links_from(id)),
            "links_from",
        );
        same(
            format!("{:?}", inner.links_to(id)),
            format!("{:?}", wrapped.links_to(id)),
            "links_to",
        );
    }
    for q in ["alpha", "beta", "nothing-matches"] {
        same(
            format!("{:?}", inner.search(q, 10)),
            format!("{:?}", wrapped.search(q, 10)),
            "search",
        );
    }
    for prefix in [None, Some("p:"), Some("zz:")] {
        same(
            format!("{:?}", inner.list_ids(prefix)),
            format!("{:?}", wrapped.list_ids(prefix)),
            "list_ids",
        );
        same(
            format!("{:?}", inner.id_title_pairs(prefix)),
            format!("{:?}", wrapped.id_title_pairs(prefix)),
            "id_title_pairs",
        );
    }
    same(
        format!("{:?}", inner.degraded()),
        format!("{:?}", wrapped.degraded()),
        "degraded",
    );
}

/// A layer whose every call panics — stands in for a blocking layer that dies.
struct Panics;
impl KbQueryLayer for Panics {
    fn get(&self, _: &str) -> Option<Node> {
        panic!("inner layer failed")
    }
    fn contains(&self, _: &str) -> bool {
        panic!("inner layer failed")
    }
    fn search(&self, _: &str, _: usize) -> Result<Vec<SearchHit>, KbStoreError> {
        panic!("inner layer failed")
    }
    fn links_from(&self, _: &str) -> Result<Vec<Link>, KbStoreError> {
        panic!("inner layer failed")
    }
    fn links_to(&self, _: &str) -> Result<Vec<Link>, KbStoreError> {
        panic!("inner layer failed")
    }
    fn list_ids(&self, _: Option<&str>) -> Result<Vec<String>, KbStoreError> {
        panic!("inner layer failed")
    }
    fn id_title_pairs(&self, _: Option<&str>) -> Result<Vec<(String, String)>, KbStoreError> {
        panic!("inner layer failed")
    }
    fn health_report(&self) -> Result<Option<HealthReport>, KbStoreError> {
        panic!("inner layer failed")
    }
    fn neighborhood(&self, _: &str, _: u32) -> Result<Option<SubGraph>, KbStoreError> {
        panic!("inner layer failed")
    }
}

/// A dead worker must fail every later call and report degraded — never hang,
/// never pretend to have answered.
#[test]
fn a_dead_worker_fails_closed_and_reports_degraded() {
    let layer = OffThreadQueryLayer::spawn("dies", || Panics).expect("spawn");
    assert!(
        layer.search("x", 5).is_err(),
        "the call that kills it errors"
    );
    assert!(layer.search("x", 5).is_err(), "later calls error too");
    assert!(layer.get("p:alpha").is_none());
    assert!(!layer.contains("p:alpha"));
    assert!(layer.degraded(), "a dead worker is degraded");
    assert!(
        layer.capabilities().gaps().len() == crate::capabilities::QueryMethod::ALL.len(),
        "and claims no capability"
    );
}

/// The reason this module exists. `RemoteHubQueryLayer` holds a
/// `reqwest::blocking::Client`; the editor runs inside `Runtime::block_on`.
#[cfg(feature = "remote-hub")]
mod blocking_layer_in_a_runtime {
    use super::*;
    use crate::federation::{RemoteHubAuth, RemoteHubConfig};
    use crate::remote_hub::RemoteHubQueryLayer;
    use std::time::Duration;

    /// A hub URL on a port nothing listens on. The auth command fails on hosts
    /// without `sh`, which is also a degraded outcome — either way no real
    /// credential, keystore or network peer is touched.
    fn unreachable_hub() -> RemoteHubConfig {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .expect("bind")
            .port();
        RemoteHubConfig {
            base_url: format!("http://127.0.0.1:{port}/"),
            hub_kb_id: "test-kb".into(),
            auth: RemoteHubAuth::Command("echo test-token".into()),
        }
    }

    /// The editor's shape: a current-thread runtime, work done inside `block_on`.
    fn inside_runtime(f: impl FnOnce()) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(async { f() })
    }

    /// Control, so the next test is not vacuous: in this exact harness the BARE
    /// layer does panic. If this ever stops failing, re-check whether the
    /// wrapper is still needed rather than deleting the assertion.
    #[test]
    fn a_bare_blocking_layer_panics_when_dropped_in_a_runtime() {
        let outcome = std::panic::catch_unwind(|| {
            inside_runtime(|| drop(RemoteHubQueryLayer::new(unreachable_hub())));
        });
        assert!(
            outcome.is_err(),
            "expected the reqwest-inside-runtime panic this module guards against"
        );
    }

    #[test]
    fn a_wrapped_blocking_layer_is_used_and_dropped_in_a_runtime_without_panicking() {
        inside_runtime(|| {
            let config = unreachable_hub();
            let layer = OffThreadQueryLayer::spawn("hub", move || {
                RemoteHubQueryLayer::with_timeout(config, Duration::from_millis(500))
            })
            .expect("spawn");
            let hits = layer.search("anything", 5).unwrap_or_default();
            assert!(hits.is_empty(), "an unreachable hub returns nothing");
            assert!(layer.degraded(), "…and says so");
            drop(layer); // inside block_on: the panic site for the bare layer
        });
    }
}
