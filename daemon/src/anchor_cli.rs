//! `mae-daemon kb anchor`: record, or show, a KB's content anchor (ADR-036
//! Amendment 1 F1) — for a KB shared before the amendment whose owner will not
//! reconnect to anchor it. Offline, under the store lock.
//!
//! @ai-caution: [security] Operator-only, like `kb member` and `token mint`:
//! an anchor decides where a KB's membership is read from. Never an MCP tool or
//! an agent-reachable RPC (CLAUDE.md principle #16).

use mae_daemon::collab_handler::{operator_anchor_kb, OperatorAnchor};
use mae_sync::encoding::{base64_to_update, update_to_base64};

use crate::config::DaemonConfig;

const USAGE: &str = "usage: mae-daemon kb anchor --kb-id <id> [--public-key <base64>]\n\
     Without --public-key, prints the KB's anchor. With it, records the key as the anchor:\n\
     only the key whose fingerprint the KB names as owner, and only once.\n\
     The daemon must be stopped.";

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    kb_id: String,
    public_key: Option<[u8; 32]>,
}

/// Parse `--flag value` / `--flag=value`. Unknown flags, missing values and a
/// key that is not the standard base64 of 32 bytes are usage errors.
fn parse(rest: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) => (f, Some(v.to_string())),
            None => (arg.as_str(), None),
        };
        let value = inline
            .or_else(|| it.next().cloned())
            .ok_or(format!("{flag} needs a value"))?;
        match flag {
            "--kb-id" => a.kb_id = value,
            "--public-key" => a.public_key = Some(public_key(&value)?),
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    if a.kb_id.is_empty() {
        return Err("--kb-id is required".into());
    }
    Ok(a)
}

fn public_key(b64: &str) -> Result<[u8; 32], String> {
    let bytes =
        base64_to_update(b64.trim()).map_err(|e| format!("--public-key is not base64: {e}"))?;
    bytes
        .try_into()
        .map_err(|b: Vec<u8>| format!("--public-key is {} bytes, not 32", b.len()))
}

/// Exit code: 0 OK, 1 refused or failed, 2 usage.
pub(crate) async fn run(config: &DaemonConfig, rest: &[String]) -> i32 {
    let args = match parse(rest) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            return 2;
        }
    };
    match execute(config, &args).await {
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

async fn execute(config: &DaemonConfig, a: &Args) -> Result<String, String> {
    let store = crate::seed_apply_cli::open_exclusive(config)?;
    store.doc_store.load_kb_anchors().await;
    let b64 = |k: &[u8; 32]| update_to_base64(k);
    let Some(key) = a.public_key else {
        return Ok(match store.doc_store.kb_anchor_record(&a.kb_id).await {
            Some((k, source)) => format!("{}  {}  {}", a.kb_id, b64(&k), source.as_str()),
            None => format!("{}  (no anchor)", a.kb_id),
        });
    };
    match operator_anchor_kb(&store.doc_store, &a.kb_id, key).await? {
        OperatorAnchor::Recorded => Ok(format!("anchored {} to {}", a.kb_id, b64(&key))),
        OperatorAnchor::AlreadyAnchored => {
            Ok(format!("{} is already anchored to that key", a.kb_id))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn arguments_are_parsed_strictly() {
        let key = update_to_base64(&[7u8; 32]);
        let a = parse(&v(&["--kb-id", "k", &format!("--public-key={key}")])).unwrap();
        assert_eq!(a.kb_id, "k");
        assert_eq!(a.public_key, Some([7u8; 32]));
        assert_eq!(parse(&v(&["--kb-id=k"])).unwrap().public_key, None);
        assert!(
            parse(&v(&["--public-key", &key])).is_err(),
            "--kb-id required"
        );
        assert!(
            parse(&v(&["--kb-id", "k", "--owner", "x"])).is_err(),
            "unknown flag"
        );
        assert!(parse(&v(&["--kb-id"])).is_err(), "missing value");
        let short = update_to_base64(&[7u8; 31]);
        assert!(
            parse(&v(&["--kb-id", "k", "--public-key", &short])).is_err(),
            "31 bytes"
        );
        assert!(
            parse(&v(&["--kb-id", "k", "--public-key", "%%"])).is_err(),
            "not base64"
        );
    }
}
