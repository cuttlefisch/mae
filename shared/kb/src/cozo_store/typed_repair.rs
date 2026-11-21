//! Writing typed edges back into the bodies that lost them (#854, #941).
//!
//! Before mae-org ingest, a stored body had its links' `?rel=&w=&c=`
//! stripped, and the full reimport wrote the typed edges as `links` rows
//! beside it (`add_typed_link`). Those rows were the only record of the
//! relation: a store that became native (ADR-110) has no file left to re-read
//! it from, and the next write of the node -- or a re-derivation -- rebuilds
//! its rows from the body and drops them. Measured on real stores: two native
//! KBs held hundreds of such rows, none declared by any body.
//!
//! [`CozoKbStore::repair_typed_links`] makes the body the single truth again
//! (ADR-029/030): for every typed row (`rel_type != references`) its source
//! node's body does not declare, the body is changed to declare it
//! ([`org_ingest::declare_edges`]: the untyped link to that target is given
//! the relation, or a link is appended as `kb_add_link` appends one).
//!
//! @ai-caution: [kb-links] Ordering and concurrency. Both the editor and
//! mae-daemon open these stores, and either may be first. The repair runs
//! inside open, in the same version-gated pass as the edge re-derivation and
//! BEFORE it (`ensure_derivation_current`). Passes are serialized across
//! processes by the derivation lock (schema.rs), held for the whole open: a
//! second opener waits, then finds the stamp written and loads the repaired
//! store. Within a pass, each node is still re-read just before its write,
//! and edges its fresh body already declares are not declared again, because
//! an ordinary write by another process (not a pass) can land between the
//! plan and the write. The residual window is one read-to-write, milliseconds:
//! such a write landing exactly there is replaced by the repaired version of
//! the body read just before it.
//!
//! The re-derivation never deletes a typed row (it deletes only `references`
//! rows no body declares). So a pass that fails or is interrupted midway, or a
//! row this repair cannot or must not declare, loses nothing: the stamp is
//! written only after a complete pass, and the next open resumes, skipping
//! every node already repaired. Rows it must not declare are a seed node's, a
//! [`SEEDED_RELATIONSHIPS`] row, an external URL, or any CRDT-backed node's (see
//! `repair_node`). A row left undeclared is dropped only by an authored write
//! of its node, which is the rule from here on: the body is the truth.
//!
//! Each node goes through the store's own node write, `insert_node_with_history`
//! (ADR-106 records the version replaced).
//!
//! [`SEEDED_RELATIONSHIPS`]: CozoKbStore::SEEDED_RELATIONSHIPS

use super::util::cozo_err;
use super::*;
use crate::org_ingest::{self, WantedEdge};
use std::collections::BTreeMap as Map;

/// The change summary history records for a repaired node.
pub const TYPED_REPAIR_SUMMARY: &str = "typed links written into the body (#854)";

/// Nodes written between two checks of the interruption seam (and of
/// nothing else: every node write is its own transaction).
const REPAIR_BATCH: usize = 64;

/// The suffix of the copy a store gets before its first repair:
/// `<store>.pre-typed-links.bak-<unix seconds>`.
const BACKUP_INFIX: &str = ".pre-typed-links.bak-";

/// What [`CozoKbStore::repair_typed_links`] changed. Counts only.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TypedRepairReport {
    /// Nodes whose body now declares rows it did not.
    pub nodes_repaired: usize,
    /// Rows declared by giving an existing link the relation.
    pub rows_rewritten: usize,
    /// Rows declared by appending a link.
    pub rows_appended: usize,
    /// Undeclared typed rows left as they are (kept, not declared).
    pub rows_left: usize,
    /// Legacy `[[TARGET|DESCRIPTION]]` links in table rows rewritten to
    /// `[[TARGET][DESCRIPTION]]` (see `org_ingest::repair_table_pipe_links`).
    pub table_links_repaired: usize,
    /// CRDT-backed nodes the repair would have changed and left alone (see
    /// `repair_node`): their typed rows are kept, but a table link of theirs
    /// stays unrepaired. Logged, so it is never silent.
    pub crdt_nodes_skipped: usize,
    /// The copy taken before the first change, when one was taken.
    pub backup: Option<PathBuf>,
}

impl TypedRepairReport {
    /// Whether no body changed.
    pub fn is_noop(&self) -> bool {
        self.nodes_repaired == 0
    }
}

impl CozoKbStore {
    /// Declare, in its source node's body, every typed `links` row the body
    /// does not declare (see the module doc). Idempotent: a node whose body
    /// declares all its rows is not read twice nor written. Takes a backup of
    /// a file store before its first change.
    pub fn repair_typed_links(&self) -> Result<TypedRepairReport, KbStoreError> {
        let mut report = TypedRepairReport::default();
        let plan = self.typed_repair_plan(&mut report)?;
        if plan.is_empty() {
            return Ok(report);
        }
        report.backup = self.backup_before_typed_repair()?;
        #[cfg(test)]
        super::test_hooks::run_after_typed_repair_plan();
        for (n, batch) in plan.chunks(REPAIR_BATCH).enumerate() {
            #[cfg(test)]
            if super::test_hooks::fail_typed_repair_at_batch(n) {
                return Err(KbStoreError::Storage(
                    "test hook: typed repair interrupted".into(),
                ));
            }
            let _ = n;
            for (node, wanted) in batch {
                self.repair_node(node, wanted, &mut report)?;
            }
        }
        Ok(report)
    }

