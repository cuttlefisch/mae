//! ADR-060 Phase C: per-tenant quota accounting and independent eviction.
//!
//! **Two quota keys, not one** (see the ADR's Decision section for the full
//! rationale): the KB Unix socket has zero principal concept by deliberate,
//! documented design (`daemon/src/config.rs`'s `KbSocketConfig` doc comment),
//! so it keys on Phase A's own instance address (`instance_addr`,
//! `daemon/src/handler.rs`); the collab/OAuth listeners key on the real
//! authenticated principal instead. This module resolves either key to a
//! tenant name via `daemon.toml`'s `[[tenant]]` tables, then meters a single
//! cost-weighted points budget per fixed 60s window (GitHub's own production
//! secondary-rate-limit shape — see the ADR's Context) plus a separate,
//! tenant-scoped concurrent-request cap (`conn_limit::ConnLimiter`, reused
//! rather than reinvented — CLAUDE.md principle #8).
//!
//! **Scope of this implementation pass** (principle #15 — a bounded down
//! payment, not silently partial): wired into the KB Unix socket
//! (`handler.rs::dispatch`'s `snapshot_query_layer`/`snapshot_store`
//! chokepoint) with the exact cost table the ADR names for that surface.
//! Principal-keyed enforcement on the collab/OAuth listeners
//! (`check_and_charge_by_principal` below) is implemented and tested here as
//! a listener-agnostic capability, but not yet plugged into
//! `collab_handler::handle_doc_request_inner` — that wiring, plus a cost
//! table for `sync/*`/`kb/share`/`kb/node_update`-shaped methods (a
//! different RPC surface than the KB socket's `kb/get`-shaped one, needing
//! its own reasoned mapping rather than reusing this one blindly), is
//! tracked as explicit follow-on work rather than left as a silent gap.

use crate::config::TenantConfig;
use crate::conn_limit::{ConnGuard, ConnLimiter};
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Cost-weighted request classes (GitHub secondary-rate-limit precedent —
/// see the ADR's Context for why a flat per-request count doesn't fit: a
/// `kb/search` graph scan and a `kb/get` point lookup impose wildly
/// different load, so treating them identically either starves scans or
/// under-protects against them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestCost {
    /// `kb/get`, `kb/links_from`, `kb/links_to`, `kb/list_ids`,
    /// `kb/id_title_pairs`, `kb/id_title_body_triples`, `kb/todo_nodes`,
    /// `kb/health`, `kb/hygiene_report` — a single bounded lookup.
    Read,
    /// `kb/search`, `kb/related`, `kb/neighborhood` — a broader traversal/
    /// scan across the store, named explicitly in the ADR's cost table.
    Scan,
    /// `kb/hygiene_scan` (writes suggestions), `kb/hygiene_accept`,
    /// `kb/hygiene_dismiss` — any mutating hygiene arm.
    Mutation,
}

impl RequestCost {
    fn points(self) -> i64 {
        match self {
            RequestCost::Read => 1,
            RequestCost::Scan => 3,
            RequestCost::Mutation => 5,
        }
    }
}

const WINDOW_SECS: u64 = 60;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Outcome of a quota check. `Unconfigured` — the resolved key (instance
/// address or principal) doesn't match any `[[tenant]]` entry, including the
/// all-important "zero `[[tenant]]` tables configured" case — is always
/// treated identically to `Admitted` by callers: this is Phase A's own
/// backward-compatibility contract (zero config, zero behavior change), not
/// a distinct error path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantOutcome {
    Unconfigured,
    Admitted,
    QuotaExceeded,
    ConnectionCapExceeded,
}

/// Fixed-window points bucket, reset when the window has elapsed. A plain
/// `Mutex`, not lock-free atomics: the critical section is a handful of
/// integer comparisons with no `.await` inside it, contended only by
/// requests from THIS one tenant (the entire point of keying per-tenant is
/// that one tenant's traffic never contends with another's), so there is no
/// correctness or latency reason to reach for a harder-to-verify CAS loop.
struct PointsWindow {
    start_secs: u64,
    spent: i64,
}

