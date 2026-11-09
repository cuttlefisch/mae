//! When each document was created and each file last modified, from ONE walk
//! of history and ONE read of the blobs it names.
//!
//! **A document's identity is its file-level `:ID:`, not its path.** So:
//!
//! - **created** is the committer time of the first commit, in walk order, in
//!   which a blob whose file drawer carries that `:ID:` appeared, at ANY path.
//!   A page moved between folders as a delete in one commit and an add in a
//!   later one keeps its date. So does a page restored after a delete. A page
//!   regenerated at the same path under a NEW id is created by the commit
//!   that introduced the new id. A historical copy that later took its own id
//!   is created when that id first appears. (A copy that still shares its id
//!   at the rev is a duplicate-id refusal, so it never reaches this.)
//! - **modified** is the committer time of the last commit that changed the
//!   file's own path lineage at the rev: an add, an edit, or a rename onto its
//!   current path (`git log -1 --format=%ct -- PATH`).
//! - The path lineage also gives a fallback creation (the add at the path,
//!   following renames), used only if a node's `:ID:` was never seen in history.
//!   That should not happen for a file present at the rev; the report labels it
//!   `git-path` when it does.
//!
//! One `git log --reverse --raw -M -l0` pass gives every change with its new
//! blob sha. Every distinct `.org` blob is then read in one `git cat-file
//! --batch` pass and only its file drawer's `:ID:` is extracted
//! ([`drawer_id`]), so the cost is one read of each version of each page.
//!
//! `-l0` lifts git's rename limit. Past it (`diff.renameLimit`, default 1000
//! files), inexact renames silently turn into a delete plus an add. A git
//! warning that detection was skipped is an error (`GitRepo::run_checked`).
//!
//! Merge commits list no files (no `-m`), so changes on a merged branch are
//! seen through that branch's own commits. Walk order is git's default order,
//! reversed. On a linear history that is commit order; across branches it is
//! date order.

use std::collections::{HashMap, HashSet};

use super::git::GitRepo;
use super::SeedError;

/// A path lineage's creation and last modification (committer unix seconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileTimes {
    pub created: i64,
    pub modified: i64,
}

/// What the walk learned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    /// Every path present at the rev: its lineage's times.
    pub paths: HashMap<String, FileTimes>,
    /// Every file-level `:ID:` ever seen in a `.org` blob: when it first appeared.
    pub ids: HashMap<String, i64>,
    /// How many distinct blobs were read to find the ids.
    pub blobs_read: usize,
}

/// One change in one commit, with the blob it leaves at its destination.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Change {
    Add(String, Blob),
    Modify(String, Blob),
    Delete(String),
    Rename(String, String, Blob),
}

/// A destination blob: its sha, and whether its mode is a regular file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Blob {
    sha: String,
    regular: bool,
}

type Commits = Vec<(i64, Vec<Change>)>;

/// Walk `commit`'s history once, then read its `.org` blobs once.
pub fn history(repo: &GitRepo, commit: &str) -> Result<History, SeedError> {
    let out = repo.run_checked(&[
        "log",
        "--reverse",
        "--format=format:%x01%ct",
        "--raw",
        "--no-abbrev",
        "-M",
        "-l0",
        "-z",
        "--no-show-signature",
        commit,
        "--",
    ])?;
    let commits = parse(&out)?;
    let first_seen = blob_first_seen(&commits);
    let shas: Vec<String> = first_seen.iter().map(|(sha, _)| sha.clone()).collect();
    let mut ids: HashMap<String, i64> = HashMap::new();
    let mut at = 0;
    repo.for_each_blob(&shas, |bytes| {
        if let Some(id) = drawer_id(&String::from_utf8_lossy(bytes)) {
            ids.entry(id).or_insert(first_seen[at].1);
        }
        at += 1;
    })?;
    Ok(History {
        paths: fold(commits),
        ids,
        blobs_read: shas.len(),
    })
}

