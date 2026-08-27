//! Story D / R8 — the operator-facing half of the import audit.
//!
//! Three surfaces, one per layer R8 specifies that MAE did not have:
//!
//! * `:kb-import-plan <dir>` — the **pre-flight assessment**, persisted, and
//!   binding on the import that follows it.
//! * `:kb-import-verify <name>` — the **repeatable reconciliation pass**, which
//!   mutates nothing and can be re-run at any time.
//! * the loss report node, written **into the destination** by `kb_reimport`.
//!
//! The counters are deliberately not the deliverable here. `skipped` and
//! `failed` both read **zero** through obsidian-importer#547's 748-note loss;
//! what these surfaces report is the *source* and the *destination*, reconciled.

use std::path::{Path, PathBuf};

use mae_kb::import_plan::{ImportPlan, PlanDrift};
use mae_kb::KbStore;

use super::Editor;

impl Editor {
    /// Where a plan for `org_dir` is persisted, so `verify_source_unchanged`
    /// has an artifact to bind to on the next run.
    ///
    /// Keyed by a hash of the canonicalized source path — two different corpora
    /// must not share one plan file, and the same corpus reached by a symlink
    /// must not mint a second (`realpath` before hashing anything path-shaped,
    /// per the identity work in [`mae_kb::project_identity`]).
    pub fn kb_import_plan_path(&self, org_dir: &Path) -> Option<PathBuf> {
        let data_dir = self.mae_data_dir()?;
        let canonical = std::fs::canonicalize(org_dir).unwrap_or_else(|_| org_dir.to_path_buf());
        let key = mae_kb::import_plan::plan_key(&canonical.to_string_lossy());
        Some(data_dir.join("import-plans").join(format!("{key}.json")))
    }

    /// `:kb-import-plan <dir>` — assess without importing, and persist the plan.
    pub fn kb_import_plan(&mut self, org_dir: &str) -> Result<String, String> {
        let dir = Path::new(org_dir);
        if !dir.is_dir() {
            return Err(format!("not a directory: {org_dir}"));
        }
        // Assessing a DETACHED KB's directory reads a frozen archive as though
        // it were a live source. Every "would import" line is then a claim
        // about content the store already holds and no ingest will ever read
        // again.
        // A DIRECTORY-level question, so not `kb_stale_archive_instance` —
        // that one asks whether a specific FILE was imported, and a directory
        // never is.
        let archived = self
            .kb
            .registry
            .instances
            .iter()
            .find(|i| {
                !i.ingest_policy.allows_ingest()
                    && !i.org_dir.as_os_str().is_empty()
                    && (dir.starts_with(&i.org_dir) || i.org_dir.starts_with(dir))
            })
            .map(|i| i.name.clone());
        if let Some(kb) = archived {
            return Err(format!(
                "{} is the archived source of KB '{kb}', which is detached — assessing it \
                 would describe an import that can never happen. To check the store really \
                 holds every file, use :kb-retire-archive {kb} (a dry run by default).",
                dir.display()
            ));
        }
        let plan = ImportPlan::assess(dir);
        let saved = match self.kb_import_plan_path(dir) {
            Some(path) => match plan.save(&path) {
                Ok(()) => format!(" — plan saved to {}", path.display()),
                Err(e) => format!(" — plan NOT saved ({e})"),
            },
            None => " — plan NOT saved (no data dir)".to_string(),
        };
        Ok(format!(
            "{}{}\n{}",
            plan.summary(),
            saved,
            hazard_block(&plan)
        ))
    }

    /// Refuse an import whose corpus moved since the plan the operator read.
    ///
    /// Terraform's `plan -out` / `apply` contract: a preview the apply step may
    /// diverge from tells the operator nothing they can rely on. Returns `Ok`
    /// when there is no saved plan — this binds an operator who *asked* for a
    /// plan, and does not invent a gate for one who did not.
    pub fn kb_import_plan_drift(&self, org_dir: &Path) -> Option<Vec<PlanDrift>> {
        let path = self.kb_import_plan_path(org_dir)?;
        let plan = ImportPlan::load(&path).ok()?;
        plan.verify_source_unchanged().err()
    }