    /// Every node with typed rows its body does not declare, and those rows
    /// (sorted, so a re-run makes the same choices). Rows it must not or
    /// cannot declare are counted in `rows_left`.
    fn typed_repair_plan(
        &self,
        report: &mut TypedRepairReport,
    ) -> Result<Vec<(Node, Vec<WantedEdge>)>, KbStoreError> {
        let mut by_src: Map<String, Vec<WantedEdge>> = Map::new();
        for r in self
            .run_immut(
                "?[src, dst, rel, weight, confidence] := \
                 *links{src, dst, rel_type: rel, weight, confidence}, rel != 'references'",
            )
            .map_err(cozo_err)?
            .rows
        {
            let s = |i: usize| r.get(i).and_then(|v| v.get_str()).unwrap_or("").to_string();
            let f = |i: usize| r.get(i).and_then(|v| v.get_float()).unwrap_or(1.0);
            let (src, dst, rel) = (s(0), s(1), s(2));
            let seeded = Self::SEEDED_RELATIONSHIPS
                .iter()
                .any(|(a, b, c, _)| *a == src && *b == dst && *c == rel);
            if seeded {
                continue;
            }
            by_src.entry(src).or_default().push(WantedEdge {
                dst,
                rel,
                weight: f(3),
                confidence: f(4),
            });
        }
        // Nodes with legacy bar-form links in table rows, which the same
        // pass rewrites (they derive no edge until it does).
        for r in self
            .run_immut(
                "?[id] := *nodes{id, body, source}, source != 'seed', \
                 str_includes(body, '[['), str_includes(body, '|')",
            )
            .map_err(cozo_err)?
            .rows
        {
            if let Some(id) = r.first().and_then(|v| v.get_str()) {
                by_src.entry(id.to_string()).or_default();
            }
        }
        let mut plan = Vec::new();
        for (src, rows) in by_src {
            // A row whose source is not a node here is no body's to declare.
            let Some(node) = self.get_node(&src)? else {
                continue;
            };
            let declared: std::collections::HashSet<(String, String)> = Self::derived_links(&node)
                .into_iter()
                .map(|(d, r, _, _)| (d, r))
                .collect();
            let tables = node.source != Some(crate::NodeSource::Seed)
                && org_ingest::repair_table_pipe_links(&node.body).is_some();
            let mut wanted: Vec<WantedEdge> = rows
                .into_iter()
                .filter(|w| !declared.contains(&(w.dst.clone(), w.rel.clone())))
                .collect();
            if wanted.is_empty() && !tables {
                continue;
            }
            // Seed content is regenerated from the binary (ADR-104), never
            // edited; an undeclarable target has no link that could hold it.
            let before = wanted.len();
            if node.source != Some(crate::NodeSource::Seed) {
                wanted.retain(WantedEdge::is_declarable);
            } else {
                wanted.clear();
            }
            report.rows_left += before - wanted.len();
            if wanted.is_empty() && !tables {
                continue;
            }
            wanted.sort_by(|a, b| (&a.dst, &a.rel).cmp(&(&b.dst, &b.rel)));
            plan.push((node, wanted));
        }
        Ok(plan)
    }

