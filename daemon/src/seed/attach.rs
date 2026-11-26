//! A seeded page's attachments (ADR-115 P4: D7, Amendment 1 E1 and E2).
//!
//! **Which files.** Every `[[attachment:NAME]]` link in the page's text as the
//! seed stores it (N(F), `regen` of the split), read by mae-org's semantics,
//! the one parser every consumer derives from (ADR-112). mae-org says which
//! `:ID:` the link inherits and so which directory it names (E2): `data/
//! <ID[0:2]>/<ID[2:]>/` next to the page, the nearest enclosing entry's ID,
//! the file's own above every headline. The export reads the same links from
//! the same text with the same function ([`links`]), so the two directions
//! cannot disagree about where a file lives.
//!
//! **Path safety is ours** (mae-org builds the directory from the ID as
//! written and never validates it). A NAME must pass the store's own NAME rule
//! (`validate_attachment_name`: one portable file name, no separator, no `..`,
//! no device name), and every directory component built from an ID must be a
//! plain name: [`file_path`] refuses `..`, `.`, an empty component, a
//! backslash and a control character. The joined path is then looked up in
//! the commit's own tree, never on a filesystem, so a symlink is seen as a
//! symlink (mode `120000`) and refused, never followed.
//!
//! **What a node binds.** One NAME is one entry of the node's `attachments`
//! map, so two links of one page that name the same file once case and
//! Unicode normalization are ignored (`fold_key`), but live in different
//! directories (two headlines with their own IDs) or are spelled differently,
//! cannot both be bound: the seed refuses the page, naming both.
//!
//! @ai-caution: [kb-truth] Every case here is a named refusal (dry run lists
//! them all) and never a dropped image: a link the seed cannot carry stops
//! the seed. A new case either fits these rules or becomes a refusal.

use std::collections::{BTreeMap, HashMap};

use mae_sync::kb::{validate_attachment_name, AttachmentRef};

use super::git::GitRepo;
use super::plan::CheckoutAttr;
use super::{path_fold_key, Problem, ProblemKind, SeedError};
use crate::blob_store::process::{process, ProcessError, ProcessLimits};

/// The scheme of an attachment link, written as Org reads it: case matters.
const SCHEME: &str = "attachment:";

/// One `[[attachment:NAME]]` link of a page: its NAME as written, and the
/// directory mae-org derived for it (`None`: no `:ID:` up the outline, or a
/// `:DIR:`/`:ATTACH_DIR:`, which v1 does not support).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentLink {
    pub name: String,
    pub dir: Option<String>,
}

/// The attachment links of `text` (a whole page, drawer included, so the
/// file's own `:ID:` is inherited), in text order.
pub fn links(text: &str) -> Vec<AttachmentLink> {
    let doc = mae_org::parse(text, &mae_org::ParseOptions::default());
    mae_org::semantics(&doc)
        .links
        .into_iter()
        .filter_map(|l| {
            let name = l.target.strip_prefix(SCHEME)?.to_string();
            Some(AttachmentLink {
                name,
                dir: l.attach_dir,
            })
        })
        .collect()
}

/// The repository path of NAME in attachment directory `dir`, next to the
/// page at `page_path`; or why it is not a path the seed may read or an
/// export may write.
pub fn file_path(page_path: &str, dir: &str, name: &str) -> Result<String, String> {
    validate_attachment_name(name).map_err(|e| e.to_string())?;
    let inner = dir
        .strip_suffix('/')
        .ok_or_else(|| format!("the attachment directory {dir:?} is not a directory"))?;
    for part in inner.split('/') {
        let bad = part.is_empty()
            || part == "."
            || part == ".."
            || part.contains('\\')
            || part.chars().any(char::is_control);
        if bad {
            return Err(format!(
                "the attachment directory {dir:?} (from the entry's :ID:) is not a plain \
                 relative path"
            ));
        }
    }
    let parent = page_path.rsplit_once('/').map_or("", |(p, _)| p);
    Ok(match parent {
        "" => format!("{dir}{name}"),
        p => format!("{p}/{dir}{name}"),
    })
}

/// One NAME a page binds, and the repository path of its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub name: String,
    pub path: String,
}

