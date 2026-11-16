//! `mae-daemon kb anchor`: record, or show, a KB's content anchor (ADR-036
//! Amendment 1 F1) — for a KB shared before the amendment whose owner will not
//! reconnect to anchor it. Offline, under the store lock.
//!
//! @ai-caution: [security] Operator-only, like `kb member` and `token mint`:
//! an anchor decides where a KB's membership is read from. Never an MCP tool or
//! an agent-reachable RPC (CLAUDE.md principle #16).

use mae_daemon::collab_handler::{
    kb_membership_source, operator_anchor_kb, roster_only_members_report, MembershipSource,
    OperatorAnchor,
};
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
        let Some((k, source)) = store.doc_store.kb_anchor_record(&a.kb_id).await else {
            return Ok(format!("{}  (no anchor)", a.kb_id));
        };
        let head = format!("{}  {}  {}", a.kb_id, b64(&k), source.as_str());
        let src = kb_membership_source(&store.doc_store, &a.kb_id).await?;
        return Ok(format!("{head}\n{}", membership_lines(&src).join("\n")));
    };
    match operator_anchor_kb(&store.doc_store, &a.kb_id, key).await? {
        OperatorAnchor::Recorded { roster_only } => {
            let mut out = vec![format!("anchored {} to {}", a.kb_id, b64(&key))];
            out.extend(roster_only_lines(&roster_only));
            Ok(out.join("\n"))
        }
        OperatorAnchor::AlreadyAnchored => {
            Ok(format!("{} is already anchored to that key", a.kb_id))
        }
    }
}

/// Which source governs the KB's membership, and every member-list entry the
/// governing signed log does not admit (ADR-036 Amendment 1 F2).
fn membership_lines(src: &MembershipSource) -> Vec<String> {
    if !src.log_governed {
        return vec!["membership: member list".to_string()];
    }
    let mut out = vec!["membership: signed op-log".to_string()];
    out.extend(roster_only_lines(&src.roster_only));
    out
}

fn roster_only_lines(roster_only: &[String]) -> Vec<String> {
    if roster_only.is_empty() {
        return Vec::new();
    }
    let mut out = vec![format!(
        "! {} member-list entr{} the signed op-log does not admit have NO access; the owner must \
         admit each with a signed op:",
        roster_only.len(),
        if roster_only.len() == 1 { "y" } else { "ies" }
    )];
    out.extend(roster_only.iter().map(|fp| format!("    {fp}")));
    out
}

/// `mae-daemon doctor`'s membership section: every anchored KB whose signed
/// op-log leaves member-list entries out. Advisory — the instance works.
/// Offline only: the store lock is taken, so a running daemon is reported as
/// skipped rather than read under it.
pub(crate) fn doctor_lines(config: &DaemonConfig) -> Vec<String> {
    // Doctor is synchronous and may run inside the daemon's runtime (main) or
    // none (tests); a scoped thread with its own runtime serves both.
    std::thread::scope(|s| {
        s.spawn(|| {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => return vec![format!("  kb membership: not checked ({e})")],
            };
            rt.block_on(doctor_report(config))
        })
        .join()
        .unwrap_or_else(|_| vec!["  kb membership: not checked (the check panicked)".into()])
    })
}

async fn doctor_report(config: &DaemonConfig) -> Vec<String> {
    let store = match crate::seed_apply_cli::open_exclusive(config) {
        Ok(s) => s,
        Err(e) => return vec![format!("  kb membership: not checked ({e})")],
    };
    store.doc_store.load_kb_anchors().await;
    let report = roster_only_members_report(&store.doc_store).await;
    if report.is_empty() {
        return vec!["  kb membership: OK (no anchored KB leaves a listed member out)".into()];
    }
    let mut out = vec![format!("  kb membership: {} KB(s) to review", report.len())];
    for (kb_id, found) in report {
        match found {
            Ok(roster_only) => {
                out.push(format!("    KB {kb_id}:"));
                out.extend(
                    roster_only_lines(&roster_only)
                        .into_iter()
                        .map(|l| format!("      {l}")),
                );
            }
            Err(e) => out.push(format!("    KB {kb_id}: not checked ({e})")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_mcp::identity::Identity;
    use mae_sync::kb::{KbCollectionDoc, Role};
    use mae_sync::membership::MembershipAction;

    /// "Log wins, made visible" on the operator's surfaces: a KB shared before
    /// the amendment with Bob on its member list and an owner genesis in its
    /// log. Anchoring it with `kb anchor` names Bob, `kb anchor` (show) names
    /// him, and `doctor` names him — and said nothing before the anchor existed.
    #[tokio::test]
    async fn anchor_show_and_doctor_name_the_members_the_log_leaves_out() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = crate::tests::isolated(tmp.path());
        let (owner, bob) = (Identity::generate("owner"), Identity::generate("bob"));
        let (owner_fp, bob_fp) = (owner.fingerprint(), bob.fingerprint());
        let owner_pk = owner.public().to_bytes();
        {
            let store = crate::seed_apply_cli::open_exclusive(&config).unwrap();
            let mut coll = KbCollectionDoc::new_owned("kb-d", &owner_fp, "owner");
            coll.upsert_member(&bob_fp, "bob", Role::Editor);
            let admit = MembershipAction::Admit;
            let g = coll.build_membership_op(
                "kb-d",
                admit,
                &owner_fp,
                Some(Role::Owner),
                true,
                &owner_fp,
                0,
                None,
                0,
            );
            let sig = g.sign(&owner.secret_bytes());
            coll.append_signed_op(&g, &sig, &owner_pk);
            let by = mae_daemon::doc_editors::Attribution::Principal("test");
            let state = coll.encode_state();
            store
                .doc_store
                .share_doc("kbc:kb-d", &state, by)
                .await
                .unwrap();
        }
        let before = doctor_lines(&config).join("\n");
        assert!(before.contains("kb membership: OK"), "{before}");
        assert!(!before.contains(&bob_fp), "{before}");

        let anchor = |public_key| Args {
            kb_id: "kb-d".into(),
            public_key,
        };
        let recorded = execute(&config, &anchor(Some(owner_pk))).await.unwrap();
        assert!(
            recorded.contains(&bob_fp),
            "anchoring names bob: {recorded}"
        );
        let shown = execute(&config, &anchor(None)).await.unwrap();
        assert!(shown.contains("membership: signed op-log"), "{shown}");
        assert!(shown.contains(&bob_fp), "show names bob: {shown}");
        let after = doctor_lines(&config).join("\n");
        assert!(
            after.contains("KB kb-d") && after.contains(&bob_fp),
            "{after}"
        );
    }

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