/// Distinct `.org` blobs in the order they first appear, with that commit's time.
fn blob_first_seen(commits: &Commits) -> Vec<(String, i64)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (t, changes) in commits {
        for c in changes {
            let (path, blob) = match c {
                Change::Add(p, b) | Change::Modify(p, b) | Change::Rename(_, p, b) => (p, b),
                Change::Delete(_) => continue,
            };
            if blob.regular && path.ends_with(".org") && seen.insert(blob.sha.clone()) {
                out.push((blob.sha.clone(), *t));
            }
        }
    }
    out
}

/// The file drawer's `:ID:`, read the way `split` reads it for a canonical
/// header: the first line is `:PROPERTIES:`, the drawer ends at `:END:`, and
/// exactly one `:ID:` line in it has a non-empty value. Anything else has no
/// id here. (A property test checks this agrees with `split` on every
/// generated page.)
pub fn drawer_id(text: &str) -> Option<String> {
    let mut lines = text.lines();
    if !lines.next()?.trim().eq_ignore_ascii_case(":PROPERTIES:") {
        return None;
    }
    let mut found = None;
    for line in lines {
        let line = line.trim();
        if line.eq_ignore_ascii_case(":END:") {
            return found;
        }
        let Some((key, value)) = line.strip_prefix(':').and_then(|l| l.split_once(':')) else {
            continue;
        };
        if key.eq_ignore_ascii_case("ID") {
            if found.is_some() {
                return None;
            }
            found = Some(value.trim().to_string()).filter(|v| !v.is_empty());
        }
    }
    None
}

/// `\x01CT\n:SRCMODE DSTMODE SRCSHA DSTSHA STATUS\0PATH\0[PATH\0]:…` per
/// commit, oldest first.
fn parse(out: &[u8]) -> Result<Commits, SeedError> {
    let mut commits: Commits = Vec::new();
    let mut tokens = out
        .split(|b| *b == 0)
        .map(|t| String::from_utf8_lossy(t).into_owned());
    let bad = |what: &str| SeedError(format!("unexpected git log output: {what:?}"));
    while let Some(token) = tokens.next() {
        let token = token.trim_start_matches('\n');
        let meta = match token.strip_prefix('\u{1}') {
            Some(rest) => {
                let (ct, meta) = rest.split_once('\n').unwrap_or((rest, ""));
                let time = ct.trim().parse().map_err(|_| bad(ct))?;
                commits.push((time, Vec::new()));
                meta.to_string()
            }
            None => token.to_string(),
        };
        if meta.is_empty() {
            continue;
        }
        let change = change(&meta, || {
            tokens
                .next()
                .filter(|p| !p.is_empty())
                .ok_or_else(|| bad(&meta))
        })?;
        commits
            .last_mut()
            .ok_or_else(|| bad("a change before any commit"))?
            .1
            .push(change);
    }
    Ok(commits)
}

/// One `:SRCMODE DSTMODE SRCSHA DSTSHA STATUS` entry and its path(s).
fn change(
    meta: &str,
    mut path: impl FnMut() -> Result<String, SeedError>,
) -> Result<Change, SeedError> {
    let bad = || SeedError(format!("unexpected git log --raw entry: {meta:?}"));
    let fields: Vec<&str> = meta.strip_prefix(':').ok_or_else(bad)?.split(' ').collect();
    let [_, dst_mode, _, dst_sha, status] = fields.as_slice() else {
        return Err(bad());
    };
    let blob = Blob {
        sha: dst_sha.to_string(),
        regular: matches!(*dst_mode, "100644" | "100755"),
    };
    Ok(match status.as_bytes().first() {
        Some(b'A') => Change::Add(path()?, blob),
        Some(b'M' | b'T') => Change::Modify(path()?, blob),
        Some(b'D') => Change::Delete(path()?),
        Some(b'R') => {
            let from = path()?;
            Change::Rename(from, path()?, blob)
        }
        // A copy (only if the user's config turns copy detection on) is a new
        // file at its destination; its creation comes from its own id.
        Some(b'C') => {
            path()?;
            Change::Add(path()?, blob)
        }
        _ => return Err(bad()),
    })
}

