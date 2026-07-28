//! `KbCollectionDoc`: ADR-033 advisory lease claims for coordinating expensive,
//! KB-wide bulk operations (enrichment sweeps, embedding rebuilds) across daemons.
//!
//! See `mod.rs`'s `LeaseClaim` doc comment and the `COLL_LEASE_KEY` const comment
//! for the data-model rationale: a SINGLE flat `YMap<claim_key -> claim record>`
//! (op_kind stored as a field on each entry), not `YMap<op_kind -> YMap<claim_key
//! -> record>>`. A nested map is unsafe here: if two peers who have never synced
//! each independently create the SAME not-yet-existing nested map (e.g. both are
//! the first to ever claim "enrichment"), yrs resolves the top-level key's value
//! via last-writer-wins — ONE peer's entire subtree wins outright and the other's
//! entries are silently dropped, not merged (confirmed by a failing round-trip test
//! during development: two concurrent claimants converged to a single BLANK
//! `op_claims` map missing one side's entry, not the union of both). Flattening to
//! one level means every claim is just a new key in an ALREADY-ESTABLISHED shared
//! map (`COLL_LEASE_KEY`, eagerly seeded by every constructor — see
//! `collection_core.rs`/`collection_roles.rs`) — inserting a fresh key into an
//! existing shared YMap concurrently is the safe, ordinary case every other
//! per-principal map in this file (`member_roles`, `pending`) already relies on.

use yrs::{Map, MapPrelim, Out, ReadTxn, Transact};

use super::*;

/// Sub-key of one claim entry (a YMap value under `COLL_LEASE_KEY`, keyed by
/// `claim_key`) recording which `op_kind` this attempt was for — needed now that
/// entries for every op_kind share one flat map.
const LEASE_OP_KIND_KEY: &str = "op_kind";

impl KbCollectionDoc {
    /// Attempt to claim (or renew) the advisory lease for `op_kind` on behalf of
    /// `holder_fp`. Returns the encoded update to persist+broadcast — **empty**
    /// (`Vec::new()`) if the claim was refused (an unexpired claim from a different,
    /// tiebreak-winning holder already exists), matching `blank_node_titles_delta`'s
    /// established "no mutation happened" idiom. The caller should re-read
    /// [`Self::current_lease`] afterward to see who actually holds it — a returned
    /// non-empty delta does not by itself guarantee `holder_fp` won.
    ///
    /// A call from the CURRENT holder (same `holder_fp`, unexpired, same `op_kind`)
    /// is a renewal: it re-stamps `claimed_at` without advancing `generation` (same
    /// grant, not a new one) — the entry is looked up by
    /// `claim_key = "{op_kind}:{holder_fp}@{now}"`, so a renewal within the same
    /// unix second collapses into the prior entry; a renewal in a later second
    /// creates a fresh entry at the SAME generation.
    pub fn claim_lease(
        &mut self,
        op_kind: &str,
        holder_fp: &str,
        ttl_secs: u64,
        now: u64,
    ) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        // `COLL_LEASE_KEY` is eagerly seeded by every constructor for NEW docs; the
        // fallback create-if-missing here only serves a pre-existing collection doc
        // that predates this feature (a narrow, effectively one-time migration
        // path — the same caveat `member_roles_map`'s fallback branch already
        // carries for legacy v1 docs, see `collection_roles.rs`).
        let leases = match root.get(&txn, COLL_LEASE_KEY) {
            Some(Out::YMap(m)) => m,
            _ => root.insert(&mut txn, COLL_LEASE_KEY, MapPrelim::default()),
        };