/// Live per-tenant quota/connection state. Cheap to reconstruct — evicting
/// this and letting the next request's `entry().or_insert_with()` rebuild it
/// from `TenantConfig` is the whole eviction mechanism (see
/// `TenantRegistry::evict`), mirroring
/// `crates/core/src/editor/window_ops.rs`'s `mcp_session_windows`
/// coarse-evict-then-self-heal idiom.
struct TenantQuotaState {
    conns: ConnLimiter,
    window: Mutex<PointsWindow>,
    budget_per_minute: i64,
    last_seen_secs: AtomicU64,
}

impl TenantQuotaState {
    fn new(cfg: &TenantConfig) -> Self {
        TenantQuotaState {
            conns: ConnLimiter::new(cfg.quota.max_connections),
            window: Mutex::new(PointsWindow {
                start_secs: now_secs(),
                spent: 0,
            }),
            budget_per_minute: cfg.quota.budget_per_minute as i64,
            last_seen_secs: AtomicU64::new(now_secs()),
        }
    }

    fn touch(&self) {
        self.last_seen_secs.store(now_secs(), Ordering::Relaxed);
    }

    fn idle_secs(&self) -> u64 {
        now_secs().saturating_sub(self.last_seen_secs.load(Ordering::Relaxed))
    }

    /// `budget_per_minute <= 0` means unlimited (mirrors `ConnLimiter::new`'s
    /// own `max == 0` = unlimited convention). Rolls the fixed window over
    /// when expired, then admits iff the charge fits under budget — a
    /// rejected request is never charged, so it costs nothing towards the
    /// next request's headroom.
    fn try_charge_points(&self, points: i64) -> bool {
        if self.budget_per_minute <= 0 {
            return true;
        }
        let now = now_secs();
        let mut w = self.window.lock().unwrap();
        if now.saturating_sub(w.start_secs) >= WINDOW_SECS {
            w.start_secs = now;
            w.spent = 0;
        }
        if w.spent + points > self.budget_per_minute {
            false
        } else {
            w.spent += points;
            true
        }
    }
}

/// Resolves either quota key (KB-socket instance address, or collab/OAuth
/// principal) to a tenant name, meters cost-weighted request points per
/// fixed window, and bounds concurrent in-flight requests per tenant.
/// Config (`by_instance`/`by_principal`/tenant definitions) is resolved once
/// at daemon startup — live config reload is explicitly out of scope for
/// this phase (see the ADR's "Explicitly out of scope" list).
pub struct TenantRegistry {
    configs: HashMap<String, TenantConfig>,
    by_instance: HashMap<String, String>,
    // Exercised by this module's own tests via `check_and_charge_by_principal`
    // below (real, tested capability); not yet read by any production caller
    // since no listener wires principal-keyed enforcement in this pass — see
    // the module doc's "Scope of this implementation pass". Not dead code to
    // delete, so `dead_code` is allowed deliberately rather than worked
    // around by leaving the whole capability half-built.
    #[allow(dead_code)]
    by_principal: HashMap<String, String>,
    states: DashMap<String, Arc<TenantQuotaState>>,
}

impl TenantRegistry {
    pub fn from_config(tenants: &[TenantConfig]) -> Self {
        let mut configs = HashMap::new();
        let mut by_instance = HashMap::new();
        let mut by_principal = HashMap::new();
        for t in tenants {
            for inst in &t.instances {
                by_instance.insert(inst.clone(), t.name.clone());
            }
            for p in &t.principals {
                by_principal.insert(p.clone(), t.name.clone());
            }
            configs.insert(t.name.clone(), t.clone());
        }
        TenantRegistry {
            configs,
            by_instance,
            by_principal,
            states: DashMap::new(),
        }
    }

    /// Zero `[[tenant]]` tables configured — the exact "no-op" registry
    /// `DaemonState::new()` starts with before `main()` loads real config.
    pub fn empty() -> Self {
        TenantRegistry::from_config(&[])
    }

