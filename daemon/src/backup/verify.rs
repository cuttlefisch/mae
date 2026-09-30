//! `mae-daemon backup verify <dir>`: prove a restored backup is usable.
//!
//! Prints one `key=number` line per KB store (`kb.<name>=<nodes>`) and
//! `identity=<0|1>`, the format a restore rehearsal reads. Exits non-zero if
//! anything disagrees with the manifest the archive carried. A count that
//! merely printed would let a truncated or corrupt store pass as long as it
//! was non-empty.
//!
//! Order matters: every file is hashed BEFORE any store is opened, because a
//! Cozo open rewrites the file header (it switches the database to WAL mode).

use std::path::Path;

use mae_mcp::identity::{Identity, PublicKey};

use super::create::count_nodes;
use super::manifest::{self, Manifest, IDENTITY_DIR, MANIFEST_PATH};

/// What `verify` found: the lines to print and every disagreement.
#[derive(Debug, Default)]
pub(crate) struct Report {
    pub lines: Vec<String>,
    pub problems: Vec<String>,
}

pub(crate) fn verify(dir: &Path) -> Result<Report, String> {
    let bytes = std::fs::read(dir.join(MANIFEST_PATH)).map_err(|e| {
        format!(
            "{}: {e} (was this directory produced by `backup restore`?)",
            MANIFEST_PATH
        )
    })?;
    let m = Manifest::parse(&bytes)?;
    let mut report = Report::default();
    check_hashes(&m, dir, &mut report.problems);
    check_sqlite_integrity(&m, dir, &mut report.problems);
    for store in &m.stores {
        match count_nodes(&dir.join(&store.file)) {
            Ok(n) => {
                report.lines.push(format!("{}={n}", store.key));
                if n != store.nodes {
                    report.problems.push(format!(
                        "{}: {n} nodes restored, {} when backed up",
                        store.key, store.nodes
                    ));
                }
            }
            Err(e) => report.problems.push(format!("{}: {e}", store.key)),
        }
    }
    let identity = check_identity(&m, dir, &mut report.problems);
    report
        .lines
        .push(format!("identity={}", u8::from(identity)));
    Ok(report)
}

fn check_hashes(m: &Manifest, dir: &Path, problems: &mut Vec<String>) {
    for f in &m.files {
        match manifest::hash_file(&dir.join(&f.path)) {
            Ok((sha, bytes)) if sha == f.sha256 && bytes == f.bytes => {}
            Ok(_) => problems.push(format!("{}: content differs from the manifest", f.path)),
            Err(e) => problems.push(format!("{}: {e}", f.path)),
        }
    }
}

/// `PRAGMA integrity_check` on every SQLite file, read-only.
fn check_sqlite_integrity(m: &Manifest, dir: &Path, problems: &mut Vec<String>) {
    let sqlite_files = m
        .stores
        .iter()
        .map(|s| s.file.as_str())
        .chain(m.file("collab/state.db").map(|f| f.path.as_str()));
    for path in sqlite_files {
        if let Err(e) = integrity_ok(&dir.join(path)) {
            problems.push(format!("{path}: {e}"));
        }
    }
}

fn integrity_ok(path: &Path) -> Result<(), String> {
    let flags = sqlite::OpenFlags::new().with_read_only();
    let conn = sqlite::Connection::open_with_flags(path, flags).map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("PRAGMA integrity_check")
        .map_err(|e| e.to_string())?;
    let mut verdict = Vec::new();
    while let Ok(sqlite::State::Row) = stmt.next() {
        verdict.push(stmt.read::<String, _>(0).map_err(|e| e.to_string())?);
    }
    match verdict.as_slice() {
        [ok] if ok == "ok" => Ok(()),
        _ => Err(format!("integrity check failed: {}", verdict.join("; "))),
    }
}

/// The identity is present when the private key loads and, if the public key
/// file came along, the two belong together.
fn check_identity(m: &Manifest, dir: &Path, problems: &mut Vec<String>) -> bool {
    if !m.identity {
        return false;
    }
    let id_dir = dir.join(IDENTITY_DIR);
    let Some(id) = Identity::load_secret(&id_dir, "backup-verify") else {
        problems.push("identity: the private key does not load".into());
        return false;
    };
    let Ok(pub_line) = std::fs::read_to_string(id_dir.join("id_ed25519.pub")) else {
        return true;
    };
    match PublicKey::from_line(pub_line.trim()) {
        Some(pk) if pk.fingerprint() == id.fingerprint() => true,
        _ => {
            problems.push("identity: id_ed25519.pub does not match the private key".into());
            false
        }
    }
}