/// The NAMEs `text` binds, by NAME, and every problem with its links. A NAME
/// linked twice to the same file is one entry.
pub fn wanted(page_path: &str, text: &str) -> (BTreeMap<String, Wanted>, Vec<Problem>) {
    let mut out: BTreeMap<String, Wanted> = BTreeMap::new();
    let mut by_key: HashMap<String, Wanted> = HashMap::new();
    let mut problems = Vec::new();
    for l in links(text) {
        let link = format!("[[attachment:{}]]", l.name);
        let Some(dir) = l.dir else {
            problems.push(Problem::new(
                ProblemKind::AttachmentDir,
                format!(
                    "{link}: no attachment directory: no :ID: up the outline, or a :DIR: or \
                     :ATTACH_DIR:, which seeding does not support (ADR-115 E2)"
                ),
            ));
            continue;
        };
        if let Err(e) = validate_attachment_name(&l.name) {
            problems.push(Problem::new(
                ProblemKind::AttachmentName,
                format!("{link}: {e}"),
            ));
            continue;
        }
        let path = match file_path(page_path, &dir, &l.name) {
            Ok(p) => p,
            Err(e) => {
                problems.push(Problem::new(
                    ProblemKind::AttachmentPath,
                    format!("{link}: {e}"),
                ));
                continue;
            }
        };
        let w = Wanted {
            name: l.name.clone(),
            path,
        };
        match by_key.get(&path_fold_key(&l.name)) {
            Some(seen) if *seen == w => {}
            Some(seen) => problems.push(Problem::new(
                ProblemKind::AttachmentCollision,
                format!(
                    "{link} ({}) and [[attachment:{}]] ({}) would be one NAME of this node: \
                     one attachment map entry cannot hold both",
                    w.path, seen.name, seen.path
                ),
            )),
            None => {
                by_key.insert(path_fold_key(&l.name), w.clone());
                out.insert(l.name, w);
            }
        }
    }
    problems.sort_by(|a, b| a.detail.cmp(&b.detail));
    problems.dedup();
    (out, problems)
}

/// One attachment a planned node binds: the file in the commit and what the
/// store's processing makes of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedAttachment {
    pub name: String,
    /// The repository path of the file.
    pub path: String,
    /// Its git blob.
    pub blob: String,
    /// The sha256 of the PROCESSED bytes: the address the apply stores it at.
    pub sha256: String,
    /// The size of the processed bytes.
    pub size: u64,
    pub media_type: String,
}

impl PlannedAttachment {
    /// The node's map entry.
    pub fn entry(&self) -> AttachmentRef {
        AttachmentRef {
            sha256: self.sha256.clone(),
            size: self.size,
            media_type: self.media_type.clone(),
        }
    }
}

/// The bounds a seed reads and processes attachment files under: the
/// daemon's `oauth.max_attachment_bytes` and `max_attachment_pixels`, so a
/// file the seed accepts is one an upload would have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttachmentLimits {
    pub max_bytes: u64,
    pub max_pixels: u64,
}

impl Default for AttachmentLimits {
    fn default() -> Self {
        AttachmentLimits {
            max_bytes: crate::blob_store::DEFAULT_MAX_ATTACHMENT_BYTES,
            max_pixels: crate::blob_store::DEFAULT_MAX_ATTACHMENT_PIXELS,
        }
    }
}

impl AttachmentLimits {
    pub fn process(&self) -> ProcessLimits {
        ProcessLimits {
            max_pixels: self.max_pixels,
            max_frames: crate::blob_store::process::MAX_FRAMES,
        }
    }
}

/// Where attachment files are read from: the commit's own objects (in
/// production, [`GitBlobs`]), or bytes in hand (tests).
pub trait BlobReader: Send + Sync {
    /// The size of each blob in `shas`, read without its content.
    fn sizes(&self, shas: &[String]) -> Result<Vec<u64>, SeedError>;
    /// Hand each blob in `shas` to `f`, in order, one at a time, each
    /// checked to hash to its name.
    fn for_each(&self, shas: &[String], f: &mut dyn FnMut(&str, &[u8])) -> Result<(), SeedError>;
    /// Checkout-affecting attributes the commit's own `.gitattributes` set
    /// on `paths` (an LFS `filter` above all: the commit then holds a
    /// pointer, not the image).
    fn checkout_attributes(&self, paths: &[String]) -> Result<Vec<CheckoutAttr>, SeedError>;
}

