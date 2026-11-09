//! Reading one commit of a repository through the `git` binary: the commit's
//! tree and blob bytes, never the working copy.
//!
//! A tagged commit is the unit of input because the source's own gate passed on
//! exactly those bytes. A working copy can hold edits the gate never saw.
//!
//! @ai-caution: [git-env] Every invocation strips the `GIT_*` variables that
//! redirect git to another repository or index. A seeder run from inside a git
//! hook, or from a worktree build that exported `GIT_DIR`, would otherwise read a
//! different repository than the one named, and report on it with full
//! confidence. That leak has already happened once in this project (a worktree
//! commit's `GIT_DIR` reached a build).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::SeedError;

/// Variables that point git at a different repository, index or object store.
const REDIRECTING_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
];

/// Attributes that make a checkout's bytes differ from the blob's. `text`
/// alone does not: with LF-only content and no `eol`, the checkout is LF.
pub(crate) const CHECKOUT_ATTRS: &[&str] =
    &["eol", "crlf", "filter", "working-tree-encoding", "ident"];

/// One `ls-tree -r` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// `100644`, `100755`, `120000` (symlink), `160000` (submodule).
    pub mode: String,
    /// `blob` or `commit`.
    pub kind: String,
    pub sha: String,
    /// The path, or its raw bytes when they are not UTF-8.
    pub path: Result<String, Vec<u8>>,
}

/// A local repository, addressed by path.
#[derive(Debug, Clone)]
pub struct GitRepo {
    dir: PathBuf,
}

impl GitRepo {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        GitRepo { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub(super) fn command(&self) -> Command {
        let mut c = Command::new("git");
        c.arg("-C").arg(&self.dir);
        for var in REDIRECTING_ENV {
            c.env_remove(var);
        }
        c.stdin(Stdio::null());
        c
    }

