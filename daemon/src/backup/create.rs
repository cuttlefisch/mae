//! `mae-daemon backup create <out.tar>`: one self-contained archive of
//! everything this daemon instance keeps on disk.
//!
//! Safe against a RUNNING daemon: every SQLite file is copied with
//! `VACUUM INTO` (`mae_kb::backup::snapshot`), which reads inside one read
//! transaction and includes writes still in the WAL. Nothing is copied with a
//! plain file copy except files the daemon writes atomically (registry) or
//! never rewrites in place (identity).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use mae_kb::{CozoKbStore, KbStore};

use super::manifest::{self, FileEntry, Manifest, StoreEntry, IDENTITY_DIR};
use crate::config::DaemonConfig;

/// What a staged file is, which decides how it is copied.
#[derive(Debug)]
pub(crate) enum Kind {
    /// A Cozo KB store: snapshotted, counted, then snapshotted clean.
    Store { key: String, uuid: Option<String> },
    /// A plain SQLite database (the collab store): snapshotted.
    Sqlite,
    /// A file copied byte for byte.
    Plain,
}

#[derive(Debug)]
pub(crate) struct Source {
    pub kind: Kind,
    pub src: PathBuf,
    /// Archive-relative, `/`-separated.
    pub archive_path: String,
}

/// Every file to back up for this config. Missing optional files are skipped;
/// a registry row whose store is missing is reported on stderr and skipped.
pub(crate) fn sources(config: &DaemonConfig) -> Vec<Source> {
    let data_dir = config.effective_data_dir();
    let registry = mae_kb::federation::KbRegistry::load(&data_dir);
    let main_store = data_dir.join("daemon-kb.cozo");
    let mut out = Vec::new();
    let mut keys = HashSet::new();

    if main_store.exists() {
        // The daemon's own store takes the name of the registry row that
        // points at it, when there is one.
        let row = registry
            .instances
            .iter()
            .find(|i| crate::same_store_file(&i.db_path, &main_store));
        let key = unique_key(
            &mut keys,
            manifest::store_key(row.map_or("daemon", |r| r.name.as_str())),
            row.map(|r| r.uuid.as_str()),
        );
        out.push(Source {
            kind: Kind::Store {
                key,
                uuid: row.map(|r| r.uuid.clone()),
            },
            src: main_store.clone(),
            archive_path: "stores/daemon-kb.cozo".into(),
        });
    }
    for inst in &registry.instances {
        if inst.db_path.as_os_str().is_empty() || crate::same_store_file(&inst.db_path, &main_store)
        {
            continue;
        }
        if !inst.db_path.exists() {
            eprintln!(
                "note: KB {} has no store at {}; not in this backup",
                inst.name,
                inst.db_path.display()
            );
            continue;
        }
        let key = unique_key(&mut keys, manifest::store_key(&inst.name), Some(&inst.uuid));
        out.push(Source {
            kind: Kind::Store {
                key,
                uuid: Some(inst.uuid.clone()),
            },
            src: inst.db_path.clone(),
            archive_path: format!("stores/{}.sqlite", sanitize_file(&inst.uuid)),
        });
    }
    out.extend(side_files(config, &data_dir));
    out
}

/// Registry, collab store and identity files — each only if present.
fn side_files(config: &DaemonConfig, data_dir: &Path) -> Vec<Source> {
    let paths = config.instance_paths();
    let identity = |name: &str| format!("{IDENTITY_DIR}/{name}");
    let id_dir = paths.identity_dir.as_deref();
    let in_id = |name: &str| id_dir.map(|d| d.join(name));
    let candidates = [
        (
            Kind::Plain,
            Some(data_dir.join("kb-registry.toml")),
            "kb-registry.toml".to_string(),
        ),
        (
            Kind::Sqlite,
            Some(paths.collab_data_dir.join("state.db")),
            "collab/state.db".to_string(),
        ),
        (Kind::Plain, in_id("id_ed25519"), identity("id_ed25519")),
        (
            Kind::Plain,
            in_id("id_ed25519.pub"),
            identity("id_ed25519.pub"),
        ),
        (Kind::Plain, in_id("known_hosts"), identity("known_hosts")),
        (
            Kind::Plain,
            paths.authorized_keys,
            identity("authorized_keys"),
        ),
        (Kind::Plain, paths.keystore, identity("trusted_keys")),
    ];
    candidates
        .into_iter()
        .filter_map(|(kind, src, archive_path)| {
            src.filter(|s| s.is_file()).map(|src| Source {
                kind,
                src,
                archive_path,
            })
        })
        .collect()
}