    fn resolve_and_charge(
        &self,
        tenant_name: Option<&String>,
        cost: RequestCost,
    ) -> (TenantOutcome, Option<ConnGuard>) {
        let Some(name) = tenant_name else {
            return (TenantOutcome::Unconfigured, None);
        };
        let Some(cfg) = self.configs.get(name) else {
            return (TenantOutcome::Unconfigured, None);
        };
        let state = self
            .states
            .entry(name.clone())
            .or_insert_with(|| Arc::new(TenantQuotaState::new(cfg)))
            .clone();
        state.touch();
        let Some(guard) = state.conns.try_acquire() else {
            return (TenantOutcome::ConnectionCapExceeded, None);
        };
        if !state.try_charge_points(cost.points()) {
            drop(guard);
            return (TenantOutcome::QuotaExceeded, None);
        }
        (TenantOutcome::Admitted, Some(guard))
    }

    /// KB-socket path: `addr` is Phase A's `instance_addr(&params)` (`None`
    /// = the primary/federated instance, which is never itself a `[[tenant]]`
    /// member unless explicitly listed — matching Phase A's own "None
    /// preserves today's exact behavior" contract).
    pub fn check_and_charge_by_instance(
        &self,
        addr: Option<&str>,
        cost: RequestCost,
    ) -> (TenantOutcome, Option<ConnGuard>) {
        let tenant_name = addr.and_then(|a| self.by_instance.get(a));
        self.resolve_and_charge(tenant_name, cost)
    }

    /// Collab/OAuth path: `principal` is the authenticated fingerprint/PSK-id
    /// (`Session::authenticated_principal()` / OAuth's mapped `sub` claim).
    /// Not yet wired into `collab_handler` (see module doc) — implemented
    /// and tested here so that follow-on wiring only needs a call site, not
    /// new mechanism. `dead_code`-allowed for the same reason as
    /// `by_principal` above: real, tested, deliberately not yet called from
    /// production.
    #[allow(dead_code)]
    pub fn check_and_charge_by_principal(
        &self,
        principal: Option<&str>,
        cost: RequestCost,
    ) -> (TenantOutcome, Option<ConnGuard>) {
        let tenant_name = principal.and_then(|p| self.by_principal.get(p));
        self.resolve_and_charge(tenant_name, cost)
    }

    /// Manual eviction (the `daemon/evict_tenant` RPC — the rust-analyzer/
    /// Emacs-precedent operator escape hatch: don't wait for an idle window
    /// a still-connected tenant may never hit). Idempotent — evicting an
    /// already-absent or unknown tenant is a clean no-op, not an error.
    pub fn evict(&self, tenant_name: &str) {
        self.states.remove(tenant_name);
    }

    /// Idle-tenant sweep for `DaemonScheduler::run_maintenance_tick`. A
    /// tenant's `idle_evict_secs == 0` means "never idle-evict." Returns the
    /// evicted tenant names for logging.
    pub fn evict_idle(&self) -> Vec<String> {
        let mut evicted = Vec::new();
        self.states.retain(|name, state| {
            let idle_evict_secs = self
                .configs
                .get(name)
                .map(|c| c.quota.idle_evict_secs)
                .unwrap_or(0);
            if idle_evict_secs == 0 {
                return true;
            }
            if state.idle_secs() >= idle_evict_secs {
                evicted.push(name.clone());
                false
            } else {
                true
            }
        });
        evicted
    }

