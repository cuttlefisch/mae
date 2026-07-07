use std::process::Command;

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
