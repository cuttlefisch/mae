//! When each document was created and each page last modified, from the
//! commit graph, ONE batched tree-diff pass and ONE batched blob read.
//!
//! **created** is by `:ID:` (a document's identity, not its path): the
//! smallest committer time of any commit whose diff against one of its parents
//! introduces a `.org` blob whose leading drawer carries that id ([`drawer_id`]).
//! The MINIMUM, not the first in some walk order, so the answer does not depend
//! on how git orders a history with branches. A page moved between folders as a
//! delete and a later add keeps its date, as does a restored page; a page
//! regenerated under a new id is created when that id first appears. A page
//! created on a branch is created by the branch commit, not by the merge.
//!
//! **modified** is what `git log -1 --format=%ct -- PATH` reports at the rev:
//! the commit that introduced the page's current content under git's default
//! history simplification. From the rev, follow the FIRST parent the path is
//! unchanged against (TREESAME); the first commit changed against every parent
//! is the answer. So a change made on a branch and merged cleanly is dated by
//! the branch commit, a conflict resolved in a merge by the merge, and a branch
//! edit the merge discarded does not count at all. A root commit introduces
//! everything in it.
//!
//! How:
//! 1. `git log --format='%H %ct %P'`: the commit graph (one process).
//! 2. `git diff-tree --stdin --always --root --no-renames`: one request per
//!    (commit, parent) pair and one per root, in one process. `--always` makes
//!    every request answer with its commit's header, so answers pair with
//!    requests by position.
//! 3. `git cat-file --batch`: every distinct `.org` blob those diffs introduce,
//!    in one process, reading at most [`HISTORY_PREFIX_BYTES`] of each and
//!    discarding the rest unread into memory. A drawer that does not end
//!    within that prefix yields no id; no page approaches it (the page's own
//!    header must fit the same allowance to be seeded at all).
//!
//! Renames are not detected (`--no-renames`): identity comes from the id, and
//! a path's last change does not care where its content came from.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::process::Stdio;

use super::git::GitRepo;
use super::split::drawer_id;
use super::{SeedError, HEADER_ALLOWANCE_BYTES};

/// How much of each historical blob is read to find its id.
pub const HISTORY_PREFIX_BYTES: usize = HEADER_ALLOWANCE_BYTES;

/// What the history says about the pages at the rev.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    /// For each requested path present at the rev: its last modification.
    pub modified: HashMap<String, i64>,
    /// Every file-level `:ID:` seen in a `.org` blob: its first appearance.
    pub ids: HashMap<String, i64>,
    /// How many distinct blobs were read (prefix only) to find the ids.
    pub blobs_read: usize,
}

/// One commit: its committer time and parents, in order.
struct Commit {
    time: i64,
    parents: Vec<String>,
}

/// The paths each commit changed against each parent (a root: against the
/// empty tree, parent `None`), restricted to `wanted`. Keyed by the request
/// itself, never by position: requests are sorted, parents are not.
type Changed = HashMap<(String, Option<String>), HashSet<String>>;

/// The history of `commit` for the pages at `wanted` paths.
pub fn history(repo: &GitRepo, commit: &str, wanted: &[String]) -> Result<History, SeedError> {
    let graph = commit_graph(repo, commit)?;
    let requests = diff_requests(&graph);
    let out = diff_tree(repo, &requests)?;
    let wanted_set: HashSet<&str> = wanted.iter().map(String::as_str).collect();
    let (changed, introduced) = parse_diffs(&out, &requests, &graph, &wanted_set)?;
    let mut modified = HashMap::new();
    for path in wanted {
        modified.insert(path.clone(), last_change(commit, path, &graph, &changed)?);
    }
    let mut blobs: Vec<(String, i64)> = introduced.into_iter().collect();
    blobs.sort();
    let shas: Vec<String> = blobs.iter().map(|(s, _)| s.clone()).collect();
    let mut ids: HashMap<String, i64> = HashMap::new();
    let mut at = 0;
    repo.for_each_blob_prefix(&shas, HISTORY_PREFIX_BYTES, |prefix| {
        if let Some(id) = drawer_id(&String::from_utf8_lossy(prefix)) {
            let t = blobs[at].1;
            ids.entry(id).and_modify(|e| *e = (*e).min(t)).or_insert(t);
        }
        at += 1;
    })?;
    Ok(History {
        modified,
        ids,
        blobs_read: shas.len(),
    })
}

fn commit_graph(repo: &GitRepo, commit: &str) -> Result<HashMap<String, Commit>, SeedError> {
    let out = repo.run_checked(&["log", "--format=%H %ct %P", commit, "--"])?;
    let mut graph = HashMap::new();
    for line in String::from_utf8_lossy(&out).lines() {
        let mut parts = line.split(' ');
        let (Some(sha), Some(time)) = (parts.next(), parts.next()) else {
            return Err(SeedError(format!("unexpected git log line: {line:?}")));
        };
        let time = time
            .parse()
            .map_err(|_| SeedError(format!("unexpected git log line: {line:?}")))?;
        let parents = parts
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect();
        graph.insert(sha.to_string(), Commit { time, parents });
    }
    Ok(graph)
}

/// `(commit, parent)` per parent, and `(commit, None)` for a root; sorted so
/// the request (and so the run) is deterministic.
fn diff_requests(graph: &HashMap<String, Commit>) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = graph
        .iter()
        .flat_map(|(sha, c)| {
            let parents: Vec<Option<String>> = if c.parents.is_empty() {
                vec![None]
            } else {
                c.parents.iter().cloned().map(Some).collect()
            };
            parents.into_iter().map(move |p| (sha.clone(), p))
        })
        .collect();
    out.sort();
    out
}

