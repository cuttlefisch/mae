//! When each file was created and last modified, from ONE walk of history.
//!
//! - **created** is the committer time of the commit that added the file's
//!   content at its current path, following renames back to the add. It is
//!   what `git log --diff-filter=A --follow --format=%ct -- PATH | tail -1`
//!   reports, with two deliberate differences, each pinned by a test:
//!   - **A delete and a later re-add of the same path** starts a new file: the
//!     re-add is the creation. `--follow` walks through the deletion to the
//!     first add, but the content at the path now has no history before the
//!     re-add.
//!   - **A copy** is a new file, created by the commit that made the copy.
//!     `--follow` detects copies (`C100 src copy`) and reports the source's
//!     creation, which would give two documents one birth.
//! - **modified** is the committer time of the last commit that changed the
//!   file (an add, an edit, or a rename onto its current path) at the rev:
//!   `git log -1 --format=%ct -- PATH`.
//!
//! One `git log --reverse --name-status -M -l0` pass over the whole history,
//! rather than ~1,100 `git log --follow` processes. `-l0` lifts git's rename
//! limit: past it (`diff.renameLimit`, default 1000 files) inexact renames
//! silently become a delete plus an add, so a bulk move-and-edit would reset
//! every creation time, while `--follow`, looking at one path, would not.
//!
//! Merge commits list no files (no `-m`); changes made on a merged branch are
//! seen through that branch's own commits. The walk is git's default order,
//! reversed. On a linear history that is the commit order; across branches it
//! is date order, so a path added independently on two branches takes the
//! later add.

use std::collections::HashMap;

use super::git::GitRepo;
use super::SeedError;

/// A file's creation and last modification (committer unix seconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileTimes {
    pub created: i64,
    pub modified: i64,
}

/// One change in one commit, as `--name-status -M` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Change {
    Add(String),
    Modify(String),
    Delete(String),
    Rename(String, String),
}

/// `FileTimes` for every path present at `commit`, from one history walk.
pub fn file_times(repo: &GitRepo, commit: &str) -> Result<HashMap<String, FileTimes>, SeedError> {
    let out = repo.run_checked(&[
        "log",
        "--reverse",
        "--format=format:%x01%ct",
        "--name-status",
        "-M",
        "-l0",
        "-z",
        "--no-show-signature",
        commit,
        "--",
    ])?;
    Ok(fold(parse(&out)?))
}

/// `\x01CT\nSTATUS\0PATH\0[PATH\0]STATUS\0…` per commit, oldest first.
fn parse(out: &[u8]) -> Result<Vec<(i64, Vec<Change>)>, SeedError> {
    let mut commits: Vec<(i64, Vec<Change>)> = Vec::new();
    let mut tokens = out
        .split(|b| *b == 0)
        .map(|t| String::from_utf8_lossy(t).into_owned());
    let bad = |what: &str| SeedError(format!("unexpected git log output: {what}"));
    while let Some(token) = tokens.next() {
        let status = match token.strip_prefix('\u{1}') {
            Some(rest) => {
                let (ct, status) = rest.split_once('\n').unwrap_or((rest, ""));
                let time = ct.trim().parse().map_err(|_| bad(ct))?;
                commits.push((time, Vec::new()));
                status.to_string()
            }
            None => token,
        };
        if status.is_empty() {
            continue;
        }
        let mut path = || {
            tokens
                .next()
                .filter(|p| !p.is_empty())
                .ok_or_else(|| bad(&status))
        };
        let change = match status.as_bytes()[0] {
            b'A' => Change::Add(path()?),
            // A copy (only if the user's config turns copy detection on) is a
            // new file at its destination: see the module doc.
            b'C' => {
                path()?;
                Change::Add(path()?)
            }
            b'M' | b'T' => Change::Modify(path()?),
            b'D' => Change::Delete(path()?),
            b'R' => {
                let from = path()?;
                Change::Rename(from, path()?)
            }
            _ => return Err(bad(&status)),
        };
        commits
            .last_mut()
            .ok_or_else(|| bad("a change before any commit"))?
            .1
            .push(change);
    }
    Ok(commits)
}

/// Apply each commit's changes in two phases, sources first, so a swap
/// (`a→b`, `b→a`) in one commit cannot overwrite a lineage it still needs.
fn fold(commits: Vec<(i64, Vec<Change>)>) -> HashMap<String, FileTimes> {
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
                Change::Rename(from, to) => moved.push((to, files.remove(from).unwrap_or(fresh))),
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
                // A (re-)add starts a new lineage: see the module doc.
                Change::Add(p) => {
                    files.insert(p, fresh);
                }
                Change::Modify(p) => {
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

    fn walk(out: &str) -> HashMap<String, FileTimes> {
        fold(parse(out.as_bytes()).unwrap())
    }

    fn ft(created: i64, modified: i64) -> FileTimes {
        FileTimes { created, modified }
    }

    #[test]
    fn a_swap_in_one_commit_keeps_both_lineages() {
        let out = "\u{1}10\nA\0a\0A\0b\0\0\u{1}20\nM\0a\0\0\u{1}30\nR100\0a\0b\0R100\0b\0a\0";
        let t = walk(out);
        assert_eq!(t["b"], ft(10, 30), "b now holds a's content, created at 10");
        assert_eq!(t["a"], ft(10, 30));
    }

    #[test]
    fn malformed_output_is_an_error_not_a_guess() {
        for bad in [
            "\u{1}x\nA\0a\0",
            "\u{1}10\nQ\0a\0",
            "\u{1}10\nR100\0a\0",
            "A\0a\0",
        ] {
            assert!(parse(bad.as_bytes()).is_err(), "{bad:?}");
        }
    }
}
