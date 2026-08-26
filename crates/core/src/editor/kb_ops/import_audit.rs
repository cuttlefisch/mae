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
