//! `mae-daemon token mint` — the operator-issued bearer token of ADR-111 P1.
//!
//! How a principal without collab mTLS -- a headless service account, a script,
//! a browser user handed a link -- obtains a token for the HTTPS listener's
//! `kb/query.*` surface without an external identity provider. The token is the
//! same self-issued EdDSA token `kb/query.self_token` mints over mTLS (ADR-067
//! D3, `oauth_self_issue::mint_self_token`), signed by this daemon's identity,
//! audience-bound to `oauth.canonical_resource_uri`, and expiring.
//!
//! @ai-caution: [security] This is an OPERATOR command and must stay one
//! (CLAUDE.md principle #16). Running it requires reading the daemon's private
//! key, and it mints for an arbitrary `--sub`: exposing it as an MCP tool or an
//! RPC would hand any caller of that surface the ability to impersonate any
//! principal. `kb/query.self_token` is the safe network form -- it mints only
//! for the mTLS-verified caller's own fingerprint.
//!
//! Revocation is by expiry or by rotating the identity key (which invalidates
//! EVERY self-issued token at once); there is no per-token revocation list.
//! That is why the lifetime has a configured ceiling this command refuses to
//! exceed rather than silently clamping.

use mae_daemon::oauth_self_issue::mint_self_token;
use mae_mcp::identity::Identity;

use crate::config::DaemonConfig;

const USAGE: &str = "usage: mae-daemon token mint --sub <principal> [--ttl <duration>]\n\
     \x20 <duration>: seconds, or a number with s/m/h/d (e.g. 900, 15m, 8h)";

/// `mae-daemon token <action> …`. Returns the process exit code: 0 minted,
/// 1 refused (configuration/identity), 2 usage.
pub(crate) fn run_token(config: &DaemonConfig, rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("mint") => match parse_mint_args(&rest[1..]) {
            Ok(args) => mint(config, &args),
            Err(e) => {
                eprintln!("error: {e}\n{USAGE}");
                2
            }
        },
        _ => {
            eprintln!("{USAGE}");
            2
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct MintArgs {
    sub: String,
    /// The raw `--ttl` value; `None` = the configured default.
    ttl: Option<String>,
}

/// Parse `--sub X` / `--sub=X` and `--ttl Y` / `--ttl=Y`. Unknown flags are
/// errors: an operator's typo must not mint a token with a default they did not
/// ask for.
fn parse_mint_args(args: &[String]) -> Result<MintArgs, String> {
    let mut sub = None;
    let mut ttl = None;
    let mut i = 0;
    while i < args.len() {
        let (flag, inline) = match args[i].split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (args[i].as_str(), None),
        };
        let value = match inline {
            Some(v) => v,
            None => {
                i += 1;
                args.get(i)
                    .cloned()
                    .ok_or_else(|| format!("{flag} needs a value"))?
            }
        };
        match flag {
            "--sub" => sub = Some(value),
            "--ttl" => ttl = Some(value),
            other => return Err(format!("unknown argument '{other}'")),
        }
        i += 1;
    }
    let sub = sub
        .filter(|s| !s.trim().is_empty())
        .ok_or("--sub <principal> is required")?;
    Ok(MintArgs { sub, ttl })
}

/// Parse a lifetime: bare seconds, or a positive integer suffixed s/m/h/d.
fn parse_duration_secs(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (digits, unit) = match s.char_indices().last() {
        Some((i, c)) if c.is_ascii_alphabetic() => (&s[..i], c),
        _ => (s, 's'),
    };
    let n: u64 = digits
        .parse()
        .map_err(|_| format!("'{s}' is not a duration"))?;
    let scale = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        _ => return Err(format!("'{s}': unknown unit '{unit}' (use s, m, h or d)")),
    };
    match n.checked_mul(scale) {
        Some(0) => Err("a token lifetime must be greater than zero".to_string()),
        Some(secs) => Ok(secs),
        None => Err(format!("'{s}' is out of range")),
    }
}