    /// `:kb-import-verify <name>` — re-reconcile a registered KB's org
    /// directory against what the store actually holds.
    ///
    /// Reads both sides and writes neither, so it is safe to run at any time —
    /// which is the point: R8's reconciliation is a *repeatable* pass, not a
    /// one-shot printed during the import and then gone.
    pub fn kb_import_verify(&mut self, name_or_uuid: &str) -> Result<String, String> {
        let instance = self
            .kb
            .registry
            .find(name_or_uuid)
            .cloned()
            .ok_or_else(|| format!("no KB instance matching '{name_or_uuid}'"))?;
        if instance.org_dir.as_os_str().is_empty() {
            return Err(format!(
                "'{}' has no org directory — its content lives in the store, so there is \
                 nothing to reconcile against",
                instance.name
            ));
        }
        // A detached instance still HAS an org dir, but it is a frozen archive.
        // Reconciling against it reports the divergence that detaching created
        // as though it were loss — "source-only" for a node deleted in the
        // store, "differ" for one edited there. Both are correct and expected.
        if !instance.ingest_policy.allows_ingest() {
            return Err(format!(
                "'{}' is detached: its org directory is a frozen archive, so reconciling \
                 against it reports expected post-detach divergence as loss. Use \
                 :kb-retire-archive {} to check the store holds every file (dry run by \
                 default).",
                instance.name, instance.name
            ));
        }
        let ids = self.kb_known_ids(&instance.uuid);
        let has = |id: &str| ids.contains(id);
        let report = mae_kb::import_plan::reconcile(&instance.org_dir, &has);
        Ok(format!(
            "'{}': {}{}",
            instance.name,
            report.summary(),
            if report.is_clean() {
                String::new()
            } else {
                format!("\n{}", loss_lines(&report))
            }
        ))
    }

    /// Every node id the destination holds for this instance.
    fn kb_known_ids(&self, uuid: &str) -> std::collections::HashSet<String> {
        if let Some(store) = self.kb.instance_stores.get(uuid) {
            if let Ok(ids) = store.list_ids(None) {
                return ids.into_iter().collect();
            }
        }
        self.kb
            .instances
            .get(uuid)
            .map(|kb| kb.list_ids(None).into_iter().collect())
            .unwrap_or_default()
    }
}

