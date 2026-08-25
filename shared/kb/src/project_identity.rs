//! Stable identity for a project-scoped KB (ADR-058 / Story B, design per R11).
//!
//! # Why an opaque minted id, and not a derived one
//!
//! Every system surveyed keys per-project state on something *derived* — a path,
//! a root commit, a remote URL — and every one of them has open bugs from it.
//!
//! **VS Code's `workspaceStorage` is the pattern to avoid**: MD5 of the **raw,
//! un-canonicalized** path, salted with the inode/birthtime, under a source
//! comment reading `DO NOT CHANGE. IDENTIFIERS HAVE TO REMAIN STABLE`. A fresh
//! clone, a `cp -r`, a restore-from-backup or a container rebuild each mint a new
//! identity for the same project. **That banner is the tell**: once state is
//! keyed on a derived value, the derivation can never be fixed.
//!
//! Path-only keying was also a *security* bug, not merely a correctness one:
//! connect to two remotes with identically-named folders, trust one, and the
//! other is trusted automatically. VS Code and Zed independently converged on
//! putting the **authority** in the key.
//!
//! **The root commit fails on MAE's own repository**, measured rather than
//! assumed: `git rev-parse --is-shallow-repository` returns `true` here, and Nx —
//! the only shipped implementation of root-commit identity — refuses to identify
//! a shallow repo at all. Worse, in a shallow clone `--max-parents=0 HEAD`
//! returns the **tip commit**: a well-formed 40-hex SHA that is simply wrong,
//! with no error.
//!
//! # The ladder
//!
//! 1. `git config --local mae.kb-id` — an opaque UUID, minted once.
//! 2. No git, or an unwritable `.git` — a `realpath`-canonicalized root path.
//! 3. Neither available — **refuse**, and let the caller ask.
//!
//! Tier 1 survives rename, move, symlink, cloud-sync redirection, container
//! rebuild, shallow clone, history rewrite, fork and org rename, because it
//! derives from none of them. It lives in `$GIT_COMMON_DIR/config`, so it is
//! **shared across every worktree of a clone** and **not copied by `git clone`** —
//! the latter being correct, not a gap: the KB is local-only, so a teammate's
//! clone should start without one, exactly as git-annex mints a fresh UUID per
//! clone.

use std::path::{Path, PathBuf};

/// How a project's KB identity was resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectIdentity {
    /// Tier 1: an opaque id minted into `git config --local mae.kb-id`.
    Minted(String),
    /// Tier 2: no git (or an unwritable one) — the canonicalized root path.
    ///
    /// Weaker by construction: it does not survive a move or a rename. Callers
    /// should surface it as such rather than treating it as equivalent.
    PathFallback(PathBuf),
}

impl ProjectIdentity {
    /// The key to store against.
    pub fn key(&self) -> String {
        match self {
            ProjectIdentity::Minted(id) => format!("kbid:{id}"),
            ProjectIdentity::PathFallback(p) => format!("path:{}", p.display()),
        }
    }

    /// Whether this identity survives the project being moved or renamed.
    pub fn is_stable(&self) -> bool {
        matches!(self, ProjectIdentity::Minted(_))
    }
}

/// Why identity could not be resolved. **A first-class outcome, not an error to
/// swallow**: silently binding the wrong KB to a project is the failure that
/// matters, so "I cannot identify this" must be sayable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    /// The path does not exist, or could not be canonicalized.
    UnresolvablePath(String),
    /// A git repo whose config could not be written, and no usable fallback.
    Unwritable(String),
}

impl std::fmt::Display for IdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdentityError::UnresolvablePath(p) => {
                write!(f, "cannot resolve project path '{p}'")
            }
            IdentityError::Unwritable(p) => write!(
                f,
                "'{p}' is a git repository whose config cannot be written, so no \
                 stable KB id can be minted"
            ),
        }
    }
}

/// Read `mae.kb-id` from the repo's local config, if this is a git repo.
fn read_minted_id(root: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["config", "--local", "--get", "mae.kb-id"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!id.is_empty()).then_some(id)
}

/// Mint and persist a fresh id. `None` if the config could not be written.
fn mint_id(root: &Path) -> Option<String> {
    // The same v4 minter `KbInstance`/`collab_id` already use (ADR-105 D4) --
    // no new dependency, and one uuid implementation in the crate.
    let id = crate::federation::generate_uuid();
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["config", "--local", "mae.kb-id", &id])
        .output()
        .ok()?;
    out.status.success().then_some(id)
}

/// Is `root` inside a git working tree?
fn is_git_repo(root: &Path) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success() && o.stdout.starts_with(b"true"))
        .unwrap_or(false)
}

/// Resolve (or mint) the KB identity for the project rooted at `root`.
///
/// **`root` is canonicalized first, always.** R11's standing rule: never hash or
/// compare a path that has not been through `realpath`, or two spellings of one
/// directory silently fail to match — and, the sibling risk, two different
/// directories silently collide.
pub fn resolve(root: &Path) -> Result<ProjectIdentity, IdentityError> {
    let canonical = root
        .canonicalize()
        .map_err(|_| IdentityError::UnresolvablePath(root.display().to_string()))?;

    if is_git_repo(&canonical) {
        if let Some(id) = read_minted_id(&canonical) {
            return Ok(ProjectIdentity::Minted(id));
        }
        if let Some(id) = mint_id(&canonical) {
            return Ok(ProjectIdentity::Minted(id));
        }
        // A git repo we cannot write to: fall back rather than fail, but say so.
        return Ok(ProjectIdentity::PathFallback(canonical));
    }
    Ok(ProjectIdentity::PathFallback(canonical))
}

