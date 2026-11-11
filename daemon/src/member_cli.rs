//! `mae-daemon kb member add|remove|list`: the operator manages membership of a
//! KB this daemon owns (a seeded KB), offline, under the store lock, signed as
//! the daemon. The bridge until ADR-098 binds directory identities to members.
//!
//! @ai-caution: [security] Operator-only, like `token mint` and `kb seed`: it
//! acts as the KB's owner. Never an MCP tool or an agent-reachable RPC
//! (CLAUDE.md principle #16).

use std::sync::Arc;

use mae_daemon::collab_handler::{
    operator_change_membership, operator_list_members, OperatorMemberOp,
};
use mae_sync::kb::Role;

use crate::config::DaemonConfig;

const USAGE: &str = "usage: mae-daemon kb member add --kb-id <id> --member <SHA256:…> --role viewer|editor [--label <label>]\n\
     \x20      mae-daemon kb member remove --kb-id <id> --member <SHA256:…>\n\
     \x20      mae-daemon kb member list --kb-id <id>\n\
     The daemon must be stopped; the KB must be owned by this daemon (e.g. a seeded KB).";

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    kb_id: String,
    member: Option<String>,
    role: Option<String>,
    label: String,
}

/// Parse `--flag value` / `--flag=value`. Unknown flags and missing values are
/// usage errors, never ignored.
fn parse(rest: &[String]) -> Result<Args, String> {
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
        match flag {
            "--kb-id" => a.kb_id = value()?,
            "--member" => a.member = Some(value()?),
            "--role" => a.role = Some(value()?),
            "--label" => a.label = value()?,
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    if a.kb_id.is_empty() {
        return Err("--kb-id is required".into());
    }
    Ok(a)
}

fn role(name: Option<&str>) -> Result<Role, String> {
    match name {
        Some("viewer") => Ok(Role::Viewer),
        Some("editor") => Ok(Role::Editor),
        Some(other) => Err(format!("--role must be viewer or editor, not '{other}'")),
        None => Err("--role viewer|editor is required".into()),
    }
}

/// Exit code: 0 OK, 1 refused or failed, 2 usage.
pub(crate) async fn run(config: &DaemonConfig, rest: &[String]) -> i32 {
    let action = rest.first().map(String::as_str);
    let args = match parse(rest.get(1..).unwrap_or_default()) {
        Ok(a) if matches!(action, Some("add" | "remove" | "list")) => a,
        Ok(_) => {
            eprintln!("{USAGE}");
            return 2;
        }
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            return 2;
        }
    };
    match execute(config, action.unwrap_or_default(), &args).await {
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

async fn execute(config: &DaemonConfig, action: &str, a: &Args) -> Result<String, String> {
    let member = || {
        a.member
            .as_deref()
            .ok_or("--member is required".to_string())
    };
    let op = match action {
        "add" => Some(OperatorMemberOp::Add {
            member: member()?,
            role: role(a.role.as_deref())?,
            label: &a.label,
        }),
        "remove" => Some(OperatorMemberOp::Remove { member: member()? }),
        _ => None,
    };
    let store = crate::seed_apply_cli::open_exclusive(config)?;
    match op {
        Some(op) => {
            let identity = Arc::new(crate::token_cli::load_identity(config)?);
            store.doc_store.set_signer(Arc::clone(&identity));
            let r = operator_change_membership(&store.doc_store, &identity, &a.kb_id, op).await?;
            Ok(r.to_string())
        }
        None => {
            let (owner, members) = operator_list_members(&store.doc_store, &a.kb_id).await?;
            let mut out = format!("owner  {owner}");
            for (fp, role) in members {
                out.push_str(&format!("\n{role:<6} {fp}"));
            }
            Ok(out)
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
        let a = parse(&v(&[
            "--kb-id",
            "k",
            "--member=SHA256:x",
            "--role",
            "viewer",
        ]))
        .unwrap();
        assert_eq!(
            (a.kb_id.as_str(), a.member.as_deref(), a.role.as_deref()),
            ("k", Some("SHA256:x"), Some("viewer"))
        );
        assert!(
            parse(&v(&["--member", "SHA256:x"])).is_err(),
            "--kb-id required"
        );
        assert!(
            parse(&v(&["--kb-id", "k", "--owner", "x"])).is_err(),
            "unknown flag"
        );
        assert!(parse(&v(&["--kb-id"])).is_err(), "missing value");
        assert_eq!(role(Some("viewer")), Ok(Role::Viewer));
        assert!(
            role(Some("owner")).is_err(),
            "ownership is not transferred here"
        );
        assert!(role(None).is_err());
    }
}
