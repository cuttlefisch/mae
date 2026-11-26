//! The cache alone: when a held site serves, single flight, failures, bounds.

use super::*;

fn key(generation: u64) -> DerivationKey {
    DerivationKey {
        generation,
        blob_changes: 0,
        hidden_namespaces: vec![],
        highlighted: true,
        max_scan_nodes: 500,
        max_body_bytes: 1 << 20,
    }
}

fn site() -> Derived {
    Derived::TooLarge
}

/// A held site serves a request keyed at or below what it was derived from,
/// and never one with other options or bounds, or newer content.
#[test]
fn a_held_site_serves_only_what_it_was_derived_from() {
    let held = key(5);
    assert!(held.serves(&key(5)));
    assert!(
        held.serves(&key(4)),
        "older asked, newer held: newer is fine"
    );
    assert!(!held.serves(&key(6)), "newer content asked");
    let others = [
        DerivationKey {
            blob_changes: 1,
            ..key(5)
        },
        DerivationKey {
            hidden_namespaces: vec!["private".into()],
            ..key(5)
        },
        DerivationKey {
            highlighted: false,
            ..key(5)
        },
        DerivationKey {
            max_scan_nodes: 499,
            ..key(5)
        },
        DerivationKey {
            max_body_bytes: 1,
            ..key(5)
        },
    ];
    for other in others {
        assert!(!held.serves(&other), "{other:?}");
    }
}

/// Same key: one derivation. A newer generation: derived again. Another KB:
/// its own slot, never the first KB's site.
#[tokio::test]
async fn a_kb_is_derived_once_per_change() {
    let cache = SiteCache::default();
    for _ in 0..3 {
        cache.get_or_derive("a", key(1), || async { site() }).await;
    }
    assert_eq!(cache.derivations(), 1);
    cache.get_or_derive("a", key(2), || async { site() }).await;
    assert_eq!(cache.derivations(), 2);
    cache.get_or_derive("b", key(2), || async { site() }).await;
    assert_eq!(
        cache.derivations(),
        3,
        "another KB is never served the first's"
    );
}

/// Concurrent misses on one KB derive once.
#[tokio::test]
async fn concurrent_misses_derive_once() {
    let cache = Arc::new(SiteCache::default());
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let cache = Arc::clone(&cache);
        tasks.push(tokio::spawn(async move {
            cache
                .get_or_derive("a", key(1), || async {
                    tokio::task::yield_now().await;
                    site()
                })
                .await;
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    assert_eq!(cache.derivations(), 1);
}

/// A failed derivation is not held: the next request derives again.
#[tokio::test]
async fn a_failure_is_not_held() {
    let cache = SiteCache::default();
    cache
        .get_or_derive("a", key(1), || async { Derived::Failed })
        .await;
    cache.get_or_derive("a", key(1), || async { site() }).await;
    assert_eq!(cache.derivations(), 2);
}

/// At most `capacity` KBs are held, dropping the least recently used;
/// capacity 0 holds nothing.
#[tokio::test]
async fn the_cache_is_bounded() {
    let cache = SiteCache::default();
    cache.set_capacity(2);
    for kb in ["a", "b", "a", "c"] {
        cache.get_or_derive(kb, key(1), || async { site() }).await;
    }
    assert_eq!(cache.held_kbs(), 2);
    let before = cache.derivations();
    cache.get_or_derive("a", key(1), || async { site() }).await;
    assert_eq!(cache.derivations(), before, "a was used last, so b went");
    cache.set_capacity(0);
    cache.get_or_derive("a", key(1), || async { site() }).await;
    cache.get_or_derive("a", key(1), || async { site() }).await;
    assert_eq!(cache.derivations(), before + 2);
}