/// Re-point this project at a **new** minted id, discarding any existing one.
///
/// The `:kb-relink` repair verb. Git itself ships `git worktree repair` for
/// exactly this reason: path-independent identity is unachievable in general, so
/// a repair verb is not a defeat — it is what every system in this space needed
/// and most lacked.
pub fn relink(root: &Path) -> Result<ProjectIdentity, IdentityError> {
    let canonical = root
        .canonicalize()
        .map_err(|_| IdentityError::UnresolvablePath(root.display().to_string()))?;
    if !is_git_repo(&canonical) {
        return Ok(ProjectIdentity::PathFallback(canonical));
    }
    mint_id(&canonical)
        .map(ProjectIdentity::Minted)
        .ok_or_else(|| IdentityError::Unwritable(canonical.display().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {:?}", out);
    }

    fn repo() -> TempDir {
        let d = TempDir::new().unwrap();
        git(d.path(), &["init", "-q"]);
        d
    }

    #[test]
    fn a_git_repo_gets_a_minted_id_that_persists() {
        let d = repo();
        let first = resolve(d.path()).unwrap();
        assert!(
            first.is_stable(),
            "a git repo must get a stable id: {first:?}"
        );

        // Resolving again returns the SAME id -- minted once, not per call.
        assert_eq!(resolve(d.path()).unwrap(), first);
    }

    /// **The VS Code failure mode, and the whole point of minting.**
    ///
    /// Its `workspaceStorage` keys on the raw path salted with inode/birthtime,
    /// so a move mints a NEW identity for the same project and orphans its state.
    /// A minted id must survive the directory being renamed.
    #[test]
    fn a_minted_id_survives_the_project_being_moved() {
        let parent = TempDir::new().unwrap();
        let before = parent.path().join("before");
        std::fs::create_dir(&before).unwrap();
        git(&before, &["init", "-q"]);

        let original = resolve(&before).unwrap();

        let after = parent.path().join("after");
        std::fs::rename(&before, &after).unwrap();

        assert_eq!(
            resolve(&after).unwrap(),
            original,
            "renaming the directory must NOT change the KB identity -- that is \
             exactly the bug this design exists to avoid"
        );
    }

    /// Two different repos must never collide, however similarly they are named.
    #[test]
    fn two_repos_get_different_ids() {
        let a = repo();
        let b = repo();
        assert_ne!(resolve(a.path()).unwrap(), resolve(b.path()).unwrap());
    }

    /// Tier 2: a non-git directory still resolves, but says it is weaker.
    #[test]
    fn a_non_git_directory_falls_back_to_a_canonical_path() {
        let d = TempDir::new().unwrap();
        let id = resolve(d.path()).unwrap();
        assert!(matches!(id, ProjectIdentity::PathFallback(_)));
        assert!(
            !id.is_stable(),
            "the fallback must ADMIT it does not survive a move, rather than \
             passing itself off as equivalent"
        );
    }

    /// The path is canonicalized before use, always. Two spellings of one
    /// directory must resolve identically -- and, the sibling risk, two
    /// different directories must not collide.
    #[test]
    fn paths_are_canonicalized_before_comparison() {
        let d = TempDir::new().unwrap();
        let sub = d.path().join("proj");
        std::fs::create_dir(&sub).unwrap();
        let spelled_oddly = d.path().join("proj").join(".").join("..").join("proj");

        assert_eq!(
            resolve(&sub).unwrap(),
            resolve(&spelled_oddly).unwrap(),
            "`proj` and `proj/./../proj` are the same directory"
        );
    }

    /// A path that does not exist is a REFUSAL, not a silently-minted identity.
    ///
    /// Binding the wrong KB to a project is the failure that matters, so "I
    /// cannot identify this" has to be a first-class outcome.
    #[test]
    fn a_nonexistent_path_is_refused_rather_than_guessed() {
        let err = resolve(Path::new("/definitely/not/a/real/path/anywhere"))
            .expect_err("a missing path must not silently mint an identity");
        assert!(matches!(err, IdentityError::UnresolvablePath(_)));
    }

    /// `:kb-relink` mints a FRESH id, which is the repair verb's whole purpose:
    /// re-point a project whose KB association went wrong.
    #[test]
    fn relink_mints_a_new_id_replacing_the_old() {
        let d = repo();
        let before = resolve(d.path()).unwrap();
        let after = relink(d.path()).unwrap();

        assert!(after.is_stable());
        assert_ne!(before, after, "relink must actually re-point, not no-op");
        // ...and the new id is what subsequent resolution returns.
        assert_eq!(resolve(d.path()).unwrap(), after);
    }

    /// The id lives in `$GIT_COMMON_DIR/config`, so it is shared across every
    /// worktree of one clone -- which dissolves the worktree-splitting problem
    /// visible in other tools' per-project state.
    #[test]
    fn all_worktrees_of_one_clone_share_the_id() {
        let d = repo();
        // A commit is needed before `git worktree add`.
        std::fs::write(d.path().join("f"), "x").unwrap();
        git(d.path(), &["add", "f"]);
        git(
            d.path(),
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-qm",
                "init",
            ],
        );
        let main_id = resolve(d.path()).unwrap();

        let wt = TempDir::new().unwrap();
        let wt_path = wt.path().join("linked");
        git(
            d.path(),
            &["worktree", "add", "-q", wt_path.to_str().unwrap()],
        );

        assert_eq!(
            resolve(&wt_path).unwrap(),
            main_id,
            "a linked worktree is the same clone and must share its KB identity"
        );
    }
}
