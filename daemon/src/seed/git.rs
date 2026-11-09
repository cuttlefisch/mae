//! Reading one commit of a repository through the `git` binary: the commit's
//! tree and blob bytes, never the working copy.
//!
//! A tagged commit is the unit of input because the source's own gate passed on
//! exactly those bytes. A working copy can hold edits the gate never saw.
//!
//! @ai-caution: [git-env] Every git call goes through [`GitRepo::command`], which
//! starts from an EMPTY environment and pins the configuration that could change
//! an answer or run code. A repository is input, and its own config is attacker
//! controlled: `core.fsmonitor` runs a command (confirmed: `check-attr` ran one),
//! replace refs substitute blobs, and a partial clone fetches over the network.
//! The caller's environment is no safer: `GIT_DIR` redirects git to another
//! repository (that leak has already happened once in this project, through a
//! worktree build), and `GIT_CONFIG_PARAMETERS` injects config. Do not add a git
//! call that bypasses `command()`.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::SeedError;

#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

/// The only variables git inherits from the caller: where to find programs and
/// temporary space. Everything else, every `GIT_*` and `HOME` included, is
/// dropped.
const INHERITED_ENV: &[&str] = &["PATH", "SYSTEMROOT", "TMPDIR", "TEMP", "TMP"];

/// Set on every call.
pub(super) const FORCED_ENV: &[(&str, &str)] = &[
    // No system or global config: the operator's settings must not change
    // what the plan says.
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_CONFIG_GLOBAL", NULL_DEVICE),
    ("GIT_ATTR_NOSYSTEM", "1"),
    // `~` in a repository's `include.path` must not reach the operator's own
    // files (their config would come back in through the repository's).
    ("HOME", NULL_DEVICE),
    // No grafts file, wherever the repository says it is.
    ("GIT_GRAFT_FILE", NULL_DEVICE),
    // The objects are the commit's own: no replace refs, no lazy fetch.
    ("GIT_NO_REPLACE_OBJECTS", "1"),
    ("GIT_NO_LAZY_FETCH", "1"),
    ("GIT_TERMINAL_PROMPT", "0"),
    // Reads only: no opportunistic index refresh.
    ("GIT_OPTIONAL_LOCKS", "0"),
    // Messages the code matches on (`skipped`) are git's English ones.
    ("LC_ALL", "C"),
];