/// A key not yet taken: on a collision the uuid (or a counter) is appended, so
/// two KBs whose names sanitise alike still report separately.
fn unique_key(taken: &mut HashSet<String>, key: String, uuid: Option<&str>) -> String {
    let mut candidate = key.clone();
    let mut n = 1;
    while taken.contains(&candidate) {
        candidate = match uuid {
            Some(u) if n == 1 => format!("{key}-{}", sanitize_file(&u[..u.len().min(8)])),
            _ => format!("{key}-{n}"),
        };
        n += 1;
    }
    taken.insert(candidate.clone());
    candidate
}

fn sanitize_file(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Copy one source into `staging`, returning its store entry if it is a store.
pub(crate) fn stage(source: &Source, staging: &Path) -> Result<Option<StoreEntry>, String> {
    let dst = staging.join(&source.archive_path);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let what = |e: &dyn std::fmt::Display| format!("{}: {e}", source.src.display());
    match &source.kind {
        Kind::Plain => std::fs::copy(&source.src, &dst)
            .map(|_| None)
            .map_err(|e| what(&e)),
        Kind::Sqlite => mae_kb::backup::snapshot(&source.src, &dst)
            .map(|_| None)
            .map_err(|e| what(&e)),
        Kind::Store { key, uuid } => {
            let nodes = snapshot_counted(&source.src, &dst).map_err(|e| what(&e))?;
            Ok(Some(StoreEntry {
                key: key.clone(),
                file: source.archive_path.clone(),
                nodes,
                uuid: uuid.clone(),
            }))
        }
    }
}

/// Snapshot a Cozo store to `dst` and return the node count OF THAT COPY.
///
/// Counting needs a Cozo open, and opening switches the file to WAL mode and
/// may leave sidecars. So the count is taken on a first snapshot, and `dst` is
/// a second, clean snapshot of that first one: the archived file holds exactly
/// what was counted, as one self-contained file.
fn snapshot_counted(src: &Path, dst: &Path) -> Result<u64, String> {
    let counted = dst.with_extension("counting");
    mae_kb::backup::snapshot(src, &counted).map_err(|e| format!("snapshot: {e}"))?;
    let nodes = count_nodes(&counted)?;
    let result = mae_kb::backup::snapshot(&counted, dst).map_err(|e| format!("snapshot: {e}"));
    remove_with_sidecars(&counted);
    result.map(|_| nodes)
}

/// KB node count of the store at `path`, opened offline. The path must exist:
/// opening a missing path would CREATE an empty store and report 0.
pub(crate) fn count_nodes(path: &Path) -> Result<u64, String> {
    if !path.is_file() {
        return Err(format!("no store at {}", path.display()));
    }
    let store = CozoKbStore::open_with_engine(path, "sqlite").map_err(|e| format!("open: {e}"))?;
    let ids = store.list_ids(None).map_err(|e| format!("count: {e}"))?;
    Ok(ids.len() as u64)
}

fn remove_with_sidecars(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(p));
    }
}

/// Stage every source and build the manifest over what was staged.
pub(crate) fn stage_all(sources: &[Source], staging: &Path, now: u64) -> Result<Manifest, String> {
    let mut stores = Vec::new();
    let mut files = Vec::new();
    for source in sources {
        if let Some(store) = stage(source, staging)? {
            stores.push(store);
        }
        let (sha256, bytes) = manifest::hash_file(&staging.join(&source.archive_path))
            .map_err(|e| format!("hash {}: {e}", source.archive_path))?;
        files.push(FileEntry {
            path: source.archive_path.clone(),
            sha256,
            bytes,
        });
    }
    if stores.is_empty() {
        return Err("no KB store found to back up: a backup of nothing is not a backup".into());
    }
    let identity = files
        .iter()
        .any(|f| f.path == format!("{IDENTITY_DIR}/id_ed25519"));
    Ok(Manifest {
        format: manifest::FORMAT.into(),
        version: manifest::VERSION,
        created_unix: now,
        daemon_version: env!("CARGO_PKG_VERSION").into(),
        files,
        stores,
        identity,
    })
}