/// Apply each commit's changes in two phases, sources first, so a swap
/// (`a→b`, `b→a`) in one commit cannot overwrite a lineage it still needs.
fn fold(commits: Commits) -> HashMap<String, FileTimes> {
    let mut files: HashMap<String, FileTimes> = HashMap::new();
    for (t, changes) in commits {
        let fresh = FileTimes {
            created: t,
            modified: t,
        };
        let mut moved = Vec::new();
        for c in &changes {
            match c {
                Change::Delete(p) => {
                    files.remove(p);
                }
                Change::Rename(from, to, _) => {
                    moved.push((to, files.remove(from).unwrap_or(fresh)))
                }
                _ => {}
            }
        }
        for (to, lineage) in moved {
            files.insert(
                to.clone(),
                FileTimes {
                    modified: lineage.modified.max(t),
                    ..lineage
                },
            );
        }
        for c in changes {
            match c {
                Change::Add(p, _) => {
                    files.insert(p, fresh);
                }
                Change::Modify(p, _) => {
                    let e = files.entry(p).or_insert(fresh);
                    e.modified = e.modified.max(t);
                }
                Change::Delete(_) | Change::Rename(..) => {}
            }
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    const Z: &str = "0000000000000000000000000000000000000000";

    fn entry(status: &str, sha: &str) -> String {
        format!(":100644 100644 {Z} {sha} {status}")
    }

    fn walk(out: &str) -> HashMap<String, FileTimes> {
        fold(parse(out.as_bytes()).unwrap())
    }

    fn ft(created: i64, modified: i64) -> FileTimes {
        FileTimes { created, modified }
    }

    #[test]
    fn a_swap_in_one_commit_keeps_both_lineages() {
        let (a, m, r) = (entry("A", "aa"), entry("M", "bb"), entry("R100", "cc"));
        let out = format!(
            "\u{1}10\n{a}\0a\0{a}\0b\0\0\u{1}20\n{m}\0a\0\0\u{1}30\n{r}\0a\0b\0{r}\0b\0a\0"
        );
        let t = walk(&out);
        assert_eq!(t["b"], ft(10, 30), "b now holds a's content, created at 10");
        assert_eq!(t["a"], ft(10, 30));
    }

    #[test]
    fn malformed_output_is_an_error_not_a_guess() {
        let a = entry("A", "aa");
        for bad in [
            format!("\u{1}x\n{a}\0a\0"),
            format!("\u{1}10\n{}\0a\0", entry("Q", "aa")),
            format!("\u{1}10\n{}\0a\0", entry("R100", "aa")),
            format!("{a}\0a\0"),
            "\u{1}10\n:100644 A\0a\0".to_string(),
        ] {
            assert!(parse(bad.as_bytes()).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_drawer_id_is_read_only_from_a_leading_drawer() {
        let id = |s: &str| drawer_id(s);
        assert_eq!(
            id(":PROPERTIES:\n:ID: ens:x\n:ROLE: a\n:END:\nbody"),
            Some("ens:x".into())
        );
        assert_eq!(id(":properties:\n:id:   y  \n:end:\n"), Some("y".into()));
        assert_eq!(
            id("\n:PROPERTIES:\n:ID: x\n:END:\n"),
            None,
            "not the first line"
        );
        assert_eq!(id(":PROPERTIES:\n:ID: x\n:ID: y\n:END:\n"), None, "two ids");
        assert_eq!(id(":PROPERTIES:\n:ID:\n:END:\n"), None, "empty id");
        assert_eq!(id(":PROPERTIES:\n:ID: x\n"), None, "no :END:");
        assert_eq!(
            id("* H\n:PROPERTIES:\n:ID: x\n:END:\n"),
            None,
            "a headline drawer"
        );
    }
}