/// Everything that must hold for the running listener to ACCEPT the token --
/// checked before minting, so an operator is never handed a token that would
/// only fail later. Returns the audience to mint for.
fn listener_would_accept(config: &DaemonConfig) -> Result<&str, String> {
    let o = &config.oauth;
    if !o.enabled {
        return Err("oauth.enabled is false: there is no HTTPS listener to present it to".into());
    }
    if !o.self_issued_tokens_enabled {
        return Err(
            "oauth.self_issued_tokens_enabled is false: the listener would refuse this token"
                .into(),
        );
    }
    if o.canonical_resource_uri.is_empty() {
        return Err("oauth.canonical_resource_uri is unset: there is no audience to bind".into());
    }
    if !config.collab.enabled || config.collab.auth.mode != "key" {
        return Err(
            "self-issued tokens are validated against the key-mode identity: they need \
             collab.enabled = true and collab.auth.mode = \"key\""
                .into(),
        );
    }
    Ok(&o.canonical_resource_uri)
}

/// Load the daemon identity WITHOUT ever creating one (#652: a command that
/// only needs to read the key must not leave a fresh one behind -- that is how
/// `--check-config` once left a root-owned key the service could not read).
fn load_identity(config: &DaemonConfig) -> Result<Identity, String> {
    let dir = config
        .collab
        .auth
        .identity_dir()
        .ok_or("cannot resolve the identity dir (set collab.auth.identity_dir)")?;
    let path = dir.join("id_ed25519");
    Identity::load_secret(&dir, "daemon").ok_or_else(|| {
        if path.exists() {
            format!(
                "the daemon identity at {} is unreadable or malformed",
                path.display()
            )
        } else {
            format!(
                "no daemon identity at {} -- start the daemon once, or run \
                 `mae-daemon identity`, to create it",
                path.display()
            )
        }
    })
}

fn mint(config: &DaemonConfig, args: &MintArgs) -> i32 {
    match try_mint(config, args) {
        Ok(token) => {
            // stdout is the token and nothing else, so it can be piped straight
            // into a secret store. Everything else went to stderr.
            println!("{token}");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn try_mint(config: &DaemonConfig, args: &MintArgs) -> Result<String, String> {
    let audience = listener_would_accept(config)?;
    let max = config.oauth.self_issued_token_max_ttl_secs;
    let ttl = match &args.ttl {
        Some(raw) => parse_duration_secs(raw)?,
        None => config.oauth.self_issued_token_ttl_secs,
    };
    if ttl > max {
        return Err(format!(
            "a {ttl}s lifetime exceeds oauth.self_issued_token_max_ttl_secs ({max}s); \
             self-issued tokens cannot be revoked individually, so the ceiling is not \
             exceeded -- ask for less, or raise it in daemon.toml"
        ));
    }
    let identity = load_identity(config)?;
    let token = mint_self_token(&identity, &args.sub, audience, ttl)?;
    eprintln!(
        "minted a token for sub={} aud={audience}, valid {ttl}s (signed by {})",
        args.sub,
        identity.fingerprint()
    );
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// The shipped defaults must be mintable: a default lifetime above the
    /// default ceiling would make `token mint` without `--ttl` refuse out of
    /// the box.
    #[test]
    fn the_default_lifetime_is_within_the_default_ceiling() {
        let d = crate::oauth_config::OAuthConfig::default();
        assert!(d.self_issued_token_ttl_secs <= d.self_issued_token_max_ttl_secs);
    }

    #[test]
    fn durations_parse_in_every_unit_and_reject_the_rest() {
        for (s, want) in [
            ("1", 1u64),
            ("90", 90),
            ("90s", 90),
            ("15m", 900),
            ("8h", 28_800),
            ("2d", 172_800),
        ] {
            assert_eq!(parse_duration_secs(s), Ok(want), "{s}");
        }
        for s in [
            "",
            "0",
            "0h",
            "-5",
            "1.5h",
            "h",
            "10w",
            "ten",
            "18446744073709551615d",
        ] {
            assert!(parse_duration_secs(s).is_err(), "{s:?} must be refused");
        }
    }

    #[test]
    fn mint_args_need_a_non_empty_sub_and_reject_unknown_flags() {
        assert_eq!(
            parse_mint_args(&strings(&["--sub", "svc:a", "--ttl", "1h"])),
            Ok(MintArgs {
                sub: "svc:a".into(),
                ttl: Some("1h".into())
            })
        );
        assert_eq!(
            parse_mint_args(&strings(&["--ttl=5m", "--sub=svc:b"])),
            Ok(MintArgs {
                sub: "svc:b".into(),
                ttl: Some("5m".into())
            })
        );
        for bad in [
            &[][..],
            &["--sub"],
            &["--sub", "  "],
            &["--sub", "a", "--aud", "x"],
            &["svc:a"],
        ] {
            assert!(parse_mint_args(&strings(bad)).is_err(), "{bad:?}");
        }
    }
}
