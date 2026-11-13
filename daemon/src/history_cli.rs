//! `mae-daemon kb history head|verify`: the operator's side of ADR-116 D4.
//!
//! `head` signs the KB's history head now (or takes the latest, if nothing
//! changed) and writes it as a file, for the operator to keep somewhere the
//! daemon's key and database do not reach. `verify` checks every chain, the
//! latest signed head and, with `--against`, a head kept that way: the only
//! check that shows a history rewritten by whoever holds the daemon's key.
//!
//! @ai-caution: [security] Operator-only, offline, under the store lock, like
//! `kb member`: never an MCP tool or an agent-reachable RPC (principle #16).

use std::path::PathBuf;
use std::sync::Arc;

use mae_daemon::history_verify::{verify_kb, HeadCheck};
use mae_sync::kb::history::SignedHead;

use crate::config::DaemonConfig;

const USAGE: &str = "usage: mae-daemon kb history head --kb-id <id> [--out <file>]\n\
     \x20      mae-daemon kb history verify --kb-id <id> [--against <head-file>]\n\
     The daemon must be stopped. Keep head files where the daemon's key and database do not reach.";

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    kb_id: String,
    out: Option<PathBuf>,
    against: Option<PathBuf>,
}

/// Parse `--flag value` / `--flag=value`. Unknown flags and missing values are
/// usage errors, never ignored; `--out` belongs to `head`, `--against` to
/// `verify`.
fn parse(action: &str, rest: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) => (f, Some(v.to_string())),
            None => (arg.as_str(), None),
        };
        let mut value = || {
            inline
                .clone()
                .or_else(|| it.next().cloned())
                .ok_or(format!("{flag} needs a value"))
        };
        match (action, flag) {
            (_, "--kb-id") => a.kb_id = value()?,
            ("head", "--out") => a.out = Some(value()?.into()),
            ("verify", "--against") => a.against = Some(value()?.into()),
            (_, other) => return Err(format!("unknown argument '{other}'")),
        }
    }
    if a.kb_id.is_empty() {
        return Err("--kb-id is required".into());
    }
    Ok(a)
}

/// Exit code: 0 OK, 1 refused, failed or not verified, 2 usage.
pub(crate) async fn run(config: &DaemonConfig, rest: &[String]) -> i32 {
    let action = rest.first().map(String::as_str).unwrap_or_default();
    let args = match parse(action, rest.get(1..).unwrap_or_default()) {
        Ok(a) if matches!(action, "head" | "verify") => a,
        Ok(_) => {
            eprintln!("{USAGE}");
            return 2;
        }
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            return 2;
        }
    };
    let result = match action {
        "head" => head(config, &args).await,
        _ => verify(config, &args).await,
    };
    match result {
        Ok((out, ok)) => {
            println!("{out}");
            i32::from(!ok)
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

async fn head(config: &DaemonConfig, a: &Args) -> Result<(String, bool), String> {
    let store = crate::seed_apply_cli::open_exclusive(config)?;
    store
        .doc_store
        .set_signer(Arc::new(crate::token_cli::load_identity(config)?));
    let signed = store
        .doc_store
        .history_sign_head(&a.kb_id)
        .await
        .map_err(|e| e.to_string())?;
    let head = match signed {
        Some(h) => h,
        None => store
            .doc_store
            .history_latest_head(&a.kb_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or(format!("KB '{}' has no version history to sign", a.kb_id))?,
    };
    let text = serde_json::to_string_pretty(&head.to_json()).expect("plain data");
    match &a.out {
        None => Ok((text, true)),
        Some(path) => {
            // Never overwrite: a kept head replaced by a newer one loses what it proves.
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            std::io::Write::write_all(&mut f, format!("{text}\n").as_bytes())
                .map_err(|e| format!("{}: {e}", path.display()))?;
            Ok((
                format!(
                    "wrote {} (head {} at {})",
                    path.display(),
                    hex::encode(head.digest),
                    head.at
                ),
                true,
            ))
        }
    }
}

/// ADR-116 D4: before a backup, sign each KB's history head, so the archive
/// carries a head as of the backup. That needs the store to itself; while the
/// daemon runs, its maintenance tick signs heads and the backup carries the
/// latest. Reports what it did and never fails the backup.
pub(crate) async fn sign_before_backup(config: &DaemonConfig) {
    let store = match crate::seed_apply_cli::open_exclusive(config) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("history heads: not signed now ({e}); the backup carries the latest the daemon signed");
            return;
        }
    };
    let identity = match crate::token_cli::load_identity(config) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("history heads: not signed (no daemon identity: {e})");
            return;
        }
    };
    store.doc_store.set_signer(Arc::new(identity));
    match store.doc_store.history_sign_heads().await {
        Ok(n) => eprintln!("history heads: signed {n} (unchanged heads are not signed again)"),
        Err(e) => eprintln!("history heads: not signed ({e})"),
    }
}

