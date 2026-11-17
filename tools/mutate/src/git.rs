//! The git questions the tool asks: where its state lives, which files are
//! dirty, which changed since a ref, and what code a result was recorded on.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The lists are hashed per entry instead, so adding a mutant does not make
/// every other recorded verdict stale.
const STATE_EXCLUDE: &str = ":(exclude)mutants";

pub fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "core.quotepath=off"])
        .args(args)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn toplevel(cwd: &Path) -> Result<PathBuf, String> {
    Ok(PathBuf::from(
        git(cwd, &["rev-parse", "--show-toplevel"])?.trim(),
    ))
}

/// `.git/mae-mutate` (per worktree): journal, lock, results and logs live
/// outside the tree, so none of them can be committed or dirty a file.
pub fn state_dir(root: &Path) -> Result<PathBuf, String> {
    let p = PathBuf::from(git(root, &["rev-parse", "--git-path", "mae-mutate"])?.trim());
    Ok(if p.is_absolute() { p } else { root.join(p) })
}

/// The given files that differ from HEAD or are untracked.
pub fn dirty(root: &Path, files: &BTreeSet<String>) -> Result<Vec<String>, String> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec!["status", "--porcelain", "--untracked-files=all", "--"];
    args.extend(files.iter().map(String::as_str));
    Ok(git(root, &args)?
        .lines()
        .filter(|l| l.len() > 3)
        .map(|l| l[3..].to_string())
        .collect())
}

/// Files changed on this branch since `reference`, plus the working tree
/// (staged, unstaged and untracked).
pub fn changed_since(root: &Path, reference: &str) -> Result<BTreeSet<String>, String> {
    let range = format!("{reference}...HEAD");
    let mut out = BTreeSet::new();
    for args in [
        vec!["diff", "--no-ext-diff", "--name-only", range.as_str()],
        vec!["diff", "--no-ext-diff", "--name-only", "HEAD"],
        vec!["ls-files", "--others", "--exclude-standard"],
    ] {
        out.extend(git(root, &args)?.lines().map(str::to_string));
    }
    Ok(out)
}

/// A key for the code a verdict was reached on: HEAD, the working-tree diff
/// and the untracked files, all outside `mutants/`. A verdict recorded on other
/// code is stale and re-run, never reused.
pub fn tree_state(root: &Path) -> Result<String, String> {
    let head = git(root, &["rev-parse", "HEAD"])?;
    let diff = git(
        root,
        &[
            "diff",
            "--no-ext-diff",
            "--no-color",
            "--binary",
            "HEAD",
            "--",
            ".",
            STATE_EXCLUDE,
        ],
    )?;
    let untracked = git(
        root,
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "--",
            ".",
            STATE_EXCLUDE,
        ],
    )?;
    let mut h = Fnv::new();
    h.add(head.trim().as_bytes());
    h.add(diff.as_bytes());
    for f in untracked.lines() {
        h.add(f.as_bytes());
        h.add(&std::fs::read(root.join(f)).unwrap_or_default());
    }
    Ok(format!("{:016x}", h.0))
}

/// FNV-1a 64: deterministic across runs and platforms, which std's hasher
/// does not promise. Not collision-resistant, and nothing here needs it to be.
pub struct Fnv(pub u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn add(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
        // A separator, so ("ab","c") and ("a","bc") differ.
        self.0 ^= 0xff;
        self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
    }
}

pub fn fnv(parts: &[&str]) -> u64 {
    let mut h = Fnv::new();
    for p in parts {
        h.add(p.as_bytes());
    }
    h.0
}
