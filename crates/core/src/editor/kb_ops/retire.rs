//! Retiring a migrated KB's org archive — the step that finishes a cutover.
//!
//! Detaching a KB makes its store the source of truth but leaves the `.org`
//! files on disk, where they are read-only and permanently labelled "not the
//! KB". That is a *migrating* KB. Retiring the archive moves those files out
//! and clears the instance's `org_dir`, which makes it a **native** KB: every
//! guard goes quiet on its own because the files are gone and the directory is
//! empty, with no state to keep in sync with reality.
//!
//! @ai-caution: [kb-truth] The gate is the whole point. Moving files that the
//! store does NOT represent destroys the only copy. It is exact rather than
//! heuristic because `record_source_file` is only reached for a file ingest
//! actually parsed — a file with no `:ID:` anywhere is skipped BEFORE it, so
//! it is absent from `source_files` and shows up here as a blocker. That is
//! not hypothetical: a whole daily note sat invisible in a real primary KB
//! because of exactly that skip.

use super::super::Editor;
use mae_kb::KbStore;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// A reason one file cannot be retired.
#[derive(Debug, Clone)]
pub struct RetireBlocker {
    pub path: PathBuf,
    pub reason: String,
}

/// What retiring an archive would do, and what stops it.
#[derive(Debug, Clone)]
pub struct RetirePlan {
    pub kb: String,
    pub uuid: String,
    pub origin: PathBuf,
    /// Files verified as represented in the store — safe to move.
    pub files: Vec<PathBuf>,
    /// Files that are NOT safe to move, with why.
    pub blockers: Vec<RetireBlocker>,
    /// Set when the origin lives in a tracked git repo: retiring removes files
    /// someone else may rely on, so a plan says so out loud.
    pub git_remote: Option<String>,
}

impl RetirePlan {
    pub fn is_clean(&self) -> bool {
        self.blockers.is_empty()
    }

    /// A human-readable dry run.
    pub fn describe(&self) -> String {
        let mut out = format!(
            "Retire '{}' archive: {} file(s) would move out of {}\n",
            self.kb,
            self.files.len(),
            self.origin.display()
        );
        if let Some(remote) = &self.git_remote {
            out.push_str(&format!(
                "  NOTE: this is a tracked git repo ({remote}). The files will show as \
                 deletions in `git status`; mae does not stage, commit or push.\n"
            ));
        }
        if self.blockers.is_empty() {
            out.push_str("  every file is represented in the store — safe to retire\n");
        } else {
            out.push_str(&format!(
                "  REFUSED: {} file(s) are NOT represented in the store:\n",
                self.blockers.len()
            ));
            for b in self.blockers.iter().take(20) {
                out.push_str(&format!("    {} — {}\n", b.path.display(), b.reason));
            }
            if self.blockers.len() > 20 {
                out.push_str(&format!("    … and {} more\n", self.blockers.len() - 20));
            }
        }
        out
    }
}

fn hash_of(content: &str) -> String {
    hex::encode(Sha256::digest(content.as_bytes()))
}

/// Best-effort git remote for `dir`, read from `.git/config` rather than
/// shelling out.
/// The `origin` url in a `.git/config`, if any.
fn origin_url_in(config_text: &str) -> Option<String> {
    let mut in_origin = false;
    for line in config_text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_origin = t.starts_with("[remote \"origin\"]");
            continue;
        }
        if !in_origin {
            continue;
        }
        if let Some(url) = t.strip_prefix("url =").or_else(|| t.strip_prefix("url=")) {
            return Some(url.trim().to_string());
        }
    }
    None
}

fn git_remote_for(dir: &Path) -> Option<String> {
    let mut cur = Some(dir);
    while let Some(d) = cur {
        if let Ok(text) = std::fs::read_to_string(d.join(".git").join("config")) {
            return origin_url_in(&text);
        }
        cur = d.parent();
    }
    None
}

fn org_files_under(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n == ".git") {
                    continue;
                }
                walk(&p, out);
            } else if p.extension().and_then(|x| x.to_str()) == Some("org") {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}

/// Per-file verification, split out of `kb_retire_plan` for length and nesting.
///
/// Three questions, each catching a different way a file's content can fail to
/// be in the store — see this module's `@ai-caution` for why the first one is
/// not hypothetical.
fn verify_archive_files(
    store: &mae_kb::CozoKbStore,
    org_dir: &Path,
) -> (Vec<PathBuf>, Vec<RetireBlocker>) {
    let mut files = Vec::new();
    let mut blockers = Vec::new();
    for path in org_files_under(org_dir) {
        // MAE's own instance marker. Ingest skips it, so it is never in
        // `source_files` — without this every KB has one permanent blocker and
        // can never be retired. Found by the dry run against a real KB.
        if mae_kb::federation::is_instance_sentinel(&path) {
            continue;
        }
        let key = path.to_string_lossy().to_string();
        let Ok(Some(recorded)) = store.get_source_file_hash(&key) else {
            blockers.push(RetireBlocker {
                path,
                reason: "never imported — no node was ever created from it (missing :ID:?)".into(),
            });
            continue;
        };
        let Ok(content) = std::fs::read_to_string(&path) else {
            blockers.push(RetireBlocker {
                path,
                reason: "unreadable".into(),
            });
            continue;
        };
        if hash_of(&content) != recorded {
            blockers.push(RetireBlocker {
                path,
                reason: "modified since import — that edit never reached the store".into(),
            });
            continue;
        }
        let ids = store.get_source_file_node_ids(&key).unwrap_or_default();
        let missing: Vec<String> = ids
            .iter()
            .filter(|id| !matches!(store.get_node_light(id), Ok(Some(_))))
            .cloned()
            .collect();
        if missing.is_empty() {
            files.push(path);
        } else {
            blockers.push(RetireBlocker {
                path,
                reason: format!(
                    "its node(s) are gone from the store: {}",
                    missing.join(", ")
                ),
            });
        }
    }
    (files, blockers)
}

