use std::process::Command;

/// Embed the RESOLVED `yrs` version (and whether it is a patched git source) as
/// `MAE_YRS_VERSION`, for `mae-daemon doctor`.
///
/// That line used to be `println!("  yrs version: 0.22")` — a hand-typed constant
/// that had drifted to report 0.22 while the daemon actually linked 0.27.4. A
/// diagnostic that lies about a dependency version is worse than one that omits
/// it, and it is worst precisely when it matters: right after a security bump of
/// that dependency, which is when someone runs `doctor` to check. CLAUDE.md's
/// rule against writing measured numbers into prose applies to diagnostics too.
///
/// Parsed from the lockfile rather than the manifest so it reflects what is
/// actually linked, including the `[patch.crates-io]` fork carrying y-crdt #644.
/// Falls back to "unknown" so a build from an extracted tarball still succeeds.
fn yrs_version() -> String {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let lock = std::path::Path::new(&manifest).join("Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock.display());
    let Ok(text) = std::fs::read_to_string(&lock) else {
        return "unknown".to_string();
    };
    // Walk the `[[package]]` block whose name is exactly `yrs`.
    let mut in_yrs = false;
    let (mut version, mut patched) = (None, false);
    for line in text.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            if version.is_some() {
                break; // finished the yrs block
            }
            in_yrs = false;
        } else if line == r#"name = "yrs""# {
            in_yrs = true;
        } else if in_yrs {
            if let Some(v) = line.strip_prefix(r#"version = ""#) {
                version = Some(v.trim_end_matches('"').to_string());
            } else if line.starts_with(r#"source = "git+"#) {
                patched = true;
            }
        }
    }
    match (version, patched) {
        (Some(v), true) => format!("{v} (patched fork)"),
        (Some(v), false) => v,
        (None, _) => "unknown".to_string(),
    }
}

/// Embed the short git SHA (with a `-dirty` suffix for uncommitted trees) as
/// `MAE_BUILD_SHA`, so the editor can report *exactly* which build is running —
/// the cross-machine deploy-discipline gap the live two-machine test kept hitting
/// ("are both machines on the same commit?"). Cross-platform (CLAUDE.md #13):
/// `git` behaves identically on macOS + Linux; if git is absent or this isn't a
/// checkout (e.g. a release tarball built from an extracted source archive), fall
/// back to "unknown" so the build still succeeds.
fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let build = match sha {
        Some(sha) => {
            let dirty = Command::new("git")
                .args(["status", "--porcelain"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| !o.stdout.is_empty())
                .unwrap_or(false);
            if dirty {
                format!("{sha}-dirty")
            } else {
                sha
            }
        }
        None => "unknown".to_string(),
    };

    println!("cargo:rustc-env=MAE_BUILD_SHA={build}");
    println!("cargo:rustc-env=MAE_YRS_VERSION={}", yrs_version());

    // Rebuild when HEAD moves. `.git/HEAD` only changes on a branch switch (its
    // content is `ref: refs/heads/<branch>`, which a same-branch commit doesn't
    // touch) — watching just that left the embedded SHA silently stale after every
    // commit that didn't also switch branches, exactly the deploy-discipline gap
    // this exists to close. `.git/logs/HEAD` (the reflog) is appended on every
    // commit/checkout/merge/reset, so watch both. `--git-path` resolves the real
    // location portably (handles worktrees).
    for path in ["HEAD", "logs/HEAD"] {
        if let Some(resolved) = Command::new("git")
            .args(["rev-parse", "--git-path", path])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            println!("cargo:rerun-if-changed={resolved}");
        }
    }
}
