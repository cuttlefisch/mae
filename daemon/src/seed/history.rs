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
//!
//! **Historical blobs are not hash-checked.** A prefix cannot be verified
//! against an object name without reading the whole blob, which is what the
//! prefix read avoids. A forged historical object can therefore change an
//! id's date, and nothing else: content, fields and bodies come only from the
//! selected blobs at the rev, each of which IS verified in full (`plan`).
//! Paths are compared as raw bytes; only `*.org` entries are read.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
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
    /// How many `*.org` diff entries the tree-diff pass parsed.
    pub diff_entries: usize,
}

/// One commit: its committer time and parents, in order.
struct Commit {
    time: i64,
    parents: Vec<String>,
}

/// commit → parent (`""` for a root's empty tree) → the wanted paths changed,
/// as raw bytes. Keyed by the request itself, never by position (requests are
/// sorted, parents are not), and by bytes, never by a lossy decoding (a
/// historical `a\xff.org` must not count as a change to a real `a\u{FFFD}.org`).
type Changed = HashMap<String, HashMap<String, HashSet<Vec<u8>>>>;

/// What the diff pass collected.
struct Diffs {
    changed: Changed,
    /// Each `.org` blob any diff introduces: the earliest commit time.
    introduced: HashMap<String, i64>,
    /// How many diff entries were parsed (the pass's cost).
    entries: usize,
}

