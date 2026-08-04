//! Workspace trust for project-local configuration (ADR-089).
//!
//! MAE evaluates `$CWD/.mae/init.scm` at startup. Scheme can spawn processes, and
//! init files run during bootstrap — *before* any `PermissionPolicy` exists — so the
//! AI permission tier does not and cannot bound this path. Without a trust boundary,
//! cloning a repository and opening MAE in it is arbitrary code execution.
//!
//! This module is the boundary. A project-local init file is evaluated only from a
//! directory the user has explicitly listed in `~/.config/mae/trusted-projects`.
//!
//! @ai-caution: [security] Trust is deliberately **file-only** — there is no command,
//! no Scheme primitive, and no MCP tool that grants it. That is the point: an agent
//! that could grant trust could then plant `.mae/init.scm` and escalate across a
//! restart (the CVE-2025-53773 shape). An interactive `:trust-project` command needs
//! ADR-084's tier enforcement to gate it at the privileged tier; until that lands,
//! adding any programmatic grant path re-opens the hole this module closes.
//!
//! The companion half is [`is_protected_config_path`], which keeps a write-tier agent
//! from creating the files this module gates.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Basename of the trust list inside the user's MAE config directory.
const TRUST_FILE: &str = "trusted-projects";

/// XDG-first user config directory (`$XDG_CONFIG_HOME/mae`, else `~/.config/mae`).
///
/// Deliberately does not use the `dirs` crate: on macOS that resolves to
/// `~/Library/Application Support` and ignores `XDG_CONFIG_HOME`, which breaks both
/// the documented `~/.config/mae` contract and env-var test isolation (principle #13).
pub fn user_config_dir() -> Option<PathBuf> {
    std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })
        .map(|base| base.join("mae"))
}

/// Path of the trust list. `None` when neither `XDG_CONFIG_HOME` nor `HOME` is set.
pub fn trust_file_path() -> Option<PathBuf> {
    user_config_dir().map(|d| d.join(TRUST_FILE))
}

/// Resolve a path to its canonical form for comparison, tolerating non-existence.
///
/// Canonicalizes the deepest existing ancestor and re-joins the remainder, so a path
/// that does not exist yet still has its symlinks and `..` components resolved. This
/// is what makes the guard hold against `./a/../.mae/init.scm` and against a symlink
/// pointing into the config directory — neither survives resolution.
fn resolve(path: &Path) -> PathBuf {
    if let Ok(c) = path.canonicalize() {
        return c;
    }
    let mut suffix: Vec<&std::ffi::OsStr> = Vec::new();
    let mut cur = path;
    loop {
        match cur.parent() {
            Some(parent) => {
                if let Some(name) = cur.file_name() {
                    suffix.push(name);
                }
                if let Ok(c) = parent.canonicalize() {
                    let mut out = c;
                    for part in suffix.iter().rev() {
                        out.push(part);
                    }
                    return out;
                }
                cur = parent;
            }
            None => return path.to_path_buf(),
        }
    }
}

/// Load the set of trusted project directories.
///
/// Fails closed: an unreadable, missing, or malformed trust list yields an empty set,
/// never a permissive one. Blank lines and `#` comments are ignored; `~` is expanded.
pub fn load_trusted() -> HashSet<PathBuf> {
    let Some(path) = trust_file_path() else {
        return HashSet::new();
    };
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return HashSet::new();
    };
    contents
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let expanded = if let Some(rest) = l.strip_prefix("~/") {
                std::env::var("HOME")
                    .ok()
                    .map(|h| PathBuf::from(h).join(rest))
                    .unwrap_or_else(|| PathBuf::from(l))
            } else {
                PathBuf::from(l)
            };
            resolve(&expanded)
        })
        .collect()
}

/// Is `dir` an explicitly trusted project directory?
///
/// Exact match on the resolved path. Trust is deliberately **not** inherited by
/// subdirectories or siblings: trusting `~/src/foo` must not silently trust
/// `~/src/foo/vendor/hostile`, which is exactly where a cloned dependency lands.
pub fn is_trusted(dir: &Path) -> bool {
    let resolved = resolve(dir);
    load_trusted().contains(&resolved)
}

