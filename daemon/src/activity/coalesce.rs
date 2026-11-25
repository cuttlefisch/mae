//! Coalesced refusals (ADR-120 D6): never dropped, and bounded.
//!
//! Repeated refusals of the same `(principal or address, route, refusal
//! kind)` within `refusal_window_secs`: the first is an event committed before
//! its answer, like any request; the rest are counted here and flushed at the
//! window's end as one event with `count`, `first_at` and `last_at`. So what
//! a flood of bad tokens can make the daemon write is bounded, and the
//! fail-closed queue cannot be turned into a denial of service by them.
//!
//! A flood spread across many sources is bounded too: past half the queue
//! (refusals before authentication) or past `queue_capacity` open windows,
//! refusals are counted into one aggregate per refusal kind, without
//! addresses. The accepted limits: a crash loses the current window's
//! count, never the fact that refusals began; under a spread flood the
//! addresses past that point are not recorded, only their number.

use std::collections::HashMap;
use std::sync::Mutex;

use super::event::{Event, Principal};
use super::store::Entry;
use super::ActivitySettings;

/// What makes two refusals "the same" (D6).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    /// The principal's id, or the address for a refusal before one exists.
    pub who: String,
    pub route: String,
    pub kind: String,
}

impl Key {
    /// The key of a refused `ev`: its principal, else its address.
    pub fn of(ev: &Event) -> Key {
        let who = match (&ev.principal.id, &ev.peer) {
            (Some(id), _) => format!("principal:{id}"),
            (None, Some(p)) => format!("addr:{}", p.addr),
            (None, None) => "unknown".to_string(),
        };
        Key {
            who,
            route: ev.route.clone().unwrap_or_default(),
            kind: ev.outcome_kind.clone().unwrap_or_default(),
        }
    }
}

struct Window {
    opened_us: i64,
    /// The first refusal, committed on its own: the flush's template.
    template: Event,
    /// Refusals after the first, and when they fell.
    count: u64,
    first_us: i64,
    last_us: i64,
}

impl Window {
    fn open(ev: &Event, now_us: i64) -> Self {
        Window {
            opened_us: now_us,
            template: ev.clone(),
            count: 0,
            first_us: 0,
            last_us: 0,
        }
    }

    fn add(&mut self, at_us: i64) {
        if self.count == 0 {
            self.first_us = at_us;
        }
        self.count += 1;
        self.last_us = self.last_us.max(at_us);
    }

    /// The window's flush, if anything after the first was counted.
    fn flush(self, now_us: i64) -> Option<Entry> {
        (self.count > 0).then(|| {
            let mut ev = self.template;
            ev.at_us = now_us;
            ev.count = self.count;
            ev.first_at_us = self.first_us;
            ev.last_at_us = self.last_us;
            Entry::of(ev)
        })
    }
}

/// What to do with one refusal.
#[derive(Debug)]
pub struct Admit {
    /// Commit this refusal's own event before answering (the first of its
    /// window); `false`: it was counted.
    pub commit: bool,
    /// Windows this refusal closed, to queue now.
    pub flushed: Vec<Entry>,
}

pub struct Coalescer {
    window_us: i64,
    max_windows: usize,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    windows: HashMap<Key, Window>,
    /// One aggregate per refusal kind, without addresses.
    aggregates: HashMap<String, Window>,
}

impl Coalescer {
    pub fn new(settings: &ActivitySettings) -> Self {
        Coalescer {
            window_us: settings.refusal_window_secs as i64 * 1_000_000,
            max_windows: settings.queue_capacity.max(1),
            inner: Mutex::new(Inner::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Admit the refused `ev` at `now_us`. With `aggregate` (D6's half of the
    /// queue is held by refusals before authentication), or once every
    /// window slot is taken, it is counted into its kind's aggregate instead.
    pub fn admit(&self, ev: &Event, now_us: i64, aggregate: bool) -> Admit {
        let key = Key::of(ev);
        let mut inner = self.lock();
        let mut flushed = Vec::new();
        if let Some(w) = inner.windows.get(&key) {
            if now_us < w.opened_us + self.window_us {
                inner
                    .windows
                    .get_mut(&key)
                    .expect("just found")
                    .add(ev.at_us);
                return Admit {
                    commit: false,
                    flushed,
                };
            }
            let old = inner.windows.remove(&key).expect("just found");
            flushed.extend(old.flush(now_us));
        }
        if aggregate || inner.windows.len() >= self.max_windows {
            self.count_aggregate(&mut inner, ev, now_us, &mut flushed);
            return Admit {
                commit: false,
                flushed,
            };
        }
        inner.windows.insert(key, Window::open(ev, now_us));
        Admit {
            commit: true,
            flushed,
        }
    }

    fn count_aggregate(
        &self,
        inner: &mut Inner,
        ev: &Event,
        now_us: i64,
        flushed: &mut Vec<Entry>,
    ) {
        let kind = ev.outcome_kind.clone().unwrap_or_default();
        let expired = inner
            .aggregates
            .get(&kind)
            .is_some_and(|w| now_us >= w.opened_us + self.window_us);
        if expired {
            let old = inner.aggregates.remove(&kind).expect("just found");
            flushed.extend(old.flush(now_us));
        }
        inner
            .aggregates
            .entry(kind)
            .or_insert_with(|| {
                let mut template = ev.clone();
                template.principal = Principal::anonymous();
                template.peer = None;
                template.unverified = None;
                template.session = None;
                template.route = None;
                template.kb_id = None;
                template.node_ids = None;
                Window::open(&template, now_us)
            })
            .add(ev.at_us);
    }

    /// The flushes of every window that has ended by `now_us`.
    pub fn drain_expired(&self, now_us: i64) -> Vec<Entry> {
        let mut guard = self.lock();
        let Inner {
            windows,
            aggregates,
        } = &mut *guard;
        let mut out = windows.take_expired(now_us, self.window_us);
        out.extend(aggregates.take_expired(now_us, self.window_us));
        out
    }

    /// The flushes of every window, ended or not (clean shutdown).
    pub fn drain_all(&self, now_us: i64) -> Vec<Entry> {
        let mut inner = self.lock();
        let mut out: Vec<Entry> = inner
            .windows
            .drain()
            .filter_map(|(_, w)| w.flush(now_us))
            .collect();
        out.extend(
            inner
                .aggregates
                .drain()
                .filter_map(|(_, w)| w.flush(now_us)),
        );
        out
    }

    /// Open windows, for tests and the bound.
    pub fn open_windows(&self) -> usize {
        self.lock().windows.len()
    }
}

/// The two maps' shared expiry, over their different keys.
trait WindowMap {
    fn take_expired(&mut self, now_us: i64, window_us: i64) -> Vec<Entry>;
}

impl<K: Clone + Eq + std::hash::Hash> WindowMap for HashMap<K, Window> {
    fn take_expired(&mut self, now_us: i64, window_us: i64) -> Vec<Entry> {
        let ended: Vec<K> = self
            .iter()
            .filter(|(_, w)| now_us >= w.opened_us + window_us)
            .map(|(k, _)| k.clone())
            .collect();
        ended
            .into_iter()
            .filter_map(|k| self.remove(&k).and_then(|w| w.flush(now_us)))
            .collect()
    }
}
