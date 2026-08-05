//! Command-line argument parsing for `mae-daemon`.
//!
//! @ai-caution: [multi-instance] Global flags MUST be parsed BEFORE subcommand
//! dispatch, and every subcommand MUST resolve its config through [`Cli`] rather
//! than calling `DaemonConfig::load()` itself. Until 2026-08 they did the latter,
//! so `doctor`, `--check-config`, `keygen`, `keys`, `identity`, `authorized`,
//! `authorize` and `revoke` all silently read the DEFAULT config no matter what
//! `--config` said. On a host running two instances (staging + production, the
//! shape `assets/mae-daemon@.service` is built for) that meant every
//! administrative command operated on whichever instance owned the default
//! config — you could not validate the other one's config, and `authorize` wrote
//! the peer key into the wrong keystore. See `two_instances_do_not_share_any_path`.

use crate::config::DaemonConfig;
use std::path::PathBuf;

/// Parsed command line: the subcommand (if any), its positional arguments, and
/// the global flags that select WHICH daemon instance the command applies to.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Cli {
    /// The subcommand token (`doctor`, `keygen`, …), if one was given.
    pub subcommand: Option<String>,
    /// Positional arguments following the subcommand, with global flags removed
    /// so a subcommand that joins them into one string (`authorize`) cannot be
    /// corrupted by an interleaved `--config <path>`.
    pub rest: Vec<String>,
    /// `--config <path>` — the instance's config file.
    pub config: Option<PathBuf>,
    /// `--bind <addr>` — override `collab.bind`.
    pub bind: Option<String>,
    /// `--data-dir <path>` — override `data_dir`.
    pub data_dir: Option<PathBuf>,
    /// `--socket <path>` — override the KB Unix socket path.
    pub socket: Option<PathBuf>,
    /// `--oauth-bind <addr>` — override `oauth.bind`.
    pub oauth_bind: Option<String>,
    /// `doctor --compare-with <path>` — a SECOND instance's config, checked for
    /// resources it shares with this one. The staging-vs-production question an
    /// operator actually has, which no single-instance report can answer.
    pub compare_with: Option<PathBuf>,
    /// `--version` / `-V`.
    pub version: bool,
    /// `--check-config`.
    pub check_config: bool,
}

