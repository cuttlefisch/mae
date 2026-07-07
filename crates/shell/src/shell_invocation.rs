//! Invoking the user's login+interactive shell to source startup files
//! (`.bashrc`, `.zshrc`, etc.) before running a target program or probing
//! its resulting environment.
//!
//! Background: a process launched directly (no shell in between) never
//! benefits from whatever a user's shell startup files establish — auth
//! tokens pulled from a password manager, PATH additions from version
//! managers (nvm/asdf/pyenv), SSH/GPG agent sockets, and so on. MAE's
//! `open-ai-agent` used to launch its target program this way, so it
//! silently lacked anything the user's normal terminal has.
//!
//! Bash's startup-file selection is if/elif, not independent flags: `-l`
//! (login) reads the profile chain (`/etc/profile`, then the first of
//! `~/.bash_profile` / `~/.bash_login` / `~/.profile`); "interactive and
//! NOT login" is the only branch that reads `~/.bashrc`. So `-i -l -c`
//! together does NOT reliably source `.bashrc` — it only appears to work
//! when a user's own `.bash_profile` happens to chain `. ~/.bashrc`, which
//! is a personal-dotfiles convention, not something to rely on generally.
//! [`login_wrapped_argv`] explicitly sources `~/.bashrc` for bash regardless
//! of profile chaining. Non-bash shells (zsh) rely on `-i -l -c` alone
//! (zsh's `.zprofile`/`.zshrc` are documented as independent, unlike bash's
//! if/elif model) — see tracking issue for broader shell verification.

use std::collections::HashMap;
use std::io;
use std::sync::mpsc;
use std::time::Duration;

/// Build argv for invoking `shell_path` as a login+interactive shell that,
/// after sourcing startup files, execs `program` with `args` via positional
/// parameters (`$0`/`$@`) rather than string interpolation — so arguments
/// containing spaces/quotes pass through byte-for-byte, with no escaping
/// required and no injection risk.
///
/// Returns `(shell_path, full_argv)` ready to hand to a process/PTY spawn
/// API that takes a program + arg list.
pub fn login_wrapped_argv(
    shell_path: &str,
    program: &str,
    args: &[String],
) -> (String, Vec<String>) {
    let script = if is_bash(shell_path) {
        r#"[ -f ~/.bashrc ] && . ~/.bashrc; exec "$0" "$@""#
    } else {
        r#"exec "$0" "$@""#
    };
    let mut wrapped = vec![
        "-i".to_string(),
        "-l".to_string(),
        "-c".to_string(),
        script.to_string(),
        program.to_string(),
    ];
    wrapped.extend(args.iter().cloned());
    (shell_path.to_string(), wrapped)
}

fn is_bash(shell_path: &str) -> bool {
    std::path::Path::new(shell_path)
        .file_name()
        .and_then(|n| n.to_str())
        == Some("bash")
}

/// Resolve the user's shell the same way the rest of this crate does:
/// `$SHELL`, falling back to a sane platform default if unset.
pub fn resolve_user_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| {
        if cfg!(target_os = "macos") {
            "/bin/zsh".to_string()
        } else {
            "/bin/sh".to_string()
        }
    })
}

