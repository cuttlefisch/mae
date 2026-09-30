//! `:kb-attach` — the one cutover transition that REMOVES a protection (#825).
//!
//! `:kb-detach` makes a KB's store the source of truth; `:kb-attach` hands that
//! authority back to an `.org` directory, after which ingest may overwrite the
//! store. The two were routed to one handler and treated as mirror images. They
//! are not: detaching adds a protection, attaching removes one, and a KB is
//! typically detached *because* its directory stopped being maintained — so the
//! longer it stays detached, the more destructive a blind re-attach becomes.
//!
//! On a real machine the directory of a detached KB held ~100 hollow
//! `Previous:`/`Next:` stubs (written by mae's own dailies — see `dir_owner.rs`)
//! carrying the SAME ids as ~200 real nodes in the store. An attach followed by
//! any ingest would have replaced the real notes with the stubs.
//!
//! So attach is modelled on `:kb-retire-archive`, the other one-way step: it
//! compares what an ingest WOULD produce with what the store holds, refuses on
//! any difference by default, and proceeds over one only on an explicit
//! `confirm`.
//!
//! @ai-caution: [kb-truth] The comparison must be on CONTENT. The first design
//! reused `kb_import_verify`, which asks only whether each file's `:ID:` exists
//! in the store — a stub carrying the right id "matches" the real note it would
//! destroy. That is the exact false assurance #825 is about.

use std::path::{Path, PathBuf};

use super::*;
use mae_kb::KbStore;

/// How many ids of each kind a refusal lists before summarising.
const SHOWN: usize = 5;

/// What attaching `dir` would do to the store, computed by parsing the
/// directory exactly as ingest does and comparing node by node.
#[derive(Debug, Default)]
pub struct AttachDivergence {
    pub dir: PathBuf,
    /// Nodes the directory would produce.
    pub parsed: usize,
    /// Nodes the store holds now.
    pub stored: usize,
    /// In the store, produced by no file: an ingest from the directory no longer
    /// represents them, and a full ingest removes them.
    pub store_only: Vec<String>,
    /// In both, with different title or body: ingest overwrites the store's copy.
    /// `(id, store body chars, file body chars)` — a large drop flags a stub.
    pub changed: Vec<(String, usize, usize)>,
    /// Produced by a file, absent from the store: ingest adds them. Reported, not
    /// blocking — an addition destroys nothing.
    pub file_only: Vec<String>,
}

impl AttachDivergence {
    /// Nothing in the store would be lost or overwritten.
    pub fn is_clean(&self) -> bool {
        self.store_only.is_empty() && self.changed.is_empty()
    }

    fn list(ids: impl Iterator<Item = String>, total: usize) -> String {
        let shown: Vec<String> = ids.take(SHOWN).collect();
        let more = total.saturating_sub(shown.len());
        if more > 0 {
            format!("{} (+{more} more)", shown.join(", "))
        } else {
            shown.join(", ")
        }
    }

    pub fn describe(&self) -> String {
        let mut out = format!(
            "{} holds {} node(s); the store holds {}.",
            self.dir.display(),
            self.parsed,
            self.stored
        );
        if !self.changed.is_empty() {
            let hollow = self
                .changed
                .iter()
                .filter(|(_, store, file)| *file * 4 < *store)
                .count();
            out.push_str(&format!(
                "\n  {} node(s) would be OVERWRITTEN by different file content{}: {}",
                self.changed.len(),
                if hollow > 0 {
                    format!(" ({hollow} of them by a file under a quarter the size — likely stubs)")
                } else {
                    String::new()
                },
                Self::list(
                    self.changed
                        .iter()
                        .map(|(id, s, f)| format!("{id} ({s}→{f} chars)")),
                    self.changed.len()
                )
            ));
        }
        if !self.store_only.is_empty() {
            out.push_str(&format!(
                "\n  {} node(s) exist only in the store and no file would represent them: {}",
                self.store_only.len(),
                Self::list(self.store_only.iter().cloned(), self.store_only.len())
            ));
        }
        if !self.file_only.is_empty() {
            out.push_str(&format!(
                "\n  {} node(s) would be added from files: {}",
                self.file_only.len(),
                Self::list(self.file_only.iter().cloned(), self.file_only.len())
            ));
        }
        out
    }
}