/// Is this path one that governs MAE's own behaviour, and therefore off-limits to
/// non-privileged writes (ADR-089 D4)?
///
/// Covers the user config directory (`~/.config/mae/**`, including the trust list
/// itself) and any project-local `.mae/**`. An agent must not be able to edit the
/// files that decide what the agent may do — the "hints may tighten, never loosen"
/// ratchet. Resolution happens first, so `..` traversal and symlinks do not evade it.
pub fn is_protected_config_path(path: &Path) -> bool {
    let resolved = resolve(path);

    if let Some(cfg) = user_config_dir() {
        let cfg = resolve(&cfg);
        if resolved == cfg || resolved.starts_with(&cfg) {
            return true;
        }
    }

    // Any `.mae` directory anywhere in the resolved chain — project-local config.
    resolved
        .components()
        .any(|c| c.as_os_str() == std::ffi::OsStr::new(".mae"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Point config resolution at a scratch dir for the duration of the closure.
    ///
    /// Serialised because it mutates process-global env; parallel tests would
    /// otherwise observe each other's `XDG_CONFIG_HOME`.
    fn with_config_home<R>(dir: &Path, f: impl FnOnce() -> R) -> R {
        use std::sync::Mutex;
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var("XDG_CONFIG_HOME").ok();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir) };
        let out = f();
        match saved {
            Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
        }
        out
    }

    fn write_trust_list(cfg_home: &Path, body: &str) {
        let dir = cfg_home.join("mae");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(TRUST_FILE), body).unwrap();
    }

    // --- The attacker's tests: untrusted must not be trusted. ---

    #[test]
    fn a_directory_absent_from_the_list_is_not_trusted() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("cloned-repo");
        std::fs::create_dir_all(&project).unwrap();
        with_config_home(tmp.path(), || {
            write_trust_list(tmp.path(), "");
            assert!(!is_trusted(&project));
        });
    }

    #[test]
    fn a_missing_trust_list_trusts_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        with_config_home(tmp.path(), || {
            assert!(trust_file_path().is_some_and(|p| !p.exists()));
            assert!(!is_trusted(&project));
        });
    }

    #[test]
    fn a_malformed_trust_list_trusts_nothing_rather_than_everything() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        with_config_home(tmp.path(), || {
            // Binary garbage: read_to_string fails -> empty set, not a wildcard.
            let dir = tmp.path().join("mae");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(TRUST_FILE), [0xff, 0xfe, 0x00, 0x80]).unwrap();
            assert!(!is_trusted(&project));
        });
    }

    #[test]
    fn trust_is_not_inherited_by_a_subdirectory() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("trusted-parent");
        let child = parent.join("vendor").join("hostile-dep");
        std::fs::create_dir_all(&child).unwrap();
        with_config_home(tmp.path(), || {
            write_trust_list(tmp.path(), &parent.display().to_string());
            assert!(is_trusted(&parent), "the listed dir itself is trusted");
            assert!(
                !is_trusted(&child),
                "a vendored subdirectory must NOT inherit trust"
            );
        });
    }

    #[test]
    fn trust_is_not_shared_with_a_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("proj-a");
        let b = tmp.path().join("proj-b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        with_config_home(tmp.path(), || {
            write_trust_list(tmp.path(), &a.display().to_string());
            assert!(is_trusted(&a));
            assert!(!is_trusted(&b));
        });
    }

    #[test]
    fn traversal_does_not_launder_an_untrusted_directory_into_a_trusted_one() {
        let tmp = tempfile::tempdir().unwrap();
        let trusted = tmp.path().join("trusted");
        let hostile = tmp.path().join("hostile");
        std::fs::create_dir_all(&trusted).unwrap();
        std::fs::create_dir_all(&hostile).unwrap();
        with_config_home(tmp.path(), || {
            write_trust_list(tmp.path(), &trusted.display().to_string());
            // Spelled as a path *through* the trusted dir, resolving to the hostile one.
            let laundered = trusted.join("..").join("hostile");
            assert!(!is_trusted(&laundered));
        });
    }

    #[test]
    fn comments_and_blank_lines_are_ignored_and_do_not_become_trusted_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        with_config_home(tmp.path(), || {
            write_trust_list(
                tmp.path(),
                &format!("# a comment\n\n   \n{}\n", project.display()),
            );
            let trusted = load_trusted();
            assert_eq!(trusted.len(), 1, "only the real path is loaded");
            assert!(is_trusted(&project));
        });
    }

    // --- Protected config paths (D4). ---

    #[test]
    fn user_config_dir_contents_are_protected() {
        let tmp = tempfile::tempdir().unwrap();
        with_config_home(tmp.path(), || {
            let dir = tmp.path().join("mae");
            std::fs::create_dir_all(&dir).unwrap();
            assert!(is_protected_config_path(&dir.join("init.scm")));
            assert!(is_protected_config_path(&dir.join("config.toml")));
            assert!(is_protected_config_path(&dir.join(TRUST_FILE)));
        });
    }

    #[test]
    fn project_local_dot_mae_is_protected_anywhere_in_the_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("a").join("b").join(".mae");
        std::fs::create_dir_all(&nested).unwrap();
        with_config_home(tmp.path(), || {
            assert!(is_protected_config_path(&nested.join("init.scm")));
        });
    }

    #[test]
    fn traversal_does_not_evade_the_protected_path_guard() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(proj.join(".mae")).unwrap();
        std::fs::create_dir_all(proj.join("src")).unwrap();
        with_config_home(tmp.path(), || {
            let sneaky = proj.join("src").join("..").join(".mae").join("init.scm");
            assert!(
                is_protected_config_path(&sneaky),
                "`..` must not launder a protected path"
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_pointing_into_the_config_dir_does_not_evade_the_guard() {
        let tmp = tempfile::tempdir().unwrap();
        with_config_home(tmp.path(), || {
            let cfg = tmp.path().join("mae");
            std::fs::create_dir_all(&cfg).unwrap();
            let link = tmp.path().join("innocent-looking");
            std::os::unix::fs::symlink(&cfg, &link).unwrap();
            assert!(
                is_protected_config_path(&link.join("init.scm")),
                "a symlink into the config dir must still be protected"
            );
        });
    }

    #[test]
    fn an_ordinary_source_file_is_not_protected() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj").join("src");
        std::fs::create_dir_all(&proj).unwrap();
        with_config_home(tmp.path(), || {
            assert!(!is_protected_config_path(&proj.join("main.rs")));
            // A file merely *named* like config, outside any protected dir, is fine.
            assert!(!is_protected_config_path(&proj.join("init.scm")));
        });
    }
}