/// Probe `shell_path`'s environment after sourcing startup files, non-
/// interactively (no PTY needed — this is a diagnostic, not a real spawn).
/// Timeout-guarded so a pathological/hanging rc file can't block a caller
/// for more than a few seconds.
pub fn probe_login_shell_env(shell_path: &str) -> Result<HashMap<String, String>, String> {
    let (_, args) = login_wrapped_argv(shell_path, "env", &["-0".to_string()]);
    let shell_path = shell_path.to_string();

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let out = std::process::Command::new(&shell_path).args(&args).output();
        let _ = tx.send(out);
    });
    // Observed ~2.8s in practice for a real user's `.bashrc` (gpg-agent/pass
    // lookup) — leave real headroom above that rather than a marginal 3s.
    let output = match rx.recv_timeout(Duration::from_secs(8)) {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("failed to spawn shell: {e}")),
        Err(_) => return Err("timed out waiting for shell (rc file may be hanging)".to_string()),
    };
    if !output.status.success() {
        return Err(format!(
            "shell exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(parse_nul_separated_env(&output.stdout))
}

fn parse_nul_separated_env(bytes: &[u8]) -> HashMap<String, String> {
    bytes
        .split(|b| *b == 0)
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| {
            let s = String::from_utf8_lossy(entry);
            s.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect()
}

/// Keys whose value should never be echoed back verbatim in a diagnostic —
/// matched case-insensitively against the key name.
const SENSITIVE_KEY_MARKERS: &[&str] = &["TOKEN", "KEY", "SECRET", "PASSWORD"];

fn is_sensitive_key(key: &str) -> bool {
    let upper = key.to_uppercase();
    SENSITIVE_KEY_MARKERS.iter().any(|m| upper.contains(m))
}

/// `(key, display_value)` pairs — `display_value` is already redacted for
/// sensitive-looking keys.
pub type EnvKeyValues = Vec<(String, String)>;

/// Diff `login` (what a login+interactive shell would see) against
/// `ambient` (what the current process already sees) — the keys that
/// startup-file sourcing would add or change. Sensitive-looking values are
/// redacted so a diagnostic report never leaks a real secret.
pub fn diff_envs(
    ambient: &HashMap<String, String>,
    login: &HashMap<String, String>,
) -> (EnvKeyValues, EnvKeyValues) {
    let mut added = Vec::new();
    let mut changed = Vec::new();
    for (k, v) in login {
        let display = if is_sensitive_key(k) {
            "<redacted>".to_string()
        } else {
            v.clone()
        };
        match ambient.get(k) {
            None => added.push((k.clone(), display)),
            Some(cur) if cur != v => changed.push((k.clone(), display)),
            _ => {}
        }
    }
    added.sort();
    changed.sort();
    (added, changed)
}

/// Convenience: `io::Error` wrapper for callers that want a uniform error
/// type instead of `probe_login_shell_env`'s `String`.
pub fn probe_login_shell_env_io(shell_path: &str) -> io::Result<HashMap<String, String>> {
    probe_login_shell_env(shell_path).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_wrapped_argv_uses_positional_args_not_interpolation() {
        let (shell, args) = login_wrapped_argv(
            "/usr/bin/zsh",
            "claude",
            &["--resume".to_string(), "arg with spaces".to_string()],
        );
        assert_eq!(shell, "/usr/bin/zsh");
        assert_eq!(
            args,
            vec![
                "-i",
                "-l",
                "-c",
                r#"exec "$0" "$@""#,
                "claude",
                "--resume",
                "arg with spaces",
            ]
        );
    }

    #[test]
    fn login_wrapped_argv_adds_bashrc_guard_only_for_bash() {
        let (_, bash_args) = login_wrapped_argv("/bin/bash", "claude", &[]);
        assert!(
            bash_args[3].contains(".bashrc"),
            "bash must get the explicit .bashrc guard"
        );

        let (_, zsh_args) = login_wrapped_argv("/bin/zsh", "claude", &[]);
        assert!(
            !zsh_args[3].contains(".bashrc"),
            "non-bash shells must not get the bash-specific guard"
        );

        // A path like "/usr/bin/bash" (not just "bash") must still match.
        let (_, nested_bash_args) = login_wrapped_argv("/usr/bin/bash", "claude", &[]);
        assert!(nested_bash_args[3].contains(".bashrc"));
    }

    #[test]
    fn login_wrapped_argv_never_reinterprets_special_chars_in_program_or_args() {
        // Adversarial: args crafted to break naive string interpolation.
        let adversarial = vec![
            "arg\"with\"quotes".to_string(),
            "arg'with'ticks".to_string(),
            "arg;rm -rf /".to_string(),
            "arg$(whoami)".to_string(),
        ];
        let (_, args) = login_wrapped_argv("/bin/bash", "printf", &adversarial);
        // The positional args must appear byte-for-byte, unmodified, as
        // separate argv entries — never concatenated/re-parsed.
        assert_eq!(&args[5..], adversarial.as_slice());
    }

    #[test]
    fn diff_envs_reports_added_and_changed_and_redacts_sensitive_keys() {
        let mut ambient = HashMap::new();
        ambient.insert("EDITOR".to_string(), "vim".to_string());
        ambient.insert("UNCHANGED".to_string(), "same".to_string());

        let mut login = HashMap::new();
        login.insert("EDITOR".to_string(), "emacs".to_string()); // changed
        login.insert("UNCHANGED".to_string(), "same".to_string()); // unchanged
        login.insert(
            "CLAUDE_CODE_OAUTH_TOKEN".to_string(),
            "sk-super-secret".to_string(),
        ); // added, sensitive
        login.insert("MY_PROJECT_DIR".to_string(), "/home/user/proj".to_string()); // added, not sensitive

        let (added, changed) = diff_envs(&ambient, &login);

        assert_eq!(
            added,
            vec![
                (
                    "CLAUDE_CODE_OAUTH_TOKEN".to_string(),
                    "<redacted>".to_string()
                ),
                ("MY_PROJECT_DIR".to_string(), "/home/user/proj".to_string()),
            ]
        );
        assert_eq!(changed, vec![("EDITOR".to_string(), "emacs".to_string())]);
    }

    #[test]
    fn diff_envs_empty_when_identical() {
        let mut env = HashMap::new();
        env.insert("A".to_string(), "1".to_string());
        let (added, changed) = diff_envs(&env, &env);
        assert!(added.is_empty());
        assert!(changed.is_empty());
    }

    #[test]
    fn parse_nul_separated_env_handles_values_with_newlines() {
        let raw = b"A=1\0B=multi\nline\0\0C=3\0";
        let parsed = parse_nul_separated_env(raw);
        assert_eq!(parsed.get("A"), Some(&"1".to_string()));
        assert_eq!(parsed.get("B"), Some(&"multi\nline".to_string()));
        assert_eq!(parsed.get("C"), Some(&"3".to_string()));
    }
}
