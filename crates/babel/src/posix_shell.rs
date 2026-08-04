//! Windows POSIX-shell resolution for babel `sh`/`bash` blocks.
//!
//! Extracted from `execute.rs` (#521 follow-up, size-ceiling extraction) — kept
//! as a cohesive, self-contained unit: the shell-resolution functions and the
//! tests that exercise them. See [`resolve_posix_shell`] for the entry point.

#[cfg(windows)]
use std::path::Path;

#[cfg(test)]
use crate::execute::resolve_command;
#[cfg(test)]
use crate::HeaderArgs;

/// True when `candidate` lives inside the Windows system directory.
///
/// @ai-caution: [cross-platform] `C:\Windows\System32\bash.exe` is **not** a
/// POSIX shell -- it is the Windows Subsystem for Linux launcher. It sits in the
/// system directory, which precedes the Git-for-Windows directories on the
/// default `PATH`, so a bare `Command::new("bash")` finds it before any real
/// shell. On a machine with no WSL distribution installed it never runs the
/// block at all: it prints "Windows Subsystem for Linux has no installed
/// distributions" encoded as **UTF-16**, which `from_utf8_lossy` then turns into
/// NUL-riddled mojibake inside the user's org file. Never resolve `sh`/`bash` to
/// a binary under the system root.
///
/// Kept compiled on every platform (not `#[cfg(windows)]`) so the rule stays
/// unit-testable on the machines MAE is actually developed on -- principle #13:
/// a Windows-only code path nobody can iterate against is exactly how this class
/// of bug survives.
/// Split a Windows path into its non-empty components, accepting *either*
/// separator (Windows itself accepts both).
///
/// These rules are deliberately expressed over `&str` rather than
/// `std::path::Path`: `Path` only understands `\` when it is compiled *for*
/// Windows, so a `Path`-based implementation would silently degrade to
/// "one giant component" everywhere else -- making every rule below
/// unobservable on the Linux/macOS machines MAE is actually developed on.
/// Principle #13: the fix has to be verifiable on the platform in front of you.
#[cfg_attr(not(windows), allow(dead_code))]
fn win_components(path: &str) -> Vec<&str> {
    path.split(['/', '\\']).filter(|c| !c.is_empty()).collect()
}

/// Join `parts` onto `base` with the Windows separator.
#[cfg_attr(not(windows), allow(dead_code))]
fn win_join(base: &str, parts: &[&str]) -> String {
    let mut out = base.trim_end_matches(['/', '\\']).to_string();
    for part in parts {
        out.push('\\');
        out.push_str(part);
    }
    out
}

#[cfg_attr(not(windows), allow(dead_code))]
fn is_under_windows_system_root(candidate: &str, system_root: &str) -> bool {
    let root = win_components(system_root);
    let candidate = win_components(candidate);
    // Compare component-wise, not by string prefix: a `starts_with` test would
    // also swallow a sibling directory such as `C:\Windows-Tools\bin`.
    !root.is_empty()
        && candidate.len() >= root.len()
        && root
            .iter()
            .zip(&candidate)
            .all(|(r, c)| r.eq_ignore_ascii_case(c))
}

/// Derive a Git for Windows install root from the location of `git.exe`.
///
/// Git for Windows puts `git.exe` in `<root>\cmd` (and `<root>\bin`) and its
/// POSIX shell in `<root>\bin\bash.exe`. Deriving the root from wherever `git`
/// actually is covers custom install locations that a hardcoded
/// `C:\Program Files\Git` list would miss.
#[cfg_attr(not(windows), allow(dead_code))]
fn git_install_root(git_exe: &str) -> Option<String> {
    let components = win_components(git_exe);
    let dir_index = components.len().checked_sub(2)?;
    let dir = components[dir_index];
    if !matches!(dir.to_ascii_lowercase().as_str(), "cmd" | "bin" | "mingw64") {
        return None;
    }
    let root = &components[..dir_index];
    (!root.is_empty()).then(|| root.join("\\"))
}