/// The history of `commit` for the pages at `wanted` paths.
pub fn history(repo: &GitRepo, commit: &str, wanted: &[String]) -> Result<History, SeedError> {
    let graph = commit_graph(repo, commit)?;
    let requests = diff_requests(&graph);
    let wanted_set: HashSet<&[u8]> = wanted.iter().map(|p| p.as_bytes()).collect();
    let diffs = diff_tree(repo, &requests, |tokens| {
        parse_diffs(tokens, &requests, &graph, &wanted_set)
    })?;
    let mut modified = HashMap::new();
    for path in wanted {
        let t = last_change(commit, path.as_bytes(), &graph, &diffs.changed)?;
        modified.insert(path.clone(), t);
    }
    let mut blobs: Vec<(String, i64)> = diffs.introduced.into_iter().collect();
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
        diff_entries: diffs.entries,
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

/// Run the batched diff, restricted to `*.org` paths, and hand its NUL
/// separated output to `consume` as it streams: nothing is buffered whole.
fn diff_tree<T>(
    repo: &GitRepo,
    requests: &[(String, Option<String>)],
    consume: impl FnOnce(&mut dyn Iterator<Item = std::io::Result<Vec<u8>>>) -> Result<T, SeedError>,
) -> Result<T, SeedError> {
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
        .args(["--root", "--always", "--", "*.org"])
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
    let reader = BufReader::new(child.stdout.take().expect("piped stdout"));
    let result = consume(&mut reader.split(0));
    let _ = writer.join();
    let status = child
        .wait()
        .map_err(|e| SeedError(format!("git diff-tree: {e}")))?;
    match result {
        Ok(_) if !status.success() => Err(SeedError("git diff-tree failed".to_string())),
        other => other,
    }
}

/// Pair each answer with its request, collect the wanted paths each
/// (commit, parent) changed, and the earliest time each `.org` blob appears.
fn parse_diffs(
    tokens: &mut dyn Iterator<Item = std::io::Result<Vec<u8>>>,
    requests: &[(String, Option<String>)],
    graph: &HashMap<String, Commit>,
    wanted: &HashSet<&[u8]>,
) -> Result<Diffs, SeedError> {
    let bad = |what: &[u8]| {
        SeedError(format!(
            "unexpected git diff-tree output: {:?}",
            String::from_utf8_lossy(what)
        ))
    };
    let mut diffs = Diffs {
        changed: HashMap::new(),
        introduced: HashMap::new(),
        entries: 0,
    };
    let mut request = requests.iter();
    let mut current: Option<(&str, &str, i64)> = None;
    while let Some(token) = tokens.next() {
        let token = token.map_err(|e| SeedError(format!("git diff-tree: {e}")))?;
        if token.is_empty() {
            continue;
        }
        let Some(meta) = token.strip_prefix(b":") else {
            // A header: the next request's answer begins.
            let (c, p) = request
                .next()
                .ok_or_else(|| bad(b"more answers than requests"))?;
            if token != c.as_bytes() {
                return Err(bad(&token));
            }
            let parent = p.as_deref().unwrap_or("");
            diffs
                .changed
                .entry(c.clone())
                .or_default()
                .insert(parent.to_string(), HashSet::new());
            current = Some((c, parent, graph[c].time));
            continue;
        };
        let (c, parent, time) = current.ok_or_else(|| bad(b"a change before any header"))?;
        let path = tokens
            .next()
            .ok_or_else(|| bad(meta))?
            .map_err(|e| SeedError(format!("git diff-tree: {e}")))?;
        let meta = std::str::from_utf8(meta).map_err(|_| bad(meta))?;
        let fields: Vec<&str> = meta.split(' ').collect();
        let [_, dst_mode, _, dst_sha, status] = fields.as_slice() else {
            return Err(bad(meta.as_bytes()));
        };
        diffs.entries += 1;
        let adds_blob =
            matches!(*status, "A" | "M" | "T") && matches!(*dst_mode, "100644" | "100755");
        if adds_blob && path.ends_with(b".org") {
            let e = diffs.introduced.entry(dst_sha.to_string()).or_insert(time);
            *e = (*e).min(time);
        }
        if wanted.contains(path.as_slice()) {
            if let Some(set) = diffs.changed.get_mut(c).and_then(|m| m.get_mut(parent)) {
                set.insert(path);
            }
        }
    }
    if request.next().is_some() {
        return Err(bad(b"fewer answers than requests"));
    }
    Ok(diffs)
}

/// `git log -1 -- PATH` at `rev`: follow the first parent `path` is unchanged
/// against; the first commit changed against every parent introduced it.
fn last_change<'a>(
    rev: &'a str,
    path: &[u8],
    graph: &'a HashMap<String, Commit>,
    changed: &Changed,
) -> Result<i64, SeedError> {
    let mut at: &'a str = rev;
    loop {
        let c = graph
            .get(at)
            .ok_or_else(|| SeedError(format!("commit {at} is not in the history walk")))?;
        let per_parent = changed.get(at);
        let changed_against = |parent: &str| {
            per_parent
                .and_then(|m| m.get(parent))
                .is_some_and(|d| d.contains(path))
        };
        match c.parents.iter().find(|p| !changed_against(p)) {
            Some(p) => at = p,
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

    fn parse(out: &str, wanted: &HashSet<&[u8]>) -> Result<Diffs, SeedError> {
        let mut tokens = out.as_bytes().split(|b| *b == 0).map(|t| Ok(t.to_vec()));
        parse_diffs(&mut tokens, &requests(), &graph(), wanted)
    }

    #[test]
    fn answers_pair_with_requests_and_a_mismatch_is_an_error() {
        let wanted: HashSet<&[u8]> = HashSet::from([&b"p.org"[..]]);
        let ok = format!("aa\0{E}\0p.org\0bb\0");
        let diffs = parse(&ok, &wanted).unwrap();
        assert!(diffs.changed["aa"][""].contains(&b"p.org"[..]));
        assert!(diffs.changed["bb"]["aa"].is_empty());
        assert_eq!(diffs.introduced["1111"], 10);
        assert_eq!(diffs.entries, 1);
        for bad in [
            format!("bb\0{E}\0p.org\0aa\0"),          // answers out of order
            format!("aa\0{E}\0p.org\0"),              // an answer missing
            "aa\0bb\0bb\0".to_string(),               // an extra answer
            format!("{E}\0p.org\0aa\0bb\0"),          // a change before any header
            "aa\0:100644 A\0p.org\0bb\0".to_string(), // a short entry
        ] {
            assert!(parse(&bad, &wanted).is_err(), "{bad:?}");
        }
    }
}