/// A node body minus its leading metadata — `:PROPERTIES:` drawers and
/// file-level `#+KEYWORD:` lines — trimmed.
///
/// The store keeps that header in the body text while a fresh parse lifts it
/// out (#655: properties live in two places with no reconciliation; the title
/// and filetags likewise). Measured against a real 151-node corpus in which
/// git showed exactly THREE files with content edits, comparing raw bodies
/// flagged 143 nodes as "overwritten". A check that refuses everything
/// protects nothing — it gets confirmed past, reflexively. The header is
/// metadata; the prose after it is what an attach can destroy.
pub(crate) fn content_of(body: &str) -> &str {
    let mut t = body.trim_start();
    loop {
        if t.starts_with(":PROPERTIES:") {
            match t.find("\n:END:") {
                Some(end) => t = t[end + "\n:END:".len()..].trim_start(),
                None => break,
            }
        } else if t.starts_with("#+") {
            t = t.split_once('\n').map_or("", |(_, rest)| rest).trim_start();
        } else {
            break;
        }
    }
    t.trim_end()
}

/// Compare what ingesting `dir` would produce with what `store` holds.
///
/// Parses with the same importer ingest uses, in memory only — nothing is
/// written anywhere. Title and body are compared with trailing whitespace
/// ignored, since that is not content.
pub fn attach_divergence(dir: &Path, store: &dyn KbStore) -> Result<AttachDivergence, String> {
    let (parsed, _report, _health) = mae_kb::federation::import_org_dir(dir);
    let stored = store
        .load_all()
        .map_err(|e| format!("could not read the store to compare against: {e}"))?;
    let stored: std::collections::HashMap<&str, &mae_kb::Node> =
        stored.iter().map(|n| (n.id.as_str(), n)).collect();

    let same = |a: &str, b: &str| content_of(a) == content_of(b);
    let mut div = AttachDivergence {
        dir: dir.to_path_buf(),
        parsed: parsed.iter().count(),
        stored: stored.len(),
        ..Default::default()
    };
    for (id, file_node) in parsed.iter() {
        match stored.get(id.as_str()) {
            None => div.file_only.push(id.clone()),
            Some(store_node) => {
                if !same(&store_node.title, &file_node.title)
                    || !same(&store_node.body, &file_node.body)
                {
                    div.changed.push((
                        id.clone(),
                        store_node.body.chars().count(),
                        file_node.body.chars().count(),
                    ));
                }
            }
        }
    }
    let parsed_ids: std::collections::HashSet<&str> =
        parsed.node_ids().map(String::as_str).collect();
    div.store_only = stored
        .keys()
        .filter(|id| !parsed_ids.contains(*id))
        .map(|id| id.to_string())
        .collect();
    // Deterministic output: a refusal message must not reorder between runs.
    div.store_only.sort();
    div.changed.sort();
    div.file_only.sort();
    Ok(div)
}