/// Pick a real POSIX shell for `sh`/`bash` blocks on Windows.
///
/// Pure over its inputs (the `PATH` entries, where `git.exe` was found, the
/// candidate install roots, the system root, and an `exists` probe) so the
/// ordering rules can be unit-tested off Windows.
///
/// `PATH` is consulted first, minus the WSL stub, so `PATH` remains the user's
/// override mechanism exactly as it is on Unix -- we only refuse the one entry
/// that is a launcher rather than a shell. Git-derived and well-known install
/// roots follow, since Git for Windows does not put `bash.exe` on `PATH` by
/// default even though it ships one.
#[cfg_attr(not(windows), allow(dead_code))]
fn select_windows_posix_shell(
    path_entries: &[String],
    git_exe: Option<&str>,
    install_roots: &[String],
    system_root: Option<&str>,
    exists: &dyn Fn(&str) -> bool,
) -> Option<String> {
    for dir in path_entries {
        let candidate = win_join(dir, &["bash.exe"]);
        if let Some(root) = system_root {
            if is_under_windows_system_root(&candidate, root) {
                continue;
            }
        }
        if exists(&candidate) {
            return Some(candidate);
        }
    }
    let roots = git_exe
        .and_then(git_install_root)
        .into_iter()
        .chain(install_roots.iter().cloned());
    for root in roots {
        for sub in [
            ["bin", "bash.exe"].as_slice(),
            ["usr", "bin", "bash.exe"].as_slice(),
        ] {
            let candidate = win_join(&root, sub);
            if exists(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Resolve the shell that `sh`/`bash` blocks are executed with.
///
/// On Unix this is plain `bash`, resolved from `PATH` by the OS exactly as
/// before -- this function introduces no behavior change off Windows.
///
/// On Windows it searches for a genuine POSIX shell rather than trusting
/// `PATH`'s first `bash`, which is the WSL launcher (see
/// [`is_under_windows_system_root`]). If nothing is found we still fall back to
/// `bash`: that keeps the previous behavior instead of hard-failing a user who
/// has some shell we did not anticipate, and the WSL launcher's complaint now
/// arrives as legible text rather than NUL corruption thanks to
/// `results::normalize_output`. A per-block `:cmd` remains the explicit override
/// on every platform.
pub(crate) fn resolve_posix_shell() -> String {
    #[cfg(windows)]
    {
        let path_entries: Vec<String> = std::env::var_os("PATH")
            .map(|p| {
                std::env::split_paths(&p)
                    .map(|dir| dir.to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        let git_exe = path_entries
            .iter()
            .map(|dir| win_join(dir, &["git.exe"]))
            .find(|p| Path::new(p).is_file());
        let install_roots: Vec<String> = ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
            .iter()
            .filter_map(|key| std::env::var(key).ok())
            .map(|root| win_join(&root, &["Git"]))
            .chain(
                std::env::var("LOCALAPPDATA")
                    .ok()
                    .map(|root| win_join(&root, &["Programs", "Git"])),
            )
            .collect();
        let system_root = std::env::var("SystemRoot").ok();
        if let Some(shell) = select_windows_posix_shell(
            &path_entries,
            git_exe.as_deref(),
            &install_roots,
            system_root.as_deref(),
            &|p| Path::new(p).is_file(),
        ) {
            return shell;
        }
    }
    "bash".to_string()
}

#[cfg(test)]
mod posix_shell_tests {
    use super::*;

    /// Windows paths, exercised on whatever platform CI/the developer is on --
    /// the rule is pure string/patch logic, so there is no reason to make it
    /// only observable on a runner we cannot iterate against.
    #[test]
    fn the_wsl_launcher_is_recognized_under_the_system_root() {
        let root = r"C:\Windows";
        for stub in [
            r"C:\Windows\System32\bash.exe",
            r"C:\WINDOWS\system32\bash.exe", // case-insensitive filesystem
            r"c:/windows/system32/bash.exe", // forward slashes are legal on Windows
            r"C:\Windows\bash.exe",
        ] {
            assert!(
                is_under_windows_system_root(stub, root),
                "{stub} must be refused as the WSL launcher"
            );
        }
        // The negative half: a real shell must NOT be mistaken for the stub.
        for real in [
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\msys64\usr\bin\bash.exe",
            r"C:\Windows-Tools\bin\bash.exe", // prefix-only match must not fire
            r"D:\Windows\System32\bash.exe",  // different volume
        ] {
            assert!(
                !is_under_windows_system_root(real, root),
                "{real} is a real shell and must not be refused"
            );
        }
    }

    #[test]
    fn git_install_root_is_derived_from_where_git_actually_is() {
        assert_eq!(
            git_install_root(r"C:\Program Files\Git\cmd\git.exe").as_deref(),
            Some(r"C:\Program Files\Git")
        );
        assert_eq!(
            git_install_root(r"D:\tools\Git\bin\git.exe").as_deref(),
            Some(r"D:\tools\Git")
        );
        // A `git.exe` somewhere unrecognized must not invent a root.
        assert_eq!(git_install_root(r"C:\odd\git.exe"), None);
    }

    /// The whole point of the fix: given a `PATH` whose first `bash.exe` is the
    /// WSL stub, resolution must skip it and land on the real shell. This is the
    /// exact shape of the GitHub Windows runner that produced the CI failure.
    #[test]
    fn path_resolution_skips_the_wsl_stub_and_finds_the_real_shell() {
        let system32 = r"C:\Windows\System32".to_string();
        let git_bin = r"C:\Program Files\Git\bin".to_string();
        let present = [
            r"C:\Windows\System32\bash.exe", // the trap
            r"C:\Program Files\Git\bin\bash.exe",
        ];
        let exists = |p: &str| present.contains(&p);

        let picked = select_windows_posix_shell(
            &[system32.clone(), git_bin.clone()],
            None,
            &[],
            Some(r"C:\Windows"),
            &exists,
        );
        assert_eq!(
            picked.as_deref(),
            Some(r"C:\Program Files\Git\bin\bash.exe")
        );

        // Without the system-root exclusion the stub would win -- proving the
        // assertion above is actually testing the exclusion and not the
        // incidental ordering of the PATH entries.
        let unguarded = select_windows_posix_shell(&[system32, git_bin], None, &[], None, &exists);
        assert_eq!(
            unguarded.as_deref(),
            Some(r"C:\Windows\System32\bash.exe"),
            "precondition: the stub is what a naive PATH scan picks"
        );
    }

    /// Git for Windows does not put `bash.exe` on `PATH`, so the common real
    /// case is: PATH yields only the stub, and the shell has to come from an
    /// install root -- derived from `git.exe`, or from a well-known location.
    #[test]
    fn falls_back_to_install_roots_when_path_has_only_the_stub() {
        let usr_bin_shell = r"C:\Program Files\Git\usr\bin\bash.exe";
        let exists = |p: &str| p == usr_bin_shell;
        let path_entries = [r"C:\Windows\System32".to_string()];
        let system_root = Some(r"C:\Windows");

        // Derived from where git.exe lives.
        assert_eq!(
            select_windows_posix_shell(
                &path_entries,
                Some(r"C:\Program Files\Git\cmd\git.exe"),
                &[],
                system_root,
                &exists,
            )
            .as_deref(),
            Some(usr_bin_shell)
        );
        // Or from a well-known install root when git.exe was not on PATH.
        assert_eq!(
            select_windows_posix_shell(
                &path_entries,
                None,
                &[r"C:\Program Files\Git".to_string()],
                system_root,
                &exists,
            )
            .as_deref(),
            Some(usr_bin_shell)
        );
        // And nothing is invented when no shell is installed at all.
        assert_eq!(
            select_windows_posix_shell(
                &path_entries,
                None,
                &[r"C:\Program Files\Git".to_string()],
                system_root,
                &|_| false,
            ),
            None
        );
    }

    /// Off Windows nothing changes: `sh`/`bash` still resolve to plain `bash`
    /// from `PATH`, so this fix cannot regress the platforms it is not for.
    #[cfg(not(windows))]
    #[test]
    fn unix_shell_resolution_is_unchanged() {
        let (cmd, args) = resolve_command("sh", &HeaderArgs::default());
        assert_eq!(cmd, "bash");
        assert!(args.is_empty());
        assert_eq!(resolve_command("bash", &HeaderArgs::default()).0, "bash");
    }
}