impl Cli {
    /// Parse `args` (INCLUDING argv[0], which is skipped).
    ///
    /// Global flags are recognised wherever they appear — before or after the
    /// subcommand — because `mae-daemon doctor --config X` is the order an
    /// operator naturally types, while the systemd unit puts `--config` first.
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Self {
        let args: Vec<String> = args.into_iter().skip(1).collect();
        let mut cli = Cli::default();
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_str();
            // A flag that takes a value only consumes one when a value is
            // actually present; a dangling `--config` at the end is left in
            // `rest` rather than silently swallowing nothing.
            let mut take_value = |target: &mut Option<String>| {
                if i + 1 < args.len() {
                    *target = Some(args[i + 1].clone());
                    i += 2;
                    true
                } else {
                    false
                }
            };
            let consumed = match a {
                "--config" => {
                    let mut v = None;
                    let ok = take_value(&mut v);
                    cli.config = v.map(PathBuf::from);
                    ok
                }
                "--bind" => take_value(&mut cli.bind),
                "--oauth-bind" => take_value(&mut cli.oauth_bind),
                "--compare-with" => {
                    let mut v = None;
                    let ok = take_value(&mut v);
                    cli.compare_with = v.map(PathBuf::from);
                    ok
                }
                "--data-dir" => {
                    let mut v = None;
                    let ok = take_value(&mut v);
                    cli.data_dir = v.map(PathBuf::from);
                    ok
                }
                "--socket" => {
                    let mut v = None;
                    let ok = take_value(&mut v);
                    cli.socket = v.map(PathBuf::from);
                    ok
                }
                "--version" | "-V" => {
                    cli.version = true;
                    i += 1;
                    true
                }
                "--check-config" => {
                    cli.check_config = true;
                    i += 1;
                    true
                }
                _ => false,
            };
            if consumed {
                continue;
            }
            // Not a global flag: the first such token is the subcommand, the
            // rest are its positional arguments (order preserved).
            if cli.subcommand.is_none() && !a.starts_with('-') {
                cli.subcommand = Some(a.to_string());
            } else {
                cli.rest.push(a.to_string());
            }
            i += 1;
        }
        cli
    }

    /// Load the config this invocation names, then apply the CLI overrides.
    ///
    /// An override that cannot be parsed is a hard error rather than a silent
    /// fallback to the config value: an operator who passed `--bind` and got the
    /// config's address instead is being told the wrong thing about which
    /// instance they just talked to.
    pub fn resolve_config(&self) -> Result<DaemonConfig, String> {
        let mut config = match &self.config {
            Some(path) => {
                if !path.exists() {
                    return Err(format!("config file not found: {}", path.display()));
                }
                DaemonConfig::load_from(path)
            }
            None => DaemonConfig::load(),
        };
        if let Some(addr) = &self.bind {
            config.collab.bind = addr
                .parse()
                .map_err(|e| format!("--bind {addr}: not a socket address ({e})"))?;
        }
        if let Some(addr) = &self.oauth_bind {
            config.oauth.bind = addr
                .parse()
                .map_err(|e| format!("--oauth-bind {addr}: not a socket address ({e})"))?;
        }
        if let Some(dir) = &self.data_dir {
            config.data_dir = Some(dir.clone());
        }
        if let Some(sock) = &self.socket {
            config.socket = sock.clone();
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(args: &[&str]) -> Cli {
        let mut v = vec!["mae-daemon".to_string()];
        v.extend(args.iter().map(|s| s.to_string()));
        Cli::parse(v)
    }

    #[test]
    fn config_flag_is_seen_after_a_subcommand() {
        // The regression this module exists for: the old parser only looked for
        // `--config` on a path that `doctor` had already returned from.
        let c = cli(&["doctor", "--config", "/etc/mae/daemon-prod.toml"]);
        assert_eq!(c.subcommand.as_deref(), Some("doctor"));
        assert_eq!(
            c.config,
            Some(PathBuf::from("/etc/mae/daemon-prod.toml")),
            "--config after a subcommand must still select the instance"
        );
    }

    #[test]
    fn config_flag_is_seen_before_a_subcommand() {
        let c = cli(&["--config", "/etc/mae/daemon-prod.toml", "doctor"]);
        assert_eq!(c.subcommand.as_deref(), Some("doctor"));
        assert_eq!(c.config, Some(PathBuf::from("/etc/mae/daemon-prod.toml")));
    }

    #[test]
    fn global_flags_are_stripped_from_subcommand_positionals() {
        // `authorize` joins `rest` into a single key line; a leaked `--config
        // <path>` in the middle would produce a corrupt, unparseable key.
        let c = cli(&[
            "authorize",
            "mae-ed25519",
            "--config",
            "/etc/mae/daemon-staging.toml",
            "AAAA",
            "laptop",
        ]);
        assert_eq!(c.subcommand.as_deref(), Some("authorize"));
        assert_eq!(c.rest, vec!["mae-ed25519", "AAAA", "laptop"]);
        assert_eq!(
            c.rest.join(" "),
            "mae-ed25519 AAAA laptop",
            "the reconstructed key line must not contain the global flag"
        );
        assert_eq!(
            c.config,
            Some(PathBuf::from("/etc/mae/daemon-staging.toml"))
        );
    }

    #[test]
    fn subcommand_local_flags_are_preserved_in_order() {
        // `--from-ssh-pub` belongs to `authorize`, not to us: pass it through
        // untouched, with its value still adjacent.
        let c = cli(&[
            "authorize",
            "--from-ssh-pub",
            "/home/u/.ssh/id_ed25519.pub",
            "laptop",
        ]);
        assert_eq!(c.subcommand.as_deref(), Some("authorize"));
        assert_eq!(
            c.rest,
            vec!["--from-ssh-pub", "/home/u/.ssh/id_ed25519.pub", "laptop"]
        );
    }

    #[test]
    fn a_dangling_value_flag_does_not_swallow_the_end_of_the_line() {
        let c = cli(&["doctor", "--config"]);
        assert_eq!(c.config, None);
        assert_eq!(c.rest, vec!["--config"], "left visible, not silently eaten");
    }

    #[test]
    fn every_instance_selecting_flag_is_parsed() {
        let c = cli(&[
            "--config",
            "/c.toml",
            "--socket",
            "/run/s.sock",
            "--data-dir",
            "/var/d",
            "--bind",
            "0.0.0.0:9474",
            "--oauth-bind",
            "0.0.0.0:8443",
        ]);
        assert_eq!(c.config, Some(PathBuf::from("/c.toml")));
        assert_eq!(c.socket, Some(PathBuf::from("/run/s.sock")));
        assert_eq!(c.data_dir, Some(PathBuf::from("/var/d")));
        assert_eq!(c.bind.as_deref(), Some("0.0.0.0:9474"));
        assert_eq!(c.oauth_bind.as_deref(), Some("0.0.0.0:8443"));
        assert_eq!(c.subcommand, None);
    }

    #[test]
    fn version_and_check_config_are_recognised_anywhere() {
        assert!(cli(&["--version"]).version);
        assert!(cli(&["-V"]).version);
        assert!(cli(&["--check-config", "--config", "/c.toml"]).check_config);
        assert!(cli(&["--config", "/c.toml", "--check-config"]).check_config);
    }

    #[test]
    fn a_missing_config_file_is_an_error_not_a_silent_default() {
        // The failure mode being closed: `load_from` warns to stderr and returns
        // defaults, so a typo'd path used to look like a healthy default config
        // — the operator would read a report about an instance that isn't the
        // one they named.
        let c = cli(&["--config", "/nonexistent/definitely-not-here.toml"]);
        let err = c.resolve_config().expect_err("must not fall back silently");
        assert!(err.contains("not found"), "got: {err}");
    }

    #[test]
    fn an_unparseable_bind_override_is_rejected() {
        for bad in ["not-an-address", "9474", "0.0.0.0:99999"] {
            let c = cli(&["--bind", bad]);
            assert!(
                c.resolve_config().is_err(),
                "--bind {bad} must fail loudly, not fall back to the config value"
            );
        }
        for bad in ["not-an-address", "8443"] {
            let c = cli(&["--oauth-bind", bad]);
            assert!(c.resolve_config().is_err(), "--oauth-bind {bad}");
        }
    }

    #[test]
    fn overrides_are_applied_to_the_resolved_config() {
        let c = cli(&[
            "--socket",
            "/run/mae-prod.sock",
            "--data-dir",
            "/srv/mae/prod",
            "--bind",
            "10.0.0.1:9474",
            "--oauth-bind",
            "10.0.0.1:8443",
        ]);
        let resolved = c.resolve_config().expect("valid overrides");
        assert_eq!(resolved.socket, PathBuf::from("/run/mae-prod.sock"));
        assert_eq!(resolved.data_dir, Some(PathBuf::from("/srv/mae/prod")));
        assert_eq!(resolved.collab.bind.to_string(), "10.0.0.1:9474");
        assert_eq!(resolved.oauth.bind.to_string(), "10.0.0.1:8443");
    }
}
