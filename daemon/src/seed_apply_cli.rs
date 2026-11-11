//! `mae-daemon kb seed` (apply) and `mae-daemon kb export`: the two operator
//! commands that open the collab store offline.
//!
//! Store access follows the `checkpoint`/`restore` precedent (an offline
//! `DocStore` over `state.db`), with what that precedent lacked: the store lock
//! (`mae_daemon::store_lock`), which the serving daemon also takes, and a check
//! that no daemon answers on the control socket (one built before the lock
//! existed does not take it). Lock first, then check.
//!
//! @ai-caution: [security] Operator commands, like `token mint`: applying names
//! a KB's owner and writes its content as the daemon. Neither may become an MCP
//! tool or an agent-reachable RPC (CLAUDE.md principle #16; ADR-113 A4: no agent
//! principal can perform the imported act).

use std::path::PathBuf;
use std::sync::Arc;

use mae_daemon::doc_store::DocStore;
use mae_daemon::seed::{self, ApplyOutcome, ApplyTarget};
use mae_daemon::storage;
use mae_daemon::store_lock::StoreLock;

use crate::config::DaemonConfig;
use crate::seed_cli::{plan_options, SeedArgs};

const EXPORT_USAGE: &str = "usage: mae-daemon kb export --kb-id <id> --out <dir> [--force]";

/// The collab store, opened offline and held exclusively until dropped.
pub(crate) struct OfflineStore {
    pub(crate) doc_store: Arc<DocStore>,
    _lock: StoreLock,
}

/// Open the collab store for an operator command that must not share it with
/// a running daemon. Refuses (naming why) if the lock is held or a daemon
/// answers on the control socket.
pub(crate) fn open_exclusive(config: &DaemonConfig) -> Result<OfflineStore, String> {
    let dir = config.resolve_collab_data_dir();
    let lock = StoreLock::acquire(&dir)?;
    if std::os::unix::net::UnixStream::connect(&config.socket).is_ok() {
        return Err(format!(
            "a daemon is answering on {}: stop it first",
            config.socket.display()
        ));
    }
    let db_path = dir.join("state.db");
    let backend =
        storage::SqliteBackend::open_with_pool_size(&db_path, config.collab.storage.shard_count)
            .map_err(|e| format!("open {}: {e}", db_path.display()))?;
    let doc_store = Arc::new(DocStore::new(
        Arc::new(backend),
        config.collab.storage.compact_threshold,
    ));
    Ok(OfflineStore {
        doc_store,
        _lock: lock,
    })
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Recompute the plan from git, then apply it. 0 applied (or already), 1
/// refused or failed.
pub(crate) async fn run_apply(config: &DaemonConfig, args: &SeedArgs) -> i32 {
    match apply(config, args).await {
        Ok(ApplyOutcome::Applied { created, present }) => {
            println!(
                "applied: kb={} nodes_written={created} nodes_already_present={present}",
                args.kb_id.as_deref().unwrap_or_default()
            );
            0
        }
        Ok(ApplyOutcome::AlreadyApplied) => {
            println!("already applied: nothing was written");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

async fn apply(config: &DaemonConfig, args: &SeedArgs) -> Result<ApplyOutcome, String> {
    let plan = seed::plan(&plan_options(args)).map_err(|e| e.to_string())?;
    let identity = crate::token_cli::load_identity(config)?;
    let store = open_exclusive(config)?;
    let target = ApplyTarget {
        kb_id: args.kb_id.as_deref().unwrap_or_default(),
        plan_hash: args.plan_hash.as_deref().unwrap_or_default(),
        owner: &identity,
        now: now(),
    };
    seed::apply(&store.doc_store, &plan, &target)
        .await
        .map_err(|e| format!("{} ({:?})", e.detail, e.kind))
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ExportArgs {
    kb_id: String,
    out: PathBuf,
    force: bool,
}

fn parse_export_args(args: &[String]) -> Result<ExportArgs, String> {
    let mut a = ExportArgs::default();
    let mut i = 0;
    while i < args.len() {
        let (flag, inline) = match args[i].split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (args[i].as_str(), None),
        };
        i += 1;
        if flag == "--force" && inline.is_none() {
            a.force = true;
            continue;
        }
        let value = match inline {
            Some(v) => v,
            None => {
                i += 1;
                args.get(i - 1)
                    .filter(|v| !v.starts_with("--"))
                    .cloned()
                    .ok_or_else(|| format!("{flag} needs a value"))?
            }
        };
        match flag {
            "--kb-id" => a.kb_id = value,
            "--out" => a.out = PathBuf::from(value),
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    if a.kb_id.is_empty() || a.out.as_os_str().is_empty() {
        return Err("--kb-id and --out are required".into());
    }
    Ok(a)
}

/// `kb export`: 0 every node written, 1 a node refused or the export failed,
/// 2 usage.
pub(crate) async fn run_export(config: &DaemonConfig, rest: &[String]) -> i32 {
    let args = match parse_export_args(rest) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n{EXPORT_USAGE}");
            return 2;
        }
    };
    let store = match open_exclusive(config) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    match seed::export(&store.doc_store, &args.kb_id, &args.out, args.force).await {
        Ok(report) => print_export(&report),
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn print_export(report: &seed::ExportReport) -> i32 {
    for note in &report.notes {
        println!("note: {note}");
    }
    for r in &report.refused {
        println!("refused: {} [{}] {}", r.node_id, r.kind.as_str(), r.detail);
    }
    println!(
        "exported: written={} refused={}",
        report.written.len(),
        report.refused.len()
    );
    i32::from(!report.refused.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn export_args_need_kb_and_out_and_refuse_unknowns() {
        let a = parse_export_args(&args(&["--kb-id", "k", "--out=d", "--force"])).unwrap();
        assert_eq!((a.kb_id.as_str(), a.force), ("k", true));
        for bad in [
            &["--kb-id", "k"][..],
            &["--out", "d"],
            &["--kb-id", "k", "--out", "d", "--forse"],
            &["--kb-id", "k", "--out", "d", "--force=yes"],
        ] {
            assert!(parse_export_args(&args(bad)).is_err(), "{bad:?}");
        }
    }
}