    /// Number of tenants with live (non-evicted) quota state. Diagnostic use.
    pub fn live_tenant_count(&self) -> usize {
        self.states.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TenantQuotaConfig;

    fn tenant(name: &str, instances: &[&str], budget_per_minute: u32) -> TenantConfig {
        TenantConfig {
            name: name.to_string(),
            instances: instances.iter().map(|s| s.to_string()).collect(),
            principals: Vec::new(),
            quota: TenantQuotaConfig {
                max_connections: 32,
                budget_per_minute,
                max_result_bytes: 4_194_304,
                idle_evict_secs: 1800,
            },
        }
    }

    /// Adversarial test 1: a quota-exceeding tenant is rejected before it
    /// can consume any capacity belonging to co-resident tenants — a
    /// genuine 3-tenant baseline (principle #14: N-way, not 2-way), not a
    /// single-tenant happy path that can't distinguish "isolated" from
    /// "just happened to work."
    #[test]
    fn quota_exceeding_tenant_rejected_without_touching_other_tenants_budget() {
        let registry = TenantRegistry::from_config(&[
            tenant("team-a", &["a-kb"], 10),
            tenant("team-b", &["b-kb"], 10),
            tenant("team-c", &["c-kb"], 10),
        ]);

        // team-a spends its entire budget (10 points of Scan @ 3 fits 3x = 9,
        // one more Read @1 tips to 10 — exactly at budget).
        for _ in 0..3 {
            let (outcome, _guard) =
                registry.check_and_charge_by_instance(Some("a-kb"), RequestCost::Scan);
            assert_eq!(outcome, TenantOutcome::Admitted);
        }
        let (outcome, _guard) =
            registry.check_and_charge_by_instance(Some("a-kb"), RequestCost::Read);
        assert_eq!(
            outcome,
            TenantOutcome::Admitted,
            "exactly at budget, 9+1=10"
        );

        // The next request for team-a is over budget and must be rejected.
        let (outcome, guard) =
            registry.check_and_charge_by_instance(Some("a-kb"), RequestCost::Read);
        assert_eq!(outcome, TenantOutcome::QuotaExceeded);
        assert!(
            guard.is_none(),
            "a rejected request must not hold a conn guard"
        );

        // team-b and team-c, never having spent a point, are completely
        // unaffected by team-a's exhausted budget — real per-tenant
        // isolation, not a shared global counter that merely LOOKS isolated
        // because only one tenant was ever exercised.
        for kb in ["b-kb", "c-kb"] {
            for _ in 0..3 {
                let (outcome, _guard) =
                    registry.check_and_charge_by_instance(Some(kb), RequestCost::Scan);
                assert_eq!(
                    outcome,
                    TenantOutcome::Admitted,
                    "{kb} must be unaffected by team-a's exhausted budget"
                );
            }
        }
    }

    /// Adversarial test 2: KB-socket isolation is genuinely keyed on Phase
    /// A's instance address, not accidentally per-call/per-connection — the
    /// nearest wrong-but-plausible copy-paste, given `ConnLimiter`'s own
    /// shape is per-accepted-connection. Two DIFFERENT addresses that both
    /// belong to the SAME tenant share its one budget; a third address
    /// belonging to a DIFFERENT tenant is unaffected.
    #[test]
    fn kb_socket_isolation_is_keyed_on_instance_not_per_call() {
        let registry = TenantRegistry::from_config(&[
            tenant("multi-instance-tenant", &["kb-one", "kb-two"], 5),
            tenant("other-tenant", &["kb-three"], 5),
        ]);

        // kb-one and kb-two are the SAME tenant: exhausting the budget via
        // kb-one must be visible when addressing kb-two next.
        for _ in 0..5 {
            let (outcome, _g) =
                registry.check_and_charge_by_instance(Some("kb-one"), RequestCost::Read);
            assert_eq!(outcome, TenantOutcome::Admitted);
        }
        let (outcome, _g) =
            registry.check_and_charge_by_instance(Some("kb-two"), RequestCost::Read);
        assert_eq!(
            outcome,
            TenantOutcome::QuotaExceeded,
            "kb-two shares kb-one's tenant budget, already exhausted"
        );

        // kb-three is a genuinely different tenant, unaffected.
        let (outcome, _g) =
            registry.check_and_charge_by_instance(Some("kb-three"), RequestCost::Read);
        assert_eq!(outcome, TenantOutcome::Admitted);
    }

    /// Adversarial test 3: the mirror case for the (not-yet-wired-into-a-
    /// listener, but implemented) principal-keyed collab/OAuth path — proves
    /// the two key spaces (instance vs. principal) never cross-contaminate:
    /// a principal string that happens to collide textually with an
    /// instance address must not accidentally resolve to that instance's
    /// tenant.
    #[test]
    fn principal_keyed_isolation_never_crosses_into_instance_keyed_tenants() {
        let mut instance_tenant = tenant("instance-tenant", &["shared-name"], 100);
        instance_tenant.principals = Vec::new();
        let mut principal_tenant = tenant("principal-tenant", &[], 5);
        principal_tenant.principals = vec!["shared-name".to_string()];

        let registry = TenantRegistry::from_config(&[instance_tenant, principal_tenant]);

        // Exhaust "principal-tenant"'s budget via the principal-keyed path.
        for _ in 0..5 {
            let (outcome, _g) =
                registry.check_and_charge_by_principal(Some("shared-name"), RequestCost::Read);
            assert_eq!(outcome, TenantOutcome::Admitted);
        }
        let (outcome, _g) =
            registry.check_and_charge_by_principal(Some("shared-name"), RequestCost::Read);
        assert_eq!(outcome, TenantOutcome::QuotaExceeded);

        // The SAME literal string, addressed via the INSTANCE-keyed path,
        // must resolve to the completely separate "instance-tenant" (budget
        // 100, untouched) rather than accidentally sharing state with
        // "principal-tenant" merely because the raw key string matches.
        let (outcome, _g) =
            registry.check_and_charge_by_instance(Some("shared-name"), RequestCost::Read);
        assert_eq!(
            outcome,
            TenantOutcome::Admitted,
            "instance-keyed 'shared-name' must resolve to instance-tenant's own \
             untouched budget, not principal-tenant's exhausted one"
        );
    }

    /// Adversarial test 4: unbounded-growth eviction (the rust-analyzer/
    /// Emacs precedent this phase names explicitly) — evicting a tenant
    /// mid-session must self-heal cleanly on its next request (a fresh
    /// budget, not a permanently-broken tenant), and must never touch a
    /// co-resident tenant's live state.
    #[test]
    fn evicting_a_tenant_self_heals_and_never_disturbs_co_resident_tenants() {
        let registry = TenantRegistry::from_config(&[
            tenant("evictee", &["e-kb"], 3),
            tenant("survivor", &["s-kb"], 3),
        ]);

        // Both tenants spend into existence and partially exhaust budget.
        let (o, _g) = registry.check_and_charge_by_instance(Some("e-kb"), RequestCost::Scan);
        assert_eq!(o, TenantOutcome::Admitted);
        let (o, _g) = registry.check_and_charge_by_instance(Some("s-kb"), RequestCost::Scan);
        assert_eq!(o, TenantOutcome::Admitted);
        assert_eq!(registry.live_tenant_count(), 2);

        registry.evict("evictee");
        assert_eq!(
            registry.live_tenant_count(),
            1,
            "eviction must remove exactly the evicted tenant's state"
        );

        // Self-heal: evictee's next request gets a FRESH budget (3 points
        // fit again), proving the eviction didn't leave a poisoned/missing
        // entry that errors instead of self-healing.
        for _ in 0..3 {
            let (o, _g) = registry.check_and_charge_by_instance(Some("e-kb"), RequestCost::Read);
            assert_eq!(
                o,
                TenantOutcome::Admitted,
                "evictee must self-heal with a fresh budget"
            );
        }

        // survivor's state was never touched by evictee's eviction: it still
        // has exactly its pre-eviction 3-point spend on record, so one more
        // Scan (3 points, total 6) must now be rejected under its 3-point-
        // per-minute budget... but it already spent 3, so it's already at
        // the boundary — one more Read (1 point) must be rejected.
        let (o, _g) = registry.check_and_charge_by_instance(Some("s-kb"), RequestCost::Read);
        assert_eq!(
            o,
            TenantOutcome::QuotaExceeded,
            "survivor's pre-eviction spend must be intact -- evicting evictee must not \
             have reset or otherwise disturbed survivor's independent state"
        );

        // Idempotent: evicting an unknown/already-absent tenant is a no-op,
        // not an error.
        registry.evict("evictee");
        registry.evict("never-existed");
    }

    /// Adversarial test 5: cost-weighting negative case — a read-only tenant
    /// must not exhaust its budget anywhere near as fast as a same-request-
    /// count mutation-heavy tenant, proving the weights are actually applied
    /// (not silently flattened to "1 point per request" somewhere).
    #[test]
    fn cost_weighting_makes_mutation_heavy_tenant_exhaust_far_faster_than_read_only() {
        let registry = TenantRegistry::from_config(&[
            tenant("read-only", &["ro-kb"], 15),
            tenant("mutation-heavy", &["mh-kb"], 15),
        ]);

        // 15 Read-cost (1pt) requests exactly fit a 15-point budget.
        let mut admitted_reads = 0;
        for _ in 0..15 {
            let (o, _g) = registry.check_and_charge_by_instance(Some("ro-kb"), RequestCost::Read);
            if o == TenantOutcome::Admitted {
                admitted_reads += 1;
            }
        }
        assert_eq!(
            admitted_reads, 15,
            "15 x 1-point reads must exactly fit a 15-point budget"
        );

        // The identical REQUEST COUNT (15) of Mutation-cost (5pt) requests
        // against the same-sized budget must exhaust far sooner -- exactly
        // 3 admitted (15/5), not anywhere near 15.
        let mut admitted_mutations = 0;
        for _ in 0..15 {
            let (o, _g) =
                registry.check_and_charge_by_instance(Some("mh-kb"), RequestCost::Mutation);
            if o == TenantOutcome::Admitted {
                admitted_mutations += 1;
            }
        }
        assert_eq!(
            admitted_mutations, 3,
            "5-point mutations must exhaust a 15-point budget after exactly 3 requests, \
             proving cost weighting is genuinely applied, not flattened to a flat per-request count"
        );
    }

    /// Adversarial test 6: the connection-cap dimension is genuinely
    /// SEPARATE from the points budget — a tenant with an exhausted
    /// connection cap but a fully untouched points budget must still be
    /// rejected (and vice versa is exercised by test 1/5's points-only
    /// exhaustion above already succeeding while connections stay free).
    #[test]
    fn connection_cap_and_points_budget_are_independently_enforced() {
        let mut cfg = tenant("conn-capped", &["cc-kb"], 1000);
        cfg.quota.max_connections = 2;
        let registry = TenantRegistry::from_config(&[cfg]);

        let (o1, g1) = registry.check_and_charge_by_instance(Some("cc-kb"), RequestCost::Read);
        assert_eq!(o1, TenantOutcome::Admitted);
        let (o2, g2) = registry.check_and_charge_by_instance(Some("cc-kb"), RequestCost::Read);
        assert_eq!(o2, TenantOutcome::Admitted);

        // A 3rd concurrent request is rejected on the CONNECTION cap despite
        // a nearly-untouched 1000-point budget (only 2 points spent) --
        // proving this isn't secretly the same mechanism as the points
        // budget wearing a different name.
        let (o3, g3) = registry.check_and_charge_by_instance(Some("cc-kb"), RequestCost::Read);
        assert_eq!(o3, TenantOutcome::ConnectionCapExceeded);
        assert!(g3.is_none());

        // Freeing one in-flight "connection" (dropping its guard) admits the
        // next request again -- the cap tracks genuinely concurrent
        // in-flight work, not a cumulative counter.
        drop(g1);
        let (o4, _g4) = registry.check_and_charge_by_instance(Some("cc-kb"), RequestCost::Read);
        assert_eq!(o4, TenantOutcome::Admitted);
        drop(g2);
        drop(_g4);
    }

    #[test]
    fn unconfigured_instance_is_always_admitted_zero_behavior_change() {
        let registry = TenantRegistry::empty();
        for _ in 0..50 {
            let (o, _g) =
                registry.check_and_charge_by_instance(Some("anything"), RequestCost::Mutation);
            assert_eq!(o, TenantOutcome::Unconfigured);
        }
        let (o, _g) = registry.check_and_charge_by_instance(None, RequestCost::Mutation);
        assert_eq!(o, TenantOutcome::Unconfigured);
    }

    #[test]
    fn zero_budget_per_minute_means_unlimited_matching_conn_limiter_convention() {
        let registry = TenantRegistry::from_config(&[tenant("unlimited", &["u-kb"], 0)]);
        for _ in 0..500 {
            let (o, _g) =
                registry.check_and_charge_by_instance(Some("u-kb"), RequestCost::Mutation);
            assert_eq!(o, TenantOutcome::Admitted);
        }
    }
}