fn read_head(path: &PathBuf) -> Result<SignedHead, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    SignedHead::from_json(&v).map_err(|e| format!("{}: {e}", path.display()))
}

fn describe(label: &str, c: &HeadCheck) -> String {
    match c {
        HeadCheck::Unchecked(why) => format!("{label}: not checked ({why})"),
        HeadCheck::Ok { at, digest } => {
            format!("{label}: ok (head {} at {at})", hex::encode(digest))
        }
        HeadCheck::Failed {
            node_id,
            seq,
            reason,
            ..
        } => format!(
            "{label}: FAILED {reason}{}",
            node_id
                .as_ref()
                .map(|n| format!(" ({n} version {})", seq.unwrap_or(0)))
                .unwrap_or_default()
        ),
    }
}

async fn verify(config: &DaemonConfig, a: &Args) -> Result<(String, bool), String> {
    let against = a.against.as_ref().map(read_head).transpose()?;
    let store = crate::seed_apply_cli::open_exclusive(config)?;
    store
        .doc_store
        .set_signer(Arc::new(crate::token_cli::load_identity(config)?));
    let r = verify_kb(&store.doc_store, &a.kb_id, against.as_ref())
        .await
        .map_err(|e| e.to_string())?;
    if r.nodes == 0 && r.against.is_none() {
        // Nothing to verify must not read as "verified": a mistyped id would.
        return Err(format!(
            "KB '{}' has no version history (check the id)",
            a.kb_id
        ));
    }
    let mut out = format!("{} nodes, {} versions", r.nodes, r.versions);
    for (node, e) in &r.failures {
        out.push_str(&format!(
            "\nchain: FAILED {node} version {} ({})",
            e.seq,
            e.broken.as_str()
        ));
    }
    out.push_str(&format!("\n{}", describe("latest signed head", &r.signed)));
    if let Some(c) = &r.against {
        out.push_str(&format!("\n{}", describe("kept head", c)));
    }
    out.push_str(if r.ok() {
        "\nverified"
    } else {
        "\nNOT VERIFIED"
    });
    Ok((out, r.ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn arguments_are_parsed_strictly() {
        let a = parse("verify", &v(&["--kb-id", "k", "--against=h.json"])).unwrap();
        assert_eq!((a.kb_id.as_str(), a.against), ("k", Some("h.json".into())));
        assert!(
            parse("verify", &v(&["--kb-id", "k", "--out", "x"])).is_err(),
            "--out is head's"
        );
        assert!(
            parse("head", &v(&["--kb-id", "k", "--against", "x"])).is_err(),
            "--against is verify's"
        );
        assert!(
            parse("head", &v(&["--out", "x"])).is_err(),
            "--kb-id is required"
        );
        assert!(
            parse("head", &v(&["--kb-id"])).is_err(),
            "a flag needs its value"
        );
    }
}