fn diff_tree(repo: &GitRepo, requests: &[(String, Option<String>)]) -> Result<Vec<u8>, SeedError> {
    let mut child = repo
        .command()
        .args([
            "diff-tree",
            "--stdin",
            "-r",
            "--raw",
            "-z",
            "--no-abbrev",
            "--no-renames",
        ])
        .args(["--root", "--always"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| SeedError(format!("cannot run git diff-tree: {e}")))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let request: Vec<u8> = requests
        .iter()
        .flat_map(|(c, p)| match p {
            Some(p) => format!("{c} {p}\n").into_bytes(),
            None => format!("{c}\n").into_bytes(),
        })
        .collect();
    let writer = std::thread::spawn(move || stdin.write_all(&request));
    let out = child.wait_with_output();
    let _ = writer.join();
    let out = out.map_err(|e| SeedError(format!("git diff-tree: {e}")))?;
    if !out.status.success() {
        return Err(SeedError("git diff-tree failed".to_string()));
    }
    Ok(out.stdout)
}

/// Pair each answer with its request, collect the wanted paths each
/// (commit, parent) changed, and the earliest time each `.org` blob appears.
fn parse_diffs(
    out: &[u8],
    requests: &[(String, Option<String>)],
    graph: &HashMap<String, Commit>,
    wanted: &HashSet<&str>,
) -> Result<(Changed, HashMap<String, i64>), SeedError> {
    let bad = |what: &str| SeedError(format!("unexpected git diff-tree output: {what:?}"));
    let mut changed: Changed = HashMap::new();
    let mut introduced: HashMap<String, i64> = HashMap::new();
    let mut tokens = out
        .split(|b| *b == 0)
        .map(|t| String::from_utf8_lossy(t).into_owned());
    let mut request = requests.iter();
    let mut current: Option<(&(String, Option<String>), i64)> = None;
    while let Some(token) = tokens.next() {
        if token.is_empty() {
            continue;
        }
        let Some(meta) = token.strip_prefix(':') else {
            // A header: the next request's answer begins.
            let key = request
                .next()
                .ok_or_else(|| bad("more answers than requests"))?;
            if token != key.0 {
                return Err(bad(&token));
            }
            changed.insert(key.clone(), HashSet::new());
            current = Some((key, graph[&key.0].time));
            continue;
        };
        let (key, time) = current.ok_or_else(|| bad("a change before any header"))?;
        let path = tokens.next().ok_or_else(|| bad(meta))?;
        let fields: Vec<&str> = meta.split(' ').collect();
        let [_, dst_mode, _, dst_sha, status] = fields.as_slice() else {
            return Err(bad(meta));
        };
        let adds_blob =
            matches!(*status, "A" | "M" | "T") && matches!(*dst_mode, "100644" | "100755");
        if adds_blob && path.ends_with(".org") {
            let e = introduced.entry(dst_sha.to_string()).or_insert(time);
            *e = (*e).min(time);
        }
        if wanted.contains(path.as_str()) {
            if let Some(set) = changed.get_mut(key) {
                set.insert(path);
            }
        }
    }
    if request.next().is_some() {
        return Err(bad("fewer answers than requests"));
    }
    Ok((changed, introduced))
}

/// `git log -1 -- PATH` at `rev`: follow the first parent `path` is unchanged
/// against; the first commit changed against every parent introduced it.
fn last_change(
    rev: &str,
    path: &str,
    graph: &HashMap<String, Commit>,
    changed: &Changed,
) -> Result<i64, SeedError> {
    let mut at = rev.to_string();
    loop {
        let c = graph
            .get(&at)
            .ok_or_else(|| SeedError(format!("commit {at} is not in the history walk")))?;
        let changed_against = |parent: &String| {
            changed
                .get(&(at.clone(), Some(parent.clone())))
                .is_some_and(|d| d.contains(path))
        };
        match c.parents.iter().find(|p| !changed_against(p)) {
            Some(p) => at = p.clone(),
            None => return Ok(c.time),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> HashMap<String, Commit> {
        HashMap::from([
            (
                "aa".to_string(),
                Commit {
                    time: 10,
                    parents: vec![],
                },
            ),
            (
                "bb".to_string(),
                Commit {
                    time: 20,
                    parents: vec!["aa".to_string()],
                },
            ),
        ])
    }

    fn requests() -> Vec<(String, Option<String>)> {
        vec![("aa".into(), None), ("bb".into(), Some("aa".into()))]
    }

    const E: &str = ":000000 100644 0000 1111 A";

    #[test]
    fn answers_pair_with_requests_and_a_mismatch_is_an_error() {
        let wanted = HashSet::from(["p.org"]);
        let ok = format!("aa\0{E}\0p.org\0bb\0");
        let (changed, introduced) =
            parse_diffs(ok.as_bytes(), &requests(), &graph(), &wanted).unwrap();
        assert!(changed[&("aa".to_string(), None)].contains("p.org"));
        assert!(changed[&("bb".to_string(), Some("aa".to_string()))].is_empty());
        assert_eq!(introduced["1111"], 10);
        for bad in [
            format!("bb\0{E}\0p.org\0aa\0"),          // answers out of order
            format!("aa\0{E}\0p.org\0"),              // an answer missing
            "aa\0bb\0bb\0".to_string(),               // an extra answer
            format!("{E}\0p.org\0aa\0bb\0"),          // a change before any header
            "aa\0:100644 A\0p.org\0bb\0".to_string(), // a short entry
        ] {
            assert!(
                parse_diffs(bad.as_bytes(), &requests(), &graph(), &wanted).is_err(),
                "{bad:?}"
            );
        }
    }
}
