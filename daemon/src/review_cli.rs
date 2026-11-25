//! `mae-daemon kb review export|verify` (ADR-113 E10): the operator's side of
//! the review event chain.
//!
//! `export` writes a KB's chain as JSON Lines (header, events, a head signed
//! now), from the stopped daemon's store. `verify` checks such a file with no
//! daemon configuration or database at all, under a key the operator brings
//! (`--key`), never the one the file claims, and with `--against`, that it
//! extends a head kept from an earlier export: the only check that shows the
//! chain's last events removed by whoever holds the daemon's key.
//!
//! @ai-caution: [security] Operator-only and offline, like `kb history`:
//! never an MCP tool or an agent-reachable RPC (principle #16). `kb/review.events`
//! is the owner's paged read of the same lines.

use std::path::PathBuf;
use std::sync::Arc;

use mae_daemon::review::chain;

use crate::config::DaemonConfig;

const USAGE: &str = "usage: mae-daemon kb review export --kb-id <id> --out <file>\n\
     \x20      mae-daemon kb review verify --in <file> --key <base64> [--against <head-file>]\n\
     export needs the daemon stopped; verify needs neither the daemon nor its configuration.\n\
     A head file is one exported `head` line, kept where the daemon's key and database do not reach.";

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    kb_id: Option<String>,
    out: Option<PathBuf>,
    input: Option<PathBuf>,
    key: Option<String>,
    against: Option<PathBuf>,
}

/// Parse `--flag value` / `--flag=value` for `action`. Unknown flags and
/// missing values are usage errors, never ignored.
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
            ("export", "--kb-id") => a.kb_id = Some(value()?),
            ("export", "--out") => a.out = Some(value()?.into()),
            ("verify", "--in") => a.input = Some(value()?.into()),
            ("verify", "--key") => a.key = Some(value()?),
            ("verify", "--against") => a.against = Some(value()?.into()),
            (_, other) => return Err(format!("unknown argument '{other}'")),
        }
    }
    let missing = match action {
        "export" if a.kb_id.is_none() => Some("--kb-id"),
        "export" if a.out.is_none() => Some("--out"),
        "verify" if a.input.is_none() => Some("--in"),
        "verify" if a.key.is_none() => Some("--key"),
        "export" | "verify" => None,
        _ => return Err(format!("unknown action '{action}'")),
    };
    match missing {
        Some(m) => Err(format!("{m} is required")),
        None => Ok(a),
    }
}

/// Whether `rest` (after `kb`) is `review verify …`, which runs before any
/// configuration is resolved: a verifier must not need the daemon's files.
pub(crate) fn is_offline_verify(subcommand: Option<&str>, rest: &[String]) -> bool {
    subcommand == Some("kb")
        && rest.first().map(String::as_str) == Some("review")
        && rest.get(1).map(String::as_str) == Some("verify")
}

/// `rest` is what follows `kb review`. Exit code: 0 OK, 1 refused or not
/// verified, 2 usage.
pub(crate) async fn run(config: Option<&DaemonConfig>, rest: &[String]) -> i32 {
    let action = rest.first().map(String::as_str).unwrap_or_default();
    let args = match parse(action, rest.get(1..).unwrap_or_default()) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            return 2;
        }
    };
    let result = match (action, config) {
        ("verify", _) => verify(&args),
        ("export", Some(config)) => export(config, &args).await,
        _ => Err("export needs the daemon's configuration".into()),
    };
    match result {
        Ok(out) => {
            println!("{out}");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

async fn export(config: &DaemonConfig, a: &Args) -> Result<String, String> {
    let (kb_id, path) = (
        a.kb_id.as_deref().unwrap_or_default(),
        a.out.as_ref().unwrap(),
    );
    let store = crate::seed_apply_cli::open_exclusive(config)?;
    store
        .doc_store
        .set_signer(Arc::new(crate::token_cli::load_identity(config)?));
    let lines = mae_daemon::review::export_lines(&store.doc_store, kb_id)
        .await
        .map_err(|e| e.message)?;
    let text: String = lines
        .iter()
        .map(|l| serde_json::to_string(l).expect("plain data") + "\n")
        .collect();
    // Never overwrite: an export kept earlier is what a later one is checked against.
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    std::io::Write::write_all(&mut f, text.as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let head = lines.last().expect("an export ends with its head");
    Ok(format!(
        "wrote {} ({} events); its head line, to keep elsewhere:\n{}",
        path.display(),
        lines.len() - 2,
        serde_json::to_string(head).expect("plain data")
    ))
}

fn verify(a: &Args) -> Result<String, String> {
    let path = a.input.as_ref().unwrap();
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let key = chain::parse_key(a.key.as_deref().unwrap_or_default())
        .ok_or("--key is not a base64 Ed25519 public key")?;
    let against = match &a.against {
        None => None,
        Some(p) => {
            let t = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            let v: serde_json::Value =
                serde_json::from_str(t.trim()).map_err(|e| format!("{}: {e}", p.display()))?;
            Some(v)
        }
    };
    let v = chain::verify_export(&text, &key, against.as_ref())
        .map_err(|e| format!("NOT VERIFIED: {e}"))?;
    let mut out = format!(
        "KB {}: {} events, head {}",
        v.kb_id, v.events, v.head_entry_hash
    );
    if let Some(seq) = v.against_seq {
        out.push_str(&format!("\nextends the kept head at event {seq}"));
    }
    out.push_str("\nverified");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn flags_belong_to_their_action_and_required_ones_are_required() {
        assert!(parse("export", &v(&["--kb-id", "k", "--out", "f"])).is_ok());
        assert!(parse("export", &v(&["--kb-id", "k"])).is_err());
        assert!(parse("verify", &v(&["--in", "f", "--key=AA"])).is_ok());
        assert!(parse("verify", &v(&["--in", "f"])).is_err());
        assert!(parse("verify", &v(&["--in", "f", "--key", "k", "--kb-id", "k"])).is_err());
        assert!(parse("export", &v(&["--kb-id", "k", "--out", "f", "--key", "k"])).is_err());
        assert!(parse("delete", &v(&[])).is_err());
    }
}