/// Bytes in hand, by blob name (tests, and `plan_entries` without a reader).
impl BlobReader for HashMap<String, Vec<u8>> {
    fn sizes(&self, shas: &[String]) -> Result<Vec<u64>, SeedError> {
        shas.iter()
            .map(|s| {
                self.get(s)
                    .map(|b| b.len() as u64)
                    .ok_or_else(|| SeedError(format!("no bytes for blob {s}")))
            })
            .collect()
    }

    fn for_each(&self, shas: &[String], f: &mut dyn FnMut(&str, &[u8])) -> Result<(), SeedError> {
        for s in shas {
            let bytes = self
                .get(s)
                .ok_or_else(|| SeedError(format!("no bytes for blob {s}")))?;
            f(s, bytes);
        }
        Ok(())
    }

    fn checkout_attributes(&self, _paths: &[String]) -> Result<Vec<CheckoutAttr>, SeedError> {
        Ok(Vec::new())
    }
}

/// A commit's blobs, read from its repository: one `cat-file` process per
/// call, one blob in memory at a time, each hash-checked (the object store is
/// input too).
pub struct GitBlobs {
    repo: GitRepo,
    commit: String,
    format: String,
}

impl GitBlobs {
    pub fn open(repo: &std::path::Path, commit: &str) -> Result<Self, SeedError> {
        let repo = GitRepo::new(repo);
        let format = repo.object_format()?;
        Ok(GitBlobs {
            repo,
            commit: commit.to_string(),
            format,
        })
    }
}

impl BlobReader for GitBlobs {
    fn sizes(&self, shas: &[String]) -> Result<Vec<u64>, SeedError> {
        self.repo.blob_sizes(shas)
    }

    fn for_each(&self, shas: &[String], f: &mut dyn FnMut(&str, &[u8])) -> Result<(), SeedError> {
        let mut i = 0;
        let mut bad = None;
        self.repo.for_each_blob(shas, |bytes| {
            let sha = &shas[i];
            i += 1;
            if super::git::blob_name(&self.format, bytes) != *sha {
                bad.get_or_insert_with(|| sha.clone());
                return;
            }
            f(sha, bytes);
        })?;
        match bad {
            Some(sha) => Err(SeedError(format!(
                "object {sha} does not hash to its name: the repository is corrupt or forged"
            ))),
            None => Ok(()),
        }
    }

    fn checkout_attributes(&self, paths: &[String]) -> Result<Vec<CheckoutAttr>, SeedError> {
        Ok(self
            .repo
            .checkout_attributes(&self.commit, paths)?
            .into_iter()
            .map(|(path, attr, value)| CheckoutAttr { path, attr, value })
            .collect())
    }
}

/// What processing made of one file: its processed description, or the
/// refusal it earns.
pub type Outcome = Result<(String, u64, String), Problem>;

/// Process one file's bytes as an upload would be (D3a), keeping only the
/// description: the sha256, size and media type of what would be stored.
pub fn describe(bytes: &[u8], limits: &AttachmentLimits) -> Outcome {
    match process(bytes.to_vec(), &limits.process()) {
        Ok(p) => Ok((
            super::plan::sha256_hex(&p.bytes),
            p.bytes.len() as u64,
            p.media_type.as_str().to_string(),
        )),
        Err(e) => Err(process_problem(e)),
    }
}

/// A processing refusal as a seed problem, with the store's own reason.
pub fn process_problem(e: ProcessError) -> Problem {
    match e {
        ProcessError::TooManyPixels(why) => Problem::new(ProblemKind::AttachmentTooManyPixels, why),
        ProcessError::Undecodable(why) => Problem::new(ProblemKind::AttachmentUndecodable, why),
        ProcessError::Unsupported(why) => Problem::new(ProblemKind::AttachmentUnsupported, why),
    }
}