    fn run(&self, args: &[&str]) -> Result<Vec<u8>, SeedError> {
        let out = self
            .command()
            .args(args)
            .output()
            .map_err(|e| SeedError(format!("cannot run git: {e}")))?;
        if !out.status.success() {
            return Err(SeedError(format!(
                "git {} failed in {}: {}",
                args.first().unwrap_or(&""),
                self.dir.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(out.stdout)
    }

    fn rev_parse(&self, spec: &str) -> Result<String, SeedError> {
        let out = self.run(&["rev-parse", "--verify", "--end-of-options", spec])?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }

    /// `(commit, tree)` for `rev`, which must name a commit (a tag is peeled).
    pub fn resolve(&self, rev: &str) -> Result<(String, String), SeedError> {
        if rev.is_empty() || rev.starts_with('-') {
            return Err(SeedError(format!("--rev {rev:?} is not a revision")));
        }
        let commit = self
            .rev_parse(&format!("{rev}^{{commit}}"))
            .map_err(|e| SeedError(format!("--rev {rev:?} does not name a commit: {e}")))?;
        let tree = self.rev_parse(&format!("{commit}^{{tree}}"))?;
        Ok((commit, tree))
    }

    /// Every entry of `commit`'s tree, recursively, in git's order.
    pub fn ls_tree(&self, commit: &str) -> Result<Vec<TreeEntry>, SeedError> {
        let out = self.run(&["ls-tree", "-r", "-z", "--full-tree", commit])?;
        out.split(|b| *b == 0)
            .filter(|rec| !rec.is_empty())
            .map(parse_ls_tree_record)
            .collect()
    }

    /// The bytes of each blob in `shas`, in the same order.
    pub fn read_blobs(&self, shas: &[String]) -> Result<Vec<Vec<u8>>, SeedError> {
        let mut child = self
            .command()
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| SeedError(format!("cannot run git cat-file: {e}")))?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let request: Vec<u8> = shas
            .iter()
            .flat_map(|s| format!("{s}\n").into_bytes())
            .collect();
        // Written from a thread: cat-file's output can fill its pipe before all
        // the requests are written, and then both sides would wait forever.
        let writer = std::thread::spawn(move || stdin.write_all(&request));
        let mut reader = BufReader::new(child.stdout.take().expect("piped stdout"));
        let blobs = shas
            .iter()
            .map(|sha| read_batch_object(&mut reader, sha))
            .collect::<Result<Vec<_>, _>>();
        let _ = writer.join();
        let _ = child.wait();
        blobs
    }

    /// For every path changed in `commit`'s history, the latest committer time
    /// (unix seconds) of a commit that changed it. ONE `git log` pass over the
    /// whole history, not one per file.
    ///
    /// Committer time, because it is when the change entered the history the
    /// tag names (an author time survives a rebase). The latest time, not the
    /// first commit met in log order, so clock skew between branches cannot
    /// pick an older commit. A merge commit lists no files (no `-m`), so a file
    /// changed on a merged branch takes that branch commit's time.
    pub fn last_commit_times(&self, commit: &str) -> Result<HashMap<String, i64>, SeedError> {
        let out = self.run(&[
            "log",
            "--format=format:%x01%ct",
            "--name-only",
            "-z",
            "--no-renames",
            "--no-show-signature",
            commit,
            "--",
        ])?;
        Ok(parse_log_times(&out))
    }

    /// Checkout-affecting attributes set on `paths` by the `.gitattributes`
    /// files of `commit` itself (not the working copy's). Needs git >= 2.40.
    pub fn checkout_attributes(
        &self,
        commit: &str,
        paths: &[String],
    ) -> Result<Vec<(String, String, String)>, SeedError> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let source = format!("--source={commit}");
        let mut args = vec!["check-attr", "-z", "--stdin", source.as_str()];
        args.extend(CHECKOUT_ATTRS);
        let mut child = self
            .command()
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| SeedError(format!("cannot run git check-attr: {e}")))?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let request: Vec<u8> = paths
            .iter()
            .flat_map(|p| [p.as_bytes(), b"\0"].concat())
            .collect();
        let writer = std::thread::spawn(move || stdin.write_all(&request));
        let mut out = Vec::new();
        let read = child
            .stdout
            .take()
            .expect("piped stdout")
            .read_to_end(&mut out);
        let _ = writer.join();
        let status = child
            .wait()
            .map_err(|e| SeedError(format!("git check-attr: {e}")))?;
        if read.is_err() || !status.success() {
            return Err(SeedError(
                "git check-attr --source failed (it needs git >= 2.40)".to_string(),
            ));
        }
        Ok(parse_check_attr(&out))
    }
}

/// `MODE SP TYPE SP SHA TAB PATH`.
fn parse_ls_tree_record(rec: &[u8]) -> Result<TreeEntry, SeedError> {
    let bad = || {
        SeedError(format!(
            "unexpected ls-tree output: {}",
            String::from_utf8_lossy(rec)
        ))
    };
    let tab = rec.iter().position(|b| *b == b'\t').ok_or_else(bad)?;
    let meta = std::str::from_utf8(&rec[..tab]).map_err(|_| bad())?;
    let mut parts = meta.split(' ');
    let (Some(mode), Some(kind), Some(sha), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(bad());
    };
    let raw = rec[tab + 1..].to_vec();
    Ok(TreeEntry {
        mode: mode.to_string(),
        kind: kind.to_string(),
        sha: sha.to_string(),
        path: String::from_utf8(raw).map_err(|e| e.into_bytes()),
    })
}

/// One `cat-file --batch` answer: `SHA SP TYPE SP SIZE LF CONTENTS LF`.
fn read_batch_object(reader: &mut impl BufRead, sha: &str) -> Result<Vec<u8>, SeedError> {
    let mut header = String::new();
    reader
        .read_line(&mut header)
        .map_err(|e| SeedError(format!("git cat-file: {e}")))?;
    let fields: Vec<&str> = header.trim_end().split(' ').collect();
    let [got, "blob", size] = fields.as_slice() else {
        return Err(SeedError(format!(
            "git cat-file: object {sha} is not a blob: {header:?}"
        )));
    };
    if *got != sha {
        return Err(SeedError(format!("git cat-file answered {got} for {sha}")));
    }
    let size: usize = size
        .parse()
        .map_err(|_| SeedError(format!("git cat-file: bad size in {header:?}")))?;
    let mut buf = vec![0; size + 1];
    reader
        .read_exact(&mut buf)
        .map_err(|e| SeedError(format!("git cat-file: short read of {sha}: {e}")))?;
    buf.pop(); // the LF after the contents
    Ok(buf)
}

/// `\x01CT \n PATH \0 PATH \0 … \0` per commit (the first path follows the
/// time after a newline; a commit with no files has none).
fn parse_log_times(out: &[u8]) -> HashMap<String, i64> {
    let mut times: HashMap<String, i64> = HashMap::new();
    let mut current: Option<i64> = None;
    for token in out.split(|b| *b == 0) {
        let path = match token.strip_prefix(b"\x01") {
            Some(rest) => {
                let (ct, first) = match rest.iter().position(|b| *b == b'\n') {
                    Some(nl) => (&rest[..nl], &rest[nl + 1..]),
                    None => (rest, &rest[rest.len()..]),
                };
                current = std::str::from_utf8(ct)
                    .ok()
                    .and_then(|s| s.trim().parse().ok());
                first
            }
            None => token,
        };
        let (Some(t), Ok(p)) = (current, std::str::from_utf8(path)) else {
            continue;
        };
        if !p.is_empty() {
            let slot = times.entry(p.to_string()).or_insert(t);
            *slot = (*slot).max(t);
        }
    }
    times
}

/// `PATH NUL ATTR NUL VALUE NUL`, repeated; keeps only attributes that are set.
fn parse_check_attr(out: &[u8]) -> Vec<(String, String, String)> {
    let fields: Vec<String> = out
        .split(|b| *b == 0)
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .collect();
    let (triples, _) = fields.as_chunks::<3>();
    triples
        .iter()
        .filter(|[_, _, value]| value != "unspecified" && value != "unset")
        .map(|[path, attr, value]| (path.clone(), attr.clone(), value.clone()))
        .collect()
}