    /// Declare `wanted` in `node`'s body and write it through the node write.
    fn repair_node(
        &self,
        planned: &Node,
        wanted: &[WantedEdge],
        report: &mut TypedRepairReport,
    ) -> Result<(), KbStoreError> {
        // Re-read just before the write: another process (the daemon, an
        // editor) may have written this node since the plan was made, and the
        // write below replaces the whole row. A node gone since is skipped.
        let Some(fresh) = self.get_node(&planned.id)? else {
            report.rows_left += wanted.len();
            return Ok(());
        };
        let node = &fresh;
        // @ai-caution: [kb-links] A CRDT-backed node is NEVER edited by the
        // repair. Every peer that opens a store runs this pass; on a shared
        // node each would insert the same text under its own client id, and
        // YATA keeps both inserts (a doubled `?rel=` or link line). The op
        // would also be unsigned, which a hub requiring signed content ops
        // refuses. Its typed rows are kept (the re-derivation never deletes
        // one) and counted in `rows_left`.
        if node.crdt_doc.is_some() {
            report.rows_left += wanted.len();
            report.crdt_nodes_skipped += 1;
            return Ok(());
        }
        // What the fresh body already declares is not declared again: a
        // concurrent pass (another process) may have repaired this node, and
        // declaring an edge twice appends a duplicate link line.
        let have: std::collections::HashSet<(String, String)> = Self::derived_links(node)
            .into_iter()
            .map(|(d, r, _, _)| (d, r))
            .collect();
        let wanted: Vec<WantedEdge> = wanted
            .iter()
            .filter(|w| !have.contains(&(w.dst.clone(), w.rel.clone())))
            .cloned()
            .collect();
        let wanted = wanted.as_slice();
        // Table links first: once rewritten they are links, which a wanted
        // edge to the same target can then be declared on.
        // The plan never holds a seed node (ADR-104: regenerated, not edited).
        let mut body = node.body.clone();
        let mut tables = 0;
        if let Some((b, n)) = org_ingest::repair_table_pipe_links(&body) {
            body = b;
            tables = n;
        }
        let declared = org_ingest::declare_edges(&body, wanted);
        if declared.is_none() {
            report.rows_left += wanted.len();
            if tables == 0 {
                return Ok(());
            }
        }
        let mut updated = node.clone();
        updated.body = declared.as_ref().map_or(body, |d| d.body.clone());
        self.insert_node_with_history(&updated, TYPED_REPAIR_SUMMARY)?;
        report.nodes_repaired += 1;
        report.table_links_repaired += tables;
        if let Some(d) = declared {
            report.rows_rewritten += d.rewritten;
            report.rows_appended += d.appended;
        }
        Ok(())
    }

    /// A consistent copy of this store's database file beside it, taken
    /// before the repair's first change; the copy an earlier, interrupted
    /// pass took is kept instead of taking another (it is the pre-repair
    /// state). `None` for an in-memory store. A store that is not one
    /// SQLite file (a sled directory being migrated) is refused: its sqlite
    /// copy is repaired when it is opened.
    fn backup_before_typed_repair(&self) -> Result<Option<PathBuf>, KbStoreError> {
        if self.path.as_os_str().is_empty() || self.path == Path::new(":memory:") {
            return Ok(None);
        }
        if !self.path.is_file() {
            return Err(KbStoreError::Storage(format!(
                "{} is not a SQLite file; typed links are repaired in its sqlite copy",
                self.path.display()
            )));
        }
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Scoped to this derivation version: a later version's repair takes
        // its own backup rather than reusing a months-old one.
        let prefix = format!(
            "{name}{BACKUP_INFIX}v{}-",
            super::rederive::DERIVATION_VERSION
        );
        let dir = backup_dir(&self.path);
        let earlier = std::fs::read_dir(dir)
            .map_err(|e| KbStoreError::Storage(e.to_string()))?
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().starts_with(&prefix));
        if let Some(e) = earlier {
            return Ok(Some(e.path()));
        }
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let dst = dir.join(format!("{prefix}{secs}"));
        // Written under a name the reuse check above never matches, and
        // renamed only once complete: a copy cut short (disk full, a crash)
        // is never taken for the pre-repair backup by a later pass, nor by a
        // concurrent one.
        let partial = dir.join(format!(".{name}.typed-links-backup.partial"));
        let _ = std::fs::remove_file(&partial);
        if let Err(e) = backup_file(&self.path, &partial) {
            let _ = std::fs::remove_file(&partial);
            return Err(e);
        }
        std::fs::rename(&partial, &dst).map_err(|e| KbStoreError::Storage(e.to_string()))?;
        Ok(Some(dst))
    }
}

/// The directory a store's backup goes in: its own. A bare relative file
/// name (`kb.sqlite`) has the parent `Some("")`, not `None`, and
/// `read_dir("")` fails with ENOENT -- which made the repair refuse every
/// store opened that way, and the version-gated pass with it.
fn backup_dir(store: &Path) -> &Path {
    match store.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

#[cfg(feature = "storage-sqlite")]
fn backup_file(src: &Path, dst: &Path) -> Result<(), KbStoreError> {
    crate::backup::snapshot(src, dst)
        .map_err(|e| KbStoreError::Storage(format!("backup before typed-link repair failed: {e}")))
}

#[cfg(not(feature = "storage-sqlite"))]
fn backup_file(_: &Path, _: &Path) -> Result<(), KbStoreError> {
    Err(KbStoreError::Storage(
        "no backup without the sqlite storage feature".into(),
    ))
}

#[cfg(test)]
mod backup_dir_tests {
    use super::backup_dir;
    use std::path::Path;

    #[test]
    fn a_bare_file_name_backs_up_into_the_current_directory() {
        assert_eq!(backup_dir(Path::new("kb.sqlite")), Path::new("."));
        // And that directory can be listed, which is what the repair does.
        assert!(std::fs::read_dir(backup_dir(Path::new("kb.sqlite"))).is_ok());
        assert_eq!(backup_dir(Path::new("a/kb.sqlite")), Path::new("a"));
        assert_eq!(backup_dir(Path::new("/srv/kb.sqlite")), Path::new("/srv"));
    }
}
