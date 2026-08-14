//! Tests for [`super`]'s collab/OAuth quota surface (#456).
//!
//! Extracted under CLAUDE.md's file-ceiling remedy when the #456 wiring pushed
//! `tenant.rs` past 800 lines — the same `watch.rs` / `watch_tests.rs` precedent.
//! `#[path]` adds a module level, so the inner module uses `super::super::*`.

#[cfg(test)]
mod tests {
    //! ADVERSARIAL (#456): the collab/OAuth cost table and principal-keyed
    //! isolation, exercised through the real `TenantQuota` charger.
    //!
    //! The wiring half — that `handle_doc_request_inner` actually consults this —
    //! is pinned in `collab_handler::tests::collab_handler_tenant_quota_tests`,
    //! which lives in the library crate and uses a stub charger. Split that way
    //! because `TenantRegistry` is a binary-crate type the library cannot name.
    use super::super::*;
    use crate::config::{TenantConfig, TenantQuotaConfig};
    use mae_daemon::quota::QuotaCharger;

    fn fp(label: &str) -> String {
        format!("SHA256:{label}")
    }

    fn tenant(name: &str, principal: &str, budget: u32) -> TenantConfig {
        TenantConfig {
            name: name.to_string(),
            instances: Vec::new(),
            principals: vec![principal.to_string()],
            quota: TenantQuotaConfig {
                budget_per_minute: budget,
                ..Default::default()
            },
        }
    }

    fn charger(tenants: &[TenantConfig]) -> TenantQuota {
        TenantQuota(Arc::new(TenantRegistry::from_config(tenants)))
    }

    /// Three tenants, so this cannot pass by accident on a two-way split:
    /// exhausting one must leave BOTH others untouched.
    #[test]
    fn exhausting_one_tenants_budget_does_not_affect_the_others() {
        let q = charger(&[
            tenant("alpha", &fp("alice"), 4),
            tenant("beta", &fp("bob"), 4),
            tenant("gamma", &fp("carol"), 4),
        ]);
        for i in 0..4 {
            assert!(
                q.charge(Some(&fp("alice")), "sync/state_vector").is_ok(),
                "alice request {i} is within a 4-point budget"
            );
        }
        assert!(
            q.charge(Some(&fp("alice")), "sync/state_vector").is_err(),
            "alice's 5th read must exceed a 4-point budget"
        );
        for who in ["bob", "carol"] {
            assert!(
                q.charge(Some(&fp(who)), "sync/state_vector").is_ok(),
                "{who} must be unaffected by alice exhausting her own budget"
            );
        }
    }

    /// Cost weighting must actually differ by method, or the table is decoration.
    #[test]
    fn a_mutation_costs_more_of_the_budget_than_a_read() {
        let reads = charger(&[tenant("alpha", &fp("alice"), 5)]);
        for _ in 0..5 {
            assert!(reads
                .charge(Some(&fp("alice")), "sync/state_vector")
                .is_ok());
        }
        assert!(
            reads
                .charge(Some(&fp("alice")), "sync/state_vector")
                .is_err(),
            "5 reads exactly consume a 5-point budget"
        );

        let writes = charger(&[tenant("alpha", &fp("alice"), 5)]);
        assert!(writes.charge(Some(&fp("alice")), "kb/node_update").is_ok());
        assert!(
            writes
                .charge(Some(&fp("alice")), "sync/state_vector")
                .is_err(),
            "one 5-point mutation consumes the whole budget, leaving no room even \
             for a 1-point read"
        );
    }

    /// A scan sits strictly between the two. Pinned separately so a later edit
    /// collapsing `Scan` into either neighbour fails loudly rather than silently
    /// re-pricing every whole-document sync on a hosted daemon.
    #[test]
    fn a_scan_costs_more_than_a_read_and_less_than_a_mutation() {
        let q = charger(&[tenant("alpha", &fp("alice"), 3)]);
        assert!(q.charge(Some(&fp("alice")), "sync/full_state").is_ok());
        assert!(
            q.charge(Some(&fp("alice")), "sync/state_vector").is_err(),
            "a 3-point scan consumes a 3-point budget"
        );

        let q = charger(&[tenant("alpha", &fp("alice"), 4)]);
        assert!(q.charge(Some(&fp("alice")), "sync/full_state").is_ok());
        assert!(
            q.charge(Some(&fp("alice")), "sync/state_vector").is_ok(),
            "3+1 fits in 4; if a scan cost as much as a mutation it would not"
        );
    }

    /// ADR-060 Phase A's contract: zero configuration, zero behaviour change.
    /// Without this, wiring quotas would silently throttle every single-user
    /// daemon that has no `[[tenant]]` tables at all.
    #[test]
    fn unconfigured_and_unauthenticated_principals_are_always_admitted() {
        let q = charger(&[tenant("alpha", &fp("alice"), 1)]);
        for i in 0..50 {
            assert!(
                q.charge(Some(&fp("stranger")), "kb/node_update").is_ok(),
                "a principal in no tenant must never be throttled (attempt {i})"
            );
            assert!(
                q.charge(None, "kb/node_update").is_ok(),
                "an unauthenticated caller must never be throttled (attempt {i})"
            );
        }
        let none = charger(&[]);
        for _ in 0..50 {
            assert!(none.charge(Some(&fp("alice")), "kb/node_update").is_ok());
        }
    }

    /// An unknown method must be the CHEAPEST class. Charging unknown methods as
    /// mutations would let anyone drain a tenant's budget with nonsense method
    /// names the dispatcher rejects anyway.
    #[test]
    fn an_unknown_method_is_charged_as_a_read_not_a_mutation() {
        let q = charger(&[tenant("alpha", &fp("alice"), 4)]);
        for i in 0..4 {
            assert!(
                q.charge(Some(&fp("alice")), "not/a/real/method").is_ok(),
                "unknown method {i} must cost 1 point, not 5"
            );
        }
        assert!(q.charge(Some(&fp("alice")), "not/a/real/method").is_err());
    }
}