/// Copy each file, verify the copy, and only then unlink the original.
///
/// Split from `kb_retire_archive` for length. The copy-verify-unlink order is
/// deliberate and not merely tidy: an interrupted retirement leaves the source
/// in place rather than a half-moved archive, and it works across filesystems
/// where a rename would not.
fn move_verified(files: &[PathBuf], origin: &Path, dest_root: &Path) -> Result<usize, String> {
    let mut moved = 0usize;
    for src in files {
        let rel = src.strip_prefix(origin).unwrap_or(src);
        let dest = dest_root.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        std::fs::copy(src, &dest).map_err(|e| format!("copy {} failed: {e}", src.display()))?;
        let a = std::fs::read_to_string(src).map_err(|e| e.to_string())?;
        let b = std::fs::read_to_string(&dest).map_err(|e| e.to_string())?;
        if hash_of(&a) != hash_of(&b) {
            return Err(format!(
                "copy of {} did not verify — stopping with {moved} file(s) moved and the rest \
                 untouched",
                src.display()
            ));
        }
        std::fs::remove_file(src).map_err(|e| format!("remove {} failed: {e}", src.display()))?;
        moved += 1;
    }
    Ok(moved)
}

impl Editor {
    /// Verify what retiring `name`'s archive would move, and what blocks it.
    pub fn kb_retire_plan(&self, name: &str) -> Result<RetirePlan, String> {
        let inst = self
            .kb
            .registry
            .find(name)
            .ok_or_else(|| format!("No such KB: {name}"))?;
        if inst.ingest_policy.allows_ingest() {
            return Err(format!(
                "'{}' is still attached — its org directory is the source of truth. \
                 Detach it first with :kb-detach {}",
                inst.name, inst.name
            ));
        }
        if inst.org_dir.as_os_str().is_empty() {
            return Err(format!(
                "'{}' has no archive — it is already native",
                inst.name
            ));
        }
        let store = self
            .kb
            .instance_stores
            .get(&inst.uuid)
            .ok_or_else(|| format!("'{}' has no open store to verify against", inst.name))?;

        let (files, blockers) = verify_archive_files(store.as_ref(), &inst.org_dir);

        Ok(RetirePlan {
            kb: inst.name.clone(),
            uuid: inst.uuid.clone(),
            origin: inst.org_dir.clone(),
            files,
            blockers,
            git_remote: git_remote_for(&inst.org_dir),
        })
    }

    /// Move a verified archive out and make the KB native.
    ///
    /// Refuses unless the plan is clean. Copies, verifies the copy's hash, and
    /// only then unlinks the original — so an interrupted retirement leaves the
    /// source in place rather than a half-moved archive, and it works across
    /// filesystems where a rename would not.
    pub fn kb_retire_archive(&mut self, name: &str) -> Result<String, String> {
        let plan = self.kb_retire_plan(name)?;
        if !plan.is_clean() {
            return Err(format!(
                "{}\nNothing was moved. Resolve the above first — reattach and reimport, \
                 or capture the content into the KB.",
                plan.describe()
            ));
        }
        let data_dir = self
            .mae_data_dir()
            .ok_or("cannot determine the MAE data directory")?;
        let stamp = super::chrono_now();
        let dest_root = data_dir.join("retired").join(&plan.kb).join(&stamp);

        let moved = move_verified(&plan.files, &plan.origin, &dest_root)?;

        // Clearing `org_dir` is what makes the KB native: every guard already
        // tests `!org_dir.is_empty()`, so they all go quiet without a new
        // state flag that could drift from what is on disk.
        let uuid = plan.uuid.clone();
        let origin = plan.origin.clone();
        let remote = plan.git_remote.clone();
        let count = plan.files.len();
        let dest_for_record = dest_root.clone();
        let apply = move |reg: &mut mae_kb::federation::KbRegistry| -> bool {
            let Some(inst) = reg.instances.iter_mut().find(|i| i.uuid == uuid) else {
                return false;
            };
            let rec = inst
                .import_record
                .get_or_insert_with(mae_kb::federation::KbImportRecord::default);
            if rec.origin.as_os_str().is_empty() {
                rec.origin = origin.clone();
            }
            if rec.file_count == 0 {
                rec.file_count = count;
            }
            if rec.git_remote.is_none() {
                rec.git_remote = remote.clone();
            }
            rec.retired_at = Some(stamp.clone());
            rec.retired_to = Some(dest_for_record.clone());
            inst.org_dir = PathBuf::new();
            true
        };
        if let Some(dir) = self.mae_data_dir() {
            let (registry, _changed, saved) = mae_kb::federation::KbRegistry::update(&dir, apply);
            if let Err(e) = saved {
                return Err(format!(
                    "moved {moved} file(s) but could not persist the registry: {e}"
                ));
            }
            self.kb.registry = registry;
            self.kb.last_local_registry_write = Some(std::time::Instant::now());
        }

        Ok(format!(
            "'{}' is now native: {moved} file(s) moved to {}. Its store is the only copy — \
             the archive is recoverable there until you delete it.",
            plan.kb,
            dest_root.display()
        ))
    }
}