fn hazard_block(plan: &ImportPlan) -> String {
    if plan.hazards.is_empty() {
        return "no hazards".to_string();
    }
    plan.hazards
        .iter()
        .map(|h| format!("  ! {}", h.describe()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn loss_lines(report: &mae_kb::import_plan::ReconcileReport) -> String {
    use mae_kb::import_plan::Reconciliation as R;
    report
        .rows
        .iter()
        .filter_map(|row| match row {
            R::Match { .. } => None,
            R::SourceOnly { path, node_ids } => Some(format!(
                "  source-only: {} ({} node(s) absent from the store)",
                path.display(),
                node_ids.len()
            )),
            R::Differ { path, missing, .. } => Some(format!(
                "  differ: {} ({} node(s) missing)",
                path.display(),
                missing.len()
            )),
            R::Error { path, reason } => Some(format!("  error: {} ({reason})", path.display())),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl Editor {
    /// Write the per-item loss report **into the destination KB** (Story D / R8).
    ///
    /// A durable, queryable artifact — reachable by `kb_search`/`kb_get` and by
    /// the agent — rather than a toast the operator may not have been looking at.
    /// Obsidian's importer writes a report note into the vault; MAE's destination
    /// is a KB, so the MAE-native form is a node.
    ///
    /// **Cost, stated rather than hidden:** with no saved plan this re-walks and
    /// re-parses the corpus to recover the per-file skip/fail reasons the
    /// `ImportReport` counters do not carry. That doubles the parse on an
    /// explicit `:kb-reimport`. It is not on the daemon's ingest tick, which
    /// calls `import_org_dir_to_store` directly. The alternative — write the
    /// report only when something looks wrong — is precisely the design that
    /// makes "nothing was lost" indistinguishable from "nothing was checked".
    pub(super) fn kb_write_loss_report(
        &mut self,
        instance: &mae_kb::federation::KbInstance,
        report: &mae_kb::federation::ImportReport,
    ) {
        let plan = self
            .kb_import_plan_path(&instance.org_dir)
            .and_then(|p| ImportPlan::load(&p).ok())
            .filter(|p| p.verify_source_unchanged().is_ok())
            .unwrap_or_else(|| ImportPlan::assess(&instance.org_dir));

        let node = mae_kb::import_plan::loss_report_node(&plan, report);
        if let Some(store) = self.kb.instance_stores.get(&instance.uuid) {
            if let Err(e) = store.update_node(&node) {
                tracing::warn!(error = %e, "could not persist the import loss report");
            }
        }
        if let Some(kb) = self.kb.instances.get_mut(&instance.uuid) {
            kb.insert(node);
        }
    }
}

impl Editor {
    /// Ex-command arms for the import-audit surfaces.
    ///
    /// Extracted from the ex dispatcher: that function is already ~1,270 lines
    /// against an 80-line ceiling, and the structural gate is per-function so
    /// that the remedy for touching it is local rather than architectural.
    pub(crate) fn dispatch_kb_import_audit(&mut self, command: &str, args: Option<&str>) {
        let arg = args.map(str::trim).filter(|s| !s.is_empty());
        match (command, arg) {
            ("kb-import-plan", None) => self.set_status("Usage: :kb-import-plan <directory>"),
            ("kb-import-plan", Some(dir)) => match self.kb_import_plan(dir) {
                Ok(msg) | Err(msg) => self.set_status(msg),
            },
            ("kb-import-verify", None) => self.set_status("Usage: :kb-import-verify <name>"),
            ("kb-import-verify", Some(name)) => match self.kb_import_verify(name) {
                Ok(msg) | Err(msg) => self.set_status(msg),
            },
            _ => {}
        }
    }

    /// `:kb-detach` / `:kb-attach` — flip which side of a KB is authoritative
    /// (KB cutover, Phase 1). Extracted alongside the arms above for the same
    /// reason.
    pub(crate) fn dispatch_kb_ingest_policy(&mut self, command: &str, args: Option<&str>) {
        let policy = if command == "kb-detach" {
            mae_kb::federation::IngestPolicy::StoreIsTruth
        } else {
            mae_kb::federation::IngestPolicy::FromOrgDir
        };
        match args.map(str::trim).filter(|s| !s.is_empty()) {
            None => self.set_status(format!("Usage: :{command} <name|primary>")),
            Some(name) => match self.kb_set_ingest_policy(name, policy) {
                Ok(msg) => self.set_status(msg),
                Err(e) => self.set_status(e),
            },
        }
    }
}

impl Editor {
    /// Refuse an import whose corpus moved since the plan the operator read.
    ///
    /// **Terraform's `plan -out`/`apply` contract.** A preview the apply step may
    /// diverge from tells the operator nothing they can rely on, which is why
    /// every notes-app "dry run" surveyed is *scoping* rather than prediction.
    ///
    /// Binds only an operator who actually asked for a plan: with no saved plan
    /// this returns `false` and the import proceeds exactly as before. Inventing
    /// the gate for someone who never planned would break every existing import,
    /// which is how a safety mechanism gets disabled wholesale.
    pub(super) fn kb_refuse_stale_plan(
        &mut self,
        instance: &mae_kb::federation::KbInstance,
    ) -> bool {
        let Some(drift) = self.kb_import_plan_drift(&instance.org_dir) else {
            return false;
        };
        self.set_status(format!(
            "'{}': the corpus changed since the saved import plan — {} difference(s), \
             starting with: {}. Re-run :kb-import-plan {} to review and re-approve.",
            instance.name,
            drift.len(),
            drift.first().map(|d| d.describe()).unwrap_or_default(),
            instance.org_dir.display()
        ));
        true
    }
}

impl Editor {
    /// Adopt an already-registered project KB for `canonical_root`, repairing a
    /// stale `project_root` when the project has moved (Story B / R11).
    ///
    /// Returns `None` when no instance matches — the caller then provisions one.
    pub(super) fn kb_adopt_project(
        &mut self,
        canonical_root: &Path,
        key: Option<&str>,
    ) -> Option<super::KbImportResult> {
        let (uuid, repaired) = self.kb.registry.adopt_moved_project(canonical_root, key)?;
        if repaired {
            if let Some(data_dir) = self.mae_data_dir() {
                let root = canonical_root.to_path_buf();
                let uuid_for_write = uuid.clone();
                let (registry, (), saved) =
                    mae_kb::federation::KbRegistry::update(&data_dir, |reg| {
                        if let Some(i) = reg.instances.iter_mut().find(|i| i.uuid == uuid_for_write)
                        {
                            i.project_root = Some(root.clone());
                        }
                    });
                if let Err(e) = saved {
                    tracing::warn!(error = %e, "could not persist the repaired project root");
                }
                self.kb.registry = registry;
            }
        }
        let name = self
            .kb
            .registry
            .instances
            .iter()
            .find(|i| i.uuid == uuid)
            .map(|i| i.name.clone())
            .unwrap_or_default();
        Some(super::KbImportResult {
            name,
            uuid,
            report: Default::default(),
            health: Default::default(),
        })
    }

    /// `:kb-relink` — re-mint this project's KB identity and re-point the
    /// registered instance at it.
    ///
    /// Git itself ships `git worktree repair` for the same reason: a
    /// path-independent identity is unachievable in general, so a repair verb is
    /// not a defeat — it is what every system in this space needed and most
    /// lacked, leaving users to delete state and start over.
    pub fn kb_relink_project(&mut self, root: Option<PathBuf>) -> Result<String, String> {
        let root = root
            .or_else(|| self.active_project_root().map(|p| p.to_path_buf()))
            .ok_or_else(|| "No project root detected".to_string())?;
        let canonical = root
            .canonicalize()
            .map_err(|e| format!("cannot resolve {}: {e}", root.display()))?;
        let identity = mae_kb::project_identity::relink(&canonical)
            .map_err(|e| format!("could not re-mint an identity: {e:?}"))?;
        let key = identity.key();

        let Some(uuid) = self
            .kb
            .registry
            .instances
            .iter()
            .find(|i| i.project_root.as_deref() == Some(canonical.as_path()))
            .map(|i| i.uuid.clone())
        else {
            return Err(format!(
                "no project KB registered for {} — run :kb-init-project first",
                canonical.display()
            ));
        };
        let data_dir = self
            .mae_data_dir()
            .ok_or_else(|| "cannot determine data directory".to_string())?;
        let key_for_write = key.clone();
        let (registry, (), saved) = mae_kb::federation::KbRegistry::update(&data_dir, |reg| {
            if let Some(i) = reg.instances.iter_mut().find(|i| i.uuid == uuid) {
                i.project_key = Some(key_for_write.clone());
            }
        });
        saved.map_err(|e| format!("could not persist the relink: {e}"))?;
        self.kb.registry = registry;
        Ok(format!(
            "relinked {} to {}{}",
            canonical.display(),
            key,
            if identity.is_stable() {
                ""
            } else {
                " (path fallback — not a git repo, so this does NOT survive a move)"
            }
        ))
    }
}

impl Editor {
    /// `:kb-relink [dir]` — the ex-command arm, extracted so the ex dispatcher
    /// does not grow (it is ~1,270 lines against an 80-line ceiling).
    pub(crate) fn dispatch_kb_relink(&mut self, args: Option<&str>) {
        let root = args
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        match self.kb_relink_project(root) {
            Ok(msg) | Err(msg) => self.set_status(msg),
        }
    }
}

impl Editor {
    /// `:kb-ingest <dir>` — index an org directory into the primary KB.
    ///
    /// Extracted from the ex dispatcher, which is ~1,270 lines against an
    /// 80-line ceiling.
    pub(crate) fn dispatch_kb_ingest(&mut self, args: Option<&str>) {
        let Some(dir) = args.map(str::trim).filter(|s| !s.is_empty()) else {
            self.set_status("Usage: :kb-ingest <directory>");
            return;
        };
        // KB cutover, Phase 1: the explicit-intent twin of `:kb-reimport`, which
        // is already refused for a detached primary. Refusing here too keeps the
        // two consistent — otherwise the safer-looking command is the one that
        // overwrites the store.
        if self.kb.primary_store_is_truth() {
            self.set_status(
                "the primary KB is detached — its store is the source of truth, so \
                 ingesting an org directory over it would overwrite it (re-attach \
                 with :kb-attach to allow ingest)",
            );
            return;
        }
        // Expand a leading `~` to $HOME — parity with `kb-register`/`kb-reimport`,
        // which expand tilde before touching the filesystem. Without this,
        // `:kb-ingest ~/Notes` reads a literal `~/Notes` (never exists) and
        // silently indexes 0 files.
        let dir = crate::file_picker::expand_tilde(dir);
        let report = self.kb.primary.ingest_org_dir(&dir);
        // `ingest_org_dir` only fills the in-memory mirror; write the nodes
        // through to the durable store so the import survives a restart
        // (daemon-less primary — nothing else snapshots it).
        let persisted = self.kb_persist_ingested(&report.ingested_ids);
        self.set_status(format!(
            "kb: indexed {}, persisted {}, skipped {} (no :ID:), errors {}",
            report.indexed,
            persisted,
            report.skipped_no_id,
            report.read_errors.len()
        ));
    }
}
