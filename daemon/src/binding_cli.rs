//! `mae-daemon binding list|bind|unbind|set-kind`: the administrator's side of
//! the identity-provider binding table (ADR-098 D3, Amendment 1 C2/C3).
//!
//! `bind` binds without proof of possession, for development, tests and
//! recovery: a local operator can already mint a token for any fingerprint
//! (`token mint`), so this adds no power. It is recorded `bound_by = admin`
//! and `list` shows which bindings were made which way. A new binding is an
//! `agent`; only `set-kind` makes one a `person`.
//!
//! Runs against a serving daemon: the table is its own file
//! ([`BINDINGS_FILE`]) that the listener reads on every request, so an
//! `unbind` or `set-kind` takes effect on the next request.
//!
//! @ai-caution: [security] Operator-only, like `token mint` and `kb member`
//! (CLAUDE.md principle #16): this decides which identity-provider subject acts
//! as which member key and who counts as a person. Never an MCP tool, a Scheme
//! primitive or a network method.

use mae_daemon::identity_binding::{BindingError, BindingStore, BoundBy, Selector, BINDINGS_FILE};
use mae_daemon::oauth_self_issue::PrincipalKind;

use crate::config::DaemonConfig;

const USAGE: &str = "usage: mae-daemon binding list\n\
     \x20      mae-daemon binding bind --issuer <iss> --subject <sub> --fingerprint <SHA256:…>\n\
     \x20      mae-daemon binding unbind (--fingerprint <SHA256:…> | --issuer <iss> --subject <sub>)\n\
     \x20      mae-daemon binding set-kind (--fingerprint <SHA256:…> | --issuer <iss> --subject <sub>) --kind person|agent";

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    issuer: Option<String>,
    subject: Option<String>,
    fingerprint: Option<String>,
    kind: Option<String>,
}

/// Parse `--flag value` / `--flag=value`. Unknown flags and missing values are
/// usage errors, never ignored.
fn parse(rest: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        let mut value = || {
            inline
                .clone()
                .or_else(|| it.next().cloned())
                .ok_or(format!("{flag} needs a value"))
        };
        match flag {
            "--issuer" => a.issuer = Some(value()?),
            "--subject" => a.subject = Some(value()?),
            "--fingerprint" => a.fingerprint = Some(value()?),
            "--kind" => a.kind = Some(value()?),
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(a)
}

/// Exactly one way of naming a binding: a fingerprint, or issuer and subject.
fn selector(a: &Args) -> Result<Selector, String> {
    match (&a.fingerprint, &a.issuer, &a.subject) {
        (Some(fp), None, None) => Ok(Selector::Fingerprint(fp.clone())),
        (None, Some(i), Some(s)) => Ok(Selector::Subject {
            issuer: i.clone(),
            subject: s.clone(),
        }),
        _ => Err("name the binding by --fingerprint, or by --issuer and --subject".into()),
    }
}

/// `mae-daemon binding <action> …`. Exit code: 0 done, 1 refused, 2 usage.
pub(crate) fn run(config: &DaemonConfig, rest: &[String]) -> i32 {
    let action = rest.first().map(String::as_str).unwrap_or_default();
    let args = match parse(rest.get(1..).unwrap_or_default()) {
        Ok(a) => a,
        Err(e) => return usage(&e),
    };
    let path = config.resolve_collab_data_dir().join(BINDINGS_FILE);
    let store = match BindingStore::open(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {}: {e}", path.display());
            return 1;
        }
    };
    let result = match action {
        "list" if args == Args::default() => list(&store),
        "bind" => bind(&store, &args),
        "unbind" if args.kind.is_none() => unbind(&store, &args),
        "set-kind" => set_kind(&store, &args),
        _ => return usage("unknown action or argument"),
    };
    match result {
        Ok(out) => {
            print!("{out}");
            0
        }
        Err(Refusal::Usage(e)) => usage(&e),
        Err(Refusal::Refused(e)) => {
            eprintln!("error: {e}");
            1
        }
    }
}

enum Refusal {
    Usage(String),
    Refused(String),
}

impl From<BindingError> for Refusal {
    fn from(e: BindingError) -> Self {
        Refusal::Refused(e.to_string())
    }
}

fn usage(e: &str) -> i32 {
    eprintln!("error: {e}\n{USAGE}");
    2
}

fn list(store: &BindingStore) -> Result<String, Refusal> {
    let mut out = String::from("issuer\tsubject\tfingerprint\tkind\tbound_by\tbound_at\n");
    for b in store.list()? {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\n",
            b.issuer,
            b.subject,
            b.fingerprint,
            b.kind.as_str(),
            b.bound_by.as_str(),
            b.bound_at
        ));
    }
    Ok(out)
}

fn bind(store: &BindingStore, a: &Args) -> Result<String, Refusal> {
    let (Some(iss), Some(sub), Some(fp), None) = (&a.issuer, &a.subject, &a.fingerprint, &a.kind)
    else {
        return Err(Refusal::Usage(
            "bind needs --issuer, --subject and --fingerprint".into(),
        ));
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    store.bind(iss, sub, fp, BoundBy::Admin, now)?;
    Ok(format!("bound {iss} {sub} -> {fp} (agent, admin)\n"))
}

fn unbind(store: &BindingStore, a: &Args) -> Result<String, Refusal> {
    let sel = selector(a).map_err(Refusal::Usage)?;
    let b = store.unbind(&sel)?;
    Ok(format!(
        "unbound {} {} -> {}\n",
        b.issuer, b.subject, b.fingerprint
    ))
}

fn set_kind(store: &BindingStore, a: &Args) -> Result<String, Refusal> {
    let kind = a
        .kind
        .as_deref()
        .and_then(PrincipalKind::parse)
        .ok_or_else(|| Refusal::Usage("--kind must be person or agent".into()))?;
    let sel = selector(a).map_err(Refusal::Usage)?;
    let b = store.set_kind(&sel, kind)?;
    Ok(format!(
        "{} {} -> {} is now {}\n",
        b.issuer,
        b.subject,
        b.fingerprint,
        kind.as_str()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_binding_is_named_exactly_one_way() {
        let both = parse(&s(&[
            "--fingerprint",
            "f",
            "--issuer",
            "i",
            "--subject",
            "x",
        ]))
        .unwrap();
        assert!(selector(&both).is_err(), "two selectors must be refused");
        let half = parse(&s(&["--issuer", "i"])).unwrap();
        assert!(
            selector(&half).is_err(),
            "issuer without subject names nothing"
        );
        let fp = parse(&s(&["--fingerprint=f"])).unwrap();
        assert_eq!(selector(&fp), Ok(Selector::Fingerprint("f".into())));
    }

    #[test]
    fn unknown_flags_and_missing_values_are_usage_errors() {
        assert!(parse(&s(&["--fingerprnt", "f"])).is_err());
        assert!(parse(&s(&["--kind"])).is_err());
    }
}