/// `-c` overrides on every call: they outrank the repository's own config.
pub(super) const FORCED_CONFIG: &[(&str, &str)] = &[
    ("core.fsmonitor", "false"),
    ("core.hooksPath", NULL_DEVICE),
    ("core.sshCommand", "false"),
    ("protocol.allow", "never"),
    ("core.attributesFile", NULL_DEVICE),
    ("core.autocrlf", "false"),
    ("core.eol", "lf"),
    ("core.quotePath", "false"),
    ("log.showRoot", "true"),
    ("log.showSignature", "false"),
    // The commit-graph and multi-pack-index files are caches the repository
    // supplies; a forged commit-graph rewrote parents and trees (confirmed).
    // Read the objects themselves.
    ("core.commitGraph", "false"),
    ("core.multiPackIndex", "false"),
    // GIT_GRAFT_FILE (above) makes git print a deprecation hint into every
    // error it reports; the hint is noise in a refusal.
    ("advice.graftFileDeprecated", "false"),
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

    /// The one way this module runs git: an empty environment plus
    /// [`INHERITED_ENV`] and [`FORCED_ENV`], and [`FORCED_CONFIG`] as `-c`.
    pub(super) fn command(&self) -> Command {
        let mut c = Command::new("git");
        c.env_clear();
        for var in INHERITED_ENV {
            if let Some(v) = std::env::var_os(var) {
                c.env(var, v);
            }
        }
        c.envs(FORCED_ENV.iter().copied());
        c.arg("--no-replace-objects").arg("-C").arg(&self.dir);
        for (key, value) in FORCED_CONFIG {
            c.arg("-c").arg(format!("{key}={value}"));
        }
        c.stdin(Stdio::null());
        c
    }

    /// Refuse a repository whose objects or attributes are not the commit's
    /// alone, or a path that is not the repository's top level.
    pub fn preflight(&self) -> Result<(), SeedError> {
        self.refuse_includes()?;
        self.require_top_level()?;
        let refuse = |why: String| Err(SeedError(format!("{}: {why}", self.dir.display())));
        if self.rev_parse_flag("--is-shallow-repository")? == "true" {
            return refuse("a shallow clone: its history is cut, so dates would be wrong".into());
        }
        let partial = self.config_matches(r"^(extensions\.partialclone|remote\..*\.promisor)$")?;
        if partial.iter().any(|v| v != "false") {
            return refuse(
                "a partial clone: blobs may be missing or fetched over the network".into(),
            );
        }
        for (file, why) in [
            ("info/attributes", "sets attributes outside the commit"),
            ("info/grafts", "rewrites history outside the commit"),
        ] {
            // Bytes, not text: a file that is not UTF-8 is still read by git
            // (a non-UTF-8 grafts file passed a text check and was applied).
            // Any byte at all refuses; comments are not worth the parser.
            let path = self.git_path(file)?;
            match std::fs::read(&path) {
                Ok(bytes) if bytes.is_empty() => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Ok(_) => return refuse(format!("{} is not empty: it {why}", path.display())),
                Err(e) => return refuse(format!("cannot read {}: {e}", path.display())),
            }
        }
        Ok(())
    }

    /// Hash-check every object that decides WHICH content and WHICH dates the
    /// plan carries: the tag chain from what `rev` names down to `commit`,
    /// then every commit and every tree reachable from `commit`
    /// (`rev-list --objects --filter=blob:none`). git does not check a loose or
    /// packed object against its name, so a forged subtree (confirmed) or a
    /// forged historical commit (confirmed: a rewritten root dated a page 1998)
    /// would otherwise be read as real. Blobs are checked where they are read
    /// (`plan`). Returns how many objects were checked.
    pub fn verify_objects(
        &self,
        rev: &str,
        commit: &str,
        format: &str,
    ) -> Result<usize, SeedError> {
        let check = |sha: &str, kind: &str, bytes: &[u8]| {
            if object_name(format, kind, bytes) == sha {
                Ok(())
            } else {
                Err(SeedError(format!(
                    "{kind} {sha} does not hash to its name: the repository is corrupt or forged"
                )))
            }
        };
        let mut checked = 0;
        let mut oid = self.rev_parse(rev)?;
        loop {
            let mut next = None;
            self.for_each_object(
                std::slice::from_ref(&oid),
                usize::MAX,
                |sha, kind, bytes| {
                    check(sha, kind, bytes)?;
                    if kind == "tag" {
                        next = tag_target(bytes);
                        if next.is_none() {
                            return Err(SeedError(format!("tag {sha} names no object")));
                        }
                    }
                    Ok(())
                },
            )?;
            checked += 1;
            match next {
                Some(target) => oid = target,
                None => break,
            }
        }
        if oid != commit {
            return Err(SeedError(format!(
                "--rev {rev:?} does not lead to commit {commit}"
            )));
        }
        let listing =
            self.run_checked(&["rev-list", "--objects", "--filter=blob:none", commit, "--"])?;
        let shas: Vec<String> = String::from_utf8_lossy(&listing)
            .lines()
            .filter_map(|l| l.split(' ').next())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        self.for_each_object(&shas, usize::MAX, |sha, kind, bytes| {
            check(sha, kind, bytes)
        })?;
        Ok(checked + shas.len())
    }

    /// The repository's object format: `sha1` or `sha256`.
    pub fn object_format(&self) -> Result<String, SeedError> {
        let format = self.rev_parse_flag("--show-object-format")?;
        match format.as_str() {
            "sha1" | "sha256" => Ok(format),
            other => Err(SeedError(format!("unsupported object format {other:?}"))),
        }
    }

    /// Refuse a repository whose own config (common or worktree) has an
    /// `[include]` or `[includeIf …]` section. An include reads a file outside
    /// the repository, and `~user/` in its path expands through the password
    /// database to any account's home, the operator's included, which the
    /// pinned `HOME` cannot stop.
    ///
    /// Read from the files themselves, BEFORE any git runs: git follows
    /// includes while loading its own configuration, so even
    /// `git config --no-includes` opened `~root/…` (measured).
    fn refuse_includes(&self) -> Result<(), SeedError> {
        for file in config_files(&self.dir) {
            let Ok(bytes) = std::fs::read(&file) else {
                continue;
            };
            if let Some(header) = include_header(&bytes) {
                return Err(SeedError(format!(
                    "{}: the repository's config includes other files ({header} in {}): an \
                     include reads files outside the repository, including an operator's own \
                     config through `~user/`; remove it to seed",
                    self.dir.display(),
                    file.display()
                )));
            }
        }
        Ok(())
    }

    /// The path must be the work tree's top level (or a bare repository's own
    /// directory): attributes and paths are read relative to it.
    fn require_top_level(&self) -> Result<(), SeedError> {
        let bare = self.rev_parse_flag("--is-bare-repository")? == "true";
        let top = if bare {
            self.rev_parse_flag("--absolute-git-dir")?
        } else {
            self.rev_parse_flag("--show-toplevel")?
        };
        let same = match (
            std::fs::canonicalize(&self.dir),
            std::fs::canonicalize(&top),
        ) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        if !same {
            return Err(SeedError(format!(
                "--git {} is not the repository's top level ({top}); name the top level",
                self.dir.display()
            )));
        }
        Ok(())
    }

    fn rev_parse_flag(&self, flag: &str) -> Result<String, SeedError> {
        self.rev_parse_args(&[flag])
    }

    fn rev_parse_args(&self, args: &[&str]) -> Result<String, SeedError> {
        let mut all = vec!["rev-parse"];
        all.extend_from_slice(args);
        let out = self.run(&all)?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }

    /// `file` inside the repository's common git directory, where `info/`
    /// lives. Deliberately NOT `rev-parse --git-path`: that honours the very
    /// overrides the pinned environment sets (`GIT_GRAFT_FILE`), and would
    /// name the null device instead of the file the repository carries.
    fn git_path(&self, file: &str) -> Result<PathBuf, SeedError> {
        let common = PathBuf::from(self.rev_parse_flag("--git-common-dir")?);
        let common = if common.is_absolute() {
            common
        } else {
            self.dir.join(common)
        };
        Ok(common.join(file))
    }

    /// Values of the repository's config keys matching `regex` (none is fine).
    fn config_matches(&self, regex: &str) -> Result<Vec<String>, SeedError> {
        let out = self
            .command()
            .args(["config", "--get-regexp", regex])
            .output()
            .map_err(|e| SeedError(format!("cannot run git: {e}")))?;
        match out.status.code() {
            Some(0) => Ok(String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| {
                    l.split_once(' ')
                        .map_or("", |(_, v)| v)
                        .trim()
                        .to_ascii_lowercase()
                })
                .collect()),
            Some(1) => Ok(Vec::new()),
            _ => Err(SeedError(format!(
                "git config failed in {}: {}",
                self.dir.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            ))),
        }
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
        let mut blobs = Vec::with_capacity(shas.len());
        self.for_each_blob(shas, |bytes| blobs.push(bytes.to_vec()))?;
        Ok(blobs)
    }

    /// Hand each blob in `shas` to `f`, in order, from ONE `cat-file --batch`
    /// process, holding one blob in memory at a time.
    pub fn for_each_blob(&self, shas: &[String], f: impl FnMut(&[u8])) -> Result<(), SeedError> {
        self.for_each_blob_prefix(shas, usize::MAX, f)
    }

    /// Like [`Self::for_each_blob`], but only the first `max` bytes of each
    /// blob are held; the rest is read from the pipe and dropped.
    pub fn for_each_blob_prefix(
        &self,
        shas: &[String],
        max: usize,
        mut f: impl FnMut(&[u8]),
    ) -> Result<(), SeedError> {
        self.for_each_object(shas, max, |sha, kind, bytes| {
            if kind != "blob" {
                return Err(SeedError(format!("object {sha} is a {kind}, not a blob")));
            }
            f(bytes);
            Ok(())
        })
    }

    /// Hand each object in `shas` (any type) to `f` with its type, in order,
    /// from ONE `cat-file --batch` process, holding at most `max` bytes of one
    /// object at a time. `f` may stop the pass with an error.
    pub fn for_each_object(
        &self,
        shas: &[String],
        max: usize,
        mut f: impl FnMut(&str, &str, &[u8]) -> Result<(), SeedError>,
    ) -> Result<(), SeedError> {
        let mut child = self
            .command()
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
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
        let mut result = Ok(());
        for sha in shas {
            let step = read_batch_object(&mut reader, sha, max)
                .and_then(|(kind, bytes)| f(sha, &kind, &bytes));
            if let Err(e) = step {
                result = Err(e);
                break;
            }
        }
        drop(reader);
        let _ = writer.join();
        let _ = child.wait();
        result
    }

    /// The size of each blob in `shas`, in order, from ONE
    /// `cat-file --batch-check` process, reading no contents.
    pub fn blob_sizes(&self, shas: &[String]) -> Result<Vec<u64>, SeedError> {
        if shas.is_empty() {
            return Ok(Vec::new());
        }
        let mut child = self
            .command()
            .args([
                "cat-file",
                "--batch-check=%(objectname) %(objecttype) %(objectsize)",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| SeedError(format!("cannot run git cat-file: {e}")))?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let request: Vec<u8> = shas
            .iter()
            .flat_map(|s| format!("{s}\n").into_bytes())
            .collect();
        let writer = std::thread::spawn(move || stdin.write_all(&request));
        let mut out = String::new();
        let read = child
            .stdout
            .take()
            .expect("piped stdout")
            .read_to_string(&mut out);
        let _ = writer.join();
        let _ = child.wait();
        read.map_err(|e| SeedError(format!("git cat-file --batch-check: {e}")))?;
        let sizes: Vec<u64> = shas
            .iter()
            .zip(out.lines())
            .map(
                |(sha, line)| match line.split(' ').collect::<Vec<_>>().as_slice() {
                    [got, "blob", size] if got == sha => size.parse().ok(),
                    _ => None,
                },
            )
            .collect::<Option<_>>()
            .ok_or_else(|| {
                SeedError("git cat-file --batch-check: unexpected answer".to_string())
            })?;
        if sizes.len() != shas.len() {
            return Err(SeedError(
                "git cat-file --batch-check: missing answers".to_string(),
            ));
        }
        Ok(sizes)
    }

    /// Like `run`, but a warning that git skipped part of its work (rename
    /// detection past its limit) is an error rather than a quieter answer.
    pub(super) fn run_checked(&self, args: &[&str]) -> Result<Vec<u8>, SeedError> {
        let out = self
            .command()
            .args(args)
            .output()
            .map_err(|e| SeedError(format!("cannot run git: {e}")))?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !out.status.success() || stderr.contains("skipped") {
            return Err(SeedError(format!(
                "git {} in {}: {}",
                args.first().unwrap_or(&""),
                self.dir.display(),
                stderr.trim()
            )));
        }
        Ok(out.stdout)
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

/// One `cat-file --batch` answer: `SHA SP TYPE SP SIZE LF CONTENTS LF`. At
/// most `max` bytes of the contents are kept; the rest is consumed and dropped.
fn read_batch_object(
    reader: &mut impl BufRead,
    sha: &str,
    max: usize,
) -> Result<(String, Vec<u8>), SeedError> {
    let mut header = String::new();
    reader
        .read_line(&mut header)
        .map_err(|e| SeedError(format!("git cat-file: {e}")))?;
    let fields: Vec<&str> = header.trim_end().split(' ').collect();
    let [got, kind, size] = fields.as_slice() else {
        return Err(SeedError(format!(
            "git cat-file: object {sha} is missing or unreadable: {:?}",
            header.trim_end()
        )));
    };
    if *got != sha {
        return Err(SeedError(format!("git cat-file answered {got} for {sha}")));
    }
    let size: usize = size
        .parse()
        .map_err(|_| SeedError(format!("git cat-file: bad size in {header:?}")))?;
    let keep = size.min(max);
    let mut buf = vec![0; keep];
    let short = |e: std::io::Error| SeedError(format!("git cat-file: short read of {sha}: {e}"));
    reader.read_exact(&mut buf).map_err(short)?;
    // The rest of the contents, and the LF after them.
    let rest = (size - keep) as u64 + 1;
    let skipped =
        std::io::copy(&mut reader.by_ref().take(rest), &mut std::io::sink()).map_err(short)?;
    if skipped != rest {
        return Err(SeedError(format!("git cat-file: short read of {sha}")));
    }
    Ok((kind.to_string(), buf))
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

/// The object name git would give `content` as a blob under `format`.
pub fn blob_name(format: &str, content: &[u8]) -> String {
    object_name(format, "blob", content)
}

/// The object name git would give `content` as an object of type `kind`
/// under `format` (`sha1` or `sha256`): the hash of `<kind> <len>\0<content>`.
pub fn object_name(format: &str, kind: &str, content: &[u8]) -> String {
    let header = format!("{kind} {}\0", content.len());
    if format == "sha256" {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(header.as_bytes());
        h.update(content);
        hex::encode(h.finalize())
    } else {
        let mut h = sha1_smol::Sha1::new();
        h.update(header.as_bytes());
        h.update(content);
        h.digest().to_string()
    }
}

/// The `object <name>` a tag object points at.
fn tag_target(tag: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(tag).ok()?;
    text.lines()
        .next()?
        .strip_prefix("object ")
        .map(|s| s.trim().to_string())
}

/// The config files a repository at `dir` (a work tree's top level, a linked
/// worktree, or a bare repository) carries, found without running git.
fn config_files(dir: &Path) -> Vec<PathBuf> {
    let dot_git = dir.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else if let Ok(text) = std::fs::read_to_string(&dot_git) {
        // A linked worktree: `.git` is a file naming its git directory.
        match text.trim().strip_prefix("gitdir:") {
            Some(p) => dir.join(p.trim()),
            None => return Vec::new(),
        }
    } else {
        dir.to_path_buf() // bare
    };
    let common = std::fs::read_to_string(git_dir.join("commondir"))
        .map(|c| git_dir.join(c.trim()))
        .unwrap_or_else(|_| git_dir.clone());
    vec![common.join("config"), git_dir.join("config.worktree")]
}

/// The first section header naming `include` or `includeIf` (any case, any
/// subsection) in a config file's bytes.
fn include_header(config: &[u8]) -> Option<String> {
    config.split(|b| *b == b'\n').find_map(|line| {
        let line = String::from_utf8_lossy(line);
        let header = line.trim_start().strip_prefix('[')?;
        let name = header.trim_start();
        name.get(..7)
            .filter(|n| n.eq_ignore_ascii_case("include"))
            .map(|_| format!("[{}", header.split(']').next().unwrap_or(header)) + "]")
    })
}
