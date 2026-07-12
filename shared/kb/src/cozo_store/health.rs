//! KB health report (Phase F): node/link counts, by-kind and by-rel-type
//! breakdowns, orphan detection, broken-link diagnosis, and hub ranking —
//! all computed via Datalog queries over `nodes`/`links`.

use super::util::cozo_err;
use super::*;

impl CozoKbStore {
    /// Compute a structured health report using Datalog queries.
    pub fn health_report(&self) -> Result<HealthReport, KbStoreError> {
        use crate::store::{BrokenLinkInfo, BrokenLinkReason};

        // Total counts
        let total_nodes = self
            .run_immut("?[id] := *nodes{id, title}, title != ''")
            .map_err(cozo_err)?
            .rows
            .len();
        let total_links = self
            .run_immut("?[src, dst, rt] := *links{src, dst, rel_type: rt}")
            .map_err(cozo_err)?
            .rows
            .len();

        // Nodes by kind
        let kind_result = self
            .run_immut("?[kind, id] := *nodes{id, kind, title}, title != ''")
            .map_err(cozo_err)?;
        let mut by_kind: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for row in &kind_result.rows {
            if let Some(kind) = row.first().and_then(|v| v.get_str()) {
                *by_kind.entry(kind.to_string()).or_default() += 1;
            }
        }

        // Namespace counts (derived from node ID prefixes)
        let ns_result = self
            .run_immut("?[id] := *nodes{id, title}, title != ''")
            .map_err(cozo_err)?;
        let mut namespace_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for row in &ns_result.rows {
            if let Some(id) = row.first().and_then(|v| v.get_str()) {
                let ns = if let Some(colon) = id.find(':') {
                    &id[..colon]
                } else {
                    "(none)"
                };
                *namespace_counts.entry(ns.to_string()).or_default() += 1;
            }
        }

        // Links by type
        let rel_result = self
            .run_immut("?[rt, src, dst] := *links{src, dst, rel_type: rt}")
            .map_err(cozo_err)?;
        let mut by_rel_type: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for row in &rel_result.rows {
            if let Some(rt) = row.first().and_then(|v| v.get_str()) {
                *by_rel_type.entry(rt.to_string()).or_default() += 1;
            }
        }

        // Orphan nodes (no links in or out) — returns IDs
        let orphan_result = self.run_immut(
            "has_links[id] := *links{src: id}\nhas_links[id] := *links{dst: id}\n?[id] := *nodes{id, title}, not has_links[id], title != ''"
        ).map_err(cozo_err)?;
        let orphan_ids: Vec<String> = orphan_result
            .rows
            .iter()
            .filter_map(|row| row.first().and_then(|v| v.get_str()).map(|s| s.to_string()))
            .collect();

        // Broken links (target doesn't exist) — returns details
        let broken_result = self.run_immut(
            "exists[id] := *nodes{id, title}, title != ''\n?[src, dst, rt] := *links{src, dst, rel_type: rt}, not exists[dst]"
        ).map_err(cozo_err)?;
        let broken_links: Vec<BrokenLinkInfo> = broken_result
            .rows
            .iter()
            .filter_map(|row| {
                let src = row.first()?.get_str()?.to_string();
                let dst = row.get(1)?.get_str()?.to_string();
                let rt = row.get(2)?.get_str()?.to_string();
                let reason = if dst.contains(':') || dst.len() > 3 {
                    BrokenLinkReason::DeletedNode
                } else {
                    BrokenLinkReason::MalformedId
                };
                Some(BrokenLinkInfo {
                    source: src,
                    target: dst,
                    rel_type: rt,
                    reason,
                })
            })
            .collect();

        // Hub nodes (highest in-degree, top 10)
        let hub_result = self
            .run_immut("in_deg[dst, id] := *links{dst, src: id}\n?[dst, id] := in_deg[dst, id]")
            .map_err(cozo_err)?;
        let mut hub_map: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for row in &hub_result.rows {
            if let Some(dst) = row.first().and_then(|v| v.get_str()) {
                *hub_map.entry(dst.to_string()).or_default() += 1;
            }
        }
        let mut hubs: Vec<(String, usize)> = hub_map.into_iter().collect();
        hubs.sort_by_key(|h| std::cmp::Reverse(h.1));
        hubs.truncate(10);

        Ok(HealthReport {
            total_nodes,
            total_links,
            namespace_counts,
            by_kind,
            by_rel_type,
            orphan_ids,
            broken_links,
            hub_nodes: hubs,
        })
    }
}