impl Editor {
    /// `:kb-attach <name> [confirm]` — make a detached or retired KB's `.org`
    /// directory authoritative again, only when doing so loses nothing, or when
    /// the caller explicitly says `confirm`.
    ///
    /// Two starting states (ADR-110):
    /// - **migrating** — detached, `org_dir` still set: attach to it.
    /// - **native** — retired, `org_dir` cleared: attach to the recorded
    ///   `import_record.origin`, restoring `org_dir`. ADR-110 as written had no
    ///   native→attached transition; a git-reviewed corpus that must return to
    ///   files-are-truth needs one (recorded as an amendment there).
    pub fn kb_attach(&mut self, name_or_uuid: &str, confirm: bool) -> Result<String, String> {
        if let Some(name) = self.kb_is_system(name_or_uuid) {
            return Err(format!(
                "'{name}' is a MAE system KB — its content comes from the binary, \
                 not from an org directory, so there is nothing to attach to"
            ));
        }
        if mae_kb::kb_identity::PRIMARY_NAME_ALIASES
            .iter()
            .any(|a| name_or_uuid.eq_ignore_ascii_case(a))
        {
            // The primary has no single org directory to compare against, so
            // there is no verification to run — only an explicit decision.
            if !confirm {
                return Err(format!(
                    "Attaching the primary KB cannot be verified (it has no single org \
                     directory to compare with its store). Run `:kb-attach {name_or_uuid} \
                     confirm` if ingest may overwrite it."
                ));
            }
            return self.kb_set_ingest_policy_unverified(
                name_or_uuid,
                mae_kb::federation::IngestPolicy::FromOrgDir,
            );
        }

        let inst = self
            .kb
            .registry
            .find(name_or_uuid)
            .cloned()
            .ok_or_else(|| format!("No such KB: {name_or_uuid}"))?;
        if inst.ingest_policy.allows_ingest() {
            return Ok(format!("'{}' is already attached", inst.name));
        }
        let (dir, from_origin) = self.kb_attach_source(&inst)?;
        let store = self
            .kb
            .instance_stores
            .get(&inst.uuid)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "'{}': its store is not loaded, so there is nothing to verify the \
                     directory against — refusing rather than attaching blind",
                    inst.name
                )
            })?;
        let div = attach_divergence(&dir, store.as_ref())?;
        if !div.is_clean() && !confirm {
            return Err(format!(
                "Refusing to attach '{}': ingest from its org directory would change the \
                 store.\n{}\nIf the directory is what you want, run `:kb-attach {} confirm`. \
                 Back up the store first — the overwritten content is not recoverable from \
                 the directory.",
                inst.name,
                div.describe(),
                inst.name
            ));
        }

        self.kb_apply_attach(&inst.uuid, from_origin.then_some(&dir));
        let mut msg = format!(
            "'{}' attached to {} — the directory is now authoritative. Run :kb-reimport {} \
             to ingest it now.",
            inst.name,
            dir.display(),
            inst.name
        );
        if !div.is_clean() {
            msg.push_str(&format!(" Confirmed over: {}", div.describe()));
        }
        Ok(msg)
    }

    /// Which directory an attach would make authoritative, and whether it is a
    /// retired origin (so `org_dir` must be restored).
    fn kb_attach_source(
        &self,
        inst: &mae_kb::federation::KbInstance,
    ) -> Result<(PathBuf, bool), String> {
        if !inst.org_dir.as_os_str().is_empty() {
            return Ok((inst.org_dir.clone(), false));
        }
        let origin = inst
            .import_record
            .as_ref()
            .map(|r| r.origin.clone())
            .filter(|o| !o.as_os_str().is_empty())
            .ok_or_else(|| {
                format!(
                    "'{}' is native with no recorded origin — there is no org directory \
                     to attach to. It was created in mae, and its store is its only form.",
                    inst.name
                )
            })?;
        if !origin.is_dir() {
            return Err(format!(
                "'{}' was retired from {}, which no longer exists — nothing to attach to",
                inst.name,
                origin.display()
            ));
        }
        Ok((origin, true))
    }

    /// Persist the attach: policy to `FromOrgDir`, and `org_dir` restored when
    /// re-attaching a retired KB to its origin.
    fn kb_apply_attach(&mut self, uuid: &str, restore_org_dir: Option<&PathBuf>) {
        let apply = |reg: &mut mae_kb::federation::KbRegistry| {
            if let Some(i) = reg.instances.iter_mut().find(|i| i.uuid == uuid) {
                if let Some(dir) = restore_org_dir {
                    i.org_dir = dir.clone();
                }
                i.ingest_policy = mae_kb::federation::IngestPolicy::FromOrgDir;
            }
        };
        if let Some(data_dir) = self.mae_data_dir() {
            let (registry, (), saved) = mae_kb::federation::KbRegistry::update(&data_dir, apply);
            if let Err(e) = saved {
                tracing::warn!(error = %e, "failed to persist KB registry");
            }
            self.kb.registry = registry;
            self.kb.last_local_registry_write = Some(std::time::Instant::now());
        } else {
            apply(&mut self.kb.registry);
        }
    }
}