        // Scan existing attempts FOR THIS op_kind: the highest generation seen so
        // far (for assigning the new one), whether an unexpired claim from a
        // DIFFERENT holder currently blocks this attempt (tiebreak: highest
        // fingerprint wins), and whether the caller already holds an unexpired
        // claim (a renewal).
        let mut max_generation: u64 = 0;
        let mut blocking_holder: Option<String> = None;
        let mut is_renewal = false;
        for (_key, v) in leases.iter(&txn) {
            let Out::YMap(entry) = v else { continue };
            if entry
                .get(&txn, LEASE_OP_KIND_KEY)
                .map(|x| x.to_string(&txn))
                .unwrap_or_default()
                != op_kind
            {
                continue;
            }
            let h = entry
                .get(&txn, LEASE_HOLDER_KEY)
                .map(|x| x.to_string(&txn))
                .unwrap_or_default();
            let at = read_u64(&entry, &txn, LEASE_CLAIMED_AT_KEY);
            let ttl = read_u64(&entry, &txn, LEASE_TTL_KEY);
            let gen = read_u64(&entry, &txn, LEASE_GENERATION_KEY);
            max_generation = max_generation.max(gen);
            let unexpired = now < at.saturating_add(ttl);
            if unexpired && h == holder_fp {
                is_renewal = true;
            }
            if unexpired && h != holder_fp {
                // ADR-026 deterministic tiebreak: highest fingerprint wins. Track
                // the strongest blocker seen (if several distinct holders somehow
                // hold concurrently-unexpired claims, only the strongest matters).
                let stronger = blocking_holder.as_deref().is_none_or(|b| h.as_str() > b);
                if stronger {
                    blocking_holder = Some(h);
                }
            }
        }

        if let Some(blocker) = &blocking_holder {
            if blocker.as_str() > holder_fp {
                // An unexpired claim from a strictly-higher fingerprint blocks us —
                // no-op, no mutation.
                return Vec::new();
            }
            // We win the tiebreak against the blocker — fall through and claim.
        }

        let final_generation = if is_renewal || max_generation == 0 {
            max_generation.max(1)
        } else {
            max_generation + 1
        };

        let claim_key = format!("{op_kind}:{holder_fp}@{now}");
        let entry = leases.insert(&mut txn, claim_key.as_str(), MapPrelim::default());
        entry.insert(&mut txn, LEASE_OP_KIND_KEY, op_kind);
        entry.insert(&mut txn, LEASE_HOLDER_KEY, holder_fp);
        entry.insert(&mut txn, LEASE_CLAIMED_AT_KEY, now.to_string());
        entry.insert(&mut txn, LEASE_TTL_KEY, ttl_secs.to_string());
        entry.insert(&mut txn, LEASE_GENERATION_KEY, final_generation.to_string());
        txn.encode_update_v1()
    }

    /// The current, unexpired lease holder for `op_kind` (as of `now`), or `None`
    /// if no unexpired claim exists. Deterministically DERIVED by replaying every
    /// recorded attempt (mirrors `derive_valid_members`'s replay-the-oplog pattern)
    /// rather than trusting a single stored "current" pointer — every peer that has
    /// synced the same set of attempts computes the identical winner: highest
    /// `generation`, ties broken by highest `holder_fp`.
    pub fn current_lease(&self, op_kind: &str, now: u64) -> Option<LeaseClaim> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        let Some(Out::YMap(leases)) = root.get(&txn, COLL_LEASE_KEY) else {
            return None;
        };
        let mut best: Option<LeaseClaim> = None;
        for (_key, v) in leases.iter(&txn) {
            let Out::YMap(entry) = v else { continue };
            if entry
                .get(&txn, LEASE_OP_KIND_KEY)
                .map(|x| x.to_string(&txn))
                .unwrap_or_default()
                != op_kind
            {
                continue;
            }
            let holder_fp = entry
                .get(&txn, LEASE_HOLDER_KEY)
                .map(|x| x.to_string(&txn))
                .unwrap_or_default();
            let claimed_at = read_u64(&entry, &txn, LEASE_CLAIMED_AT_KEY);
            let lease_ttl_secs = read_u64(&entry, &txn, LEASE_TTL_KEY);
            let generation = read_u64(&entry, &txn, LEASE_GENERATION_KEY);
            let candidate = LeaseClaim {
                op_kind: op_kind.to_string(),
                holder_fp,
                claimed_at,
                lease_ttl_secs,
                generation,
            };
            if candidate.is_expired(now) {
                continue;
            }
            best = Some(match best {
                None => candidate,
                Some(cur)
                    if candidate.generation > cur.generation
                        || (candidate.generation == cur.generation
                            && candidate.holder_fp > cur.holder_fp) =>
                {
                    candidate
                }
                Some(cur) => cur,
            });
        }
        best
    }
}

fn read_u64(entry: &yrs::MapRef, txn: &impl ReadTxn, key: &str) -> u64 {
    entry
        .get(txn, key)
        .map(|v| v.to_string(txn))
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0)
}
