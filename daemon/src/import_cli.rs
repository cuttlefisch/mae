//! `mae-daemon kb import` (ADR-113 E4): documents already approved and signed,
//! with their history, into a KB, from a directory holding `import.json`,
//! `SHA256SUMS` and the files. A dry run verifies everything and prints an
//! `import_hash`; the apply needs it, recomputes it, and writes all or
//! nothing. A local operator command only (principle #16), run like
//! `kb seed`: with the daemon stopped, under the store lock.

use std::path::PathBuf;
use std::sync::Arc;

use mae_daemon::review::import::{self, ApplyInput};

use crate::config::DaemonConfig;

const USAGE: &str =
    "usage: mae-daemon kb import --dry-run --kb-id <id> --dir <import-dir> [--json]\n\
     \x20      mae-daemon kb import --kb-id <id> --dir <import-dir> --import-hash <hash> [--json]\n\
     \x20 <import-dir> holds import.json (mae-import/1), SHA256SUMS (as sha256sum writes it) and\n\
     \x20 the files they name. Every file must have its manifest line, and every line a file.";

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ImportArgs {
    pub(crate) dry_run: bool,
    pub(crate) kb_id: String,
    pub(crate) dir: PathBuf,
    pub(crate) import_hash: Option<String>,
    pub(crate) json: bool,
}

pub(crate) fn parse(args: &[String]) -> Result<ImportArgs, String> {
    let mut a = ImportArgs::default();
    let (mut kb, mut dir) = (None, None);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) => (f, Some(v.to_string())),
            None => (arg.as_str(), None),
        };
        let mut value = |name: &str| -> Result<String, String> {
            let v = inline.clone().or_else(|| it.next().cloned());
            v.filter(|v| !v.is_empty() && !v.starts_with("--"))
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match flag {
            "--dry-run" if inline.is_none() => a.dry_run = true,
            "--json" if inline.is_none() => a.json = true,
            "--kb-id" => kb = Some(value("--kb-id")?),
            "--dir" => dir = Some(PathBuf::from(value("--dir")?)),
            "--import-hash" => a.import_hash = Some(value("--import-hash")?),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    a.kb_id = kb.ok_or("--kb-id <id> is required")?;
    a.dir = dir.ok_or("--dir <import-dir> is required")?;
    match (a.dry_run, &a.import_hash) {
        (true, Some(_)) => Err("--import-hash belongs to the apply, not the dry run".into()),
        (false, None) => Err("applying needs --import-hash <hash> (from the dry run)".into()),
        _ => Ok(a),
    }
}

fn os_user() -> String {
    std::env::var("USER")
        .ok()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| "operator".into())
}

/// Exit code: 0 done, 1 refused or failed, 2 usage.
pub(crate) async fn run(config: &DaemonConfig, rest: &[String]) -> i32 {
    let args = match parse(rest) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            return 2;
        }
    };
    let identity = match crate::token_cli::load_identity(config) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let store = match crate::seed_apply_cli::open_exclusive(config) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    store.doc_store.set_signer(Arc::new(identity));
    let limits = crate::seed_cli::attachment_limits(config);
    let outcome = async {
        let plan = import::plan(&store.doc_store, &args.dir, &args.kb_id, limits.max_bytes).await?;
        if args.dry_run {
            return Ok(import::dry_run_json(&plan));
        }
        let hash = plan.checked.import_hash.clone();
        let input = ApplyInput {
            import_hash: args.import_hash.as_deref().unwrap_or_default(),
            os_user: &os_user(),
            limits,
            quota: Some(mae_daemon::blob_store::StorageQuota::unassigned(
                config.oauth.default_storage_bytes,
            )),
        };
        let event_id = import::apply(&store.doc_store, plan, input).await?;
        Ok(serde_json::json!({ "ok": true, "event_id": event_id, "import_hash": hash }))
    }
    .await;
    report(&args, outcome)
}

fn report(args: &ImportArgs, outcome: Result<serde_json::Value, import::ImportError>) -> i32 {
    match outcome {
        Ok(v) if args.json => {
            println!("{v}");
            0
        }
        Ok(v) if args.dry_run => {
            println!(
                "dry run: {} pages, {} revisions, {} PDFs verified\nimport_hash: {}",
                v["pages"],
                v["revisions"],
                v["pdfs"],
                v["import_hash"].as_str().unwrap_or("")
            );
            0
        }
        Ok(v) => {
            println!("imported: event {}", v["event_id"].as_str().unwrap_or(""));
            0
        }
        Err(e) => {
            if args.json {
                println!("{}", e.to_json());
            }
            eprintln!("refused ({}): {e}", e.kind);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_flags_parse_and_the_dry_run_and_apply_are_kept_apart() {
        let dry = parse(&args(&["--dry-run", "--kb-id", "k", "--dir=d", "--json"])).unwrap();
        assert!(dry.dry_run && dry.json && dry.import_hash.is_none());
        let apply = parse(&args(&["--kb-id=k", "--dir", "d", "--import-hash", "h"])).unwrap();
        assert_eq!(apply.import_hash.as_deref(), Some("h"));
        for bad in [
            &["--dir", "d"][..],
            &["--kb-id", "k"],
            &["--kb-id", "k", "--dir", "d"],
            &[
                "--dry-run",
                "--kb-id",
                "k",
                "--dir",
                "d",
                "--import-hash",
                "h",
            ],
            &["--kb-id", "k", "--dir", "--json", "--import-hash", "h"],
            &[
                "--kb-id",
                "k",
                "--dir",
                "d",
                "--import-hash",
                "h",
                "--force",
            ],
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad:?}");
        }
    }
}
