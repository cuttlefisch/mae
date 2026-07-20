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
//! of profile chaining. Zsh relies on `-i -l -c` alone (`.zprofile`/`.zshrc`
//! are documented as independent, unlike bash's if/elif model) — not
//! empirically verified against a live zsh in this environment, see
//! tracking issue for broader shell verification.
//!
//! This is the SINGLE rc-sourcing implementation for the crate —
//! [`login_shell_script_argv`] is the shared primitive
//! [`probe_login_shell_env`] and every other "run a script under the user's
//! login+interactive shell" caller (`terminal.rs`'s PATH probe, `path.rs`'s
//! PATH pull) route through, rather than each hand-rolling their own
//! `-l -i -c` invocation with independently-incomplete per-shell handling
//! (issue #291 — only this module had the bash `.bashrc` guard; the other
//! two call sites lacked it entirely, and one had unreachable fish-specific
//! branch scaffolding that ran byte-identical code in both arms).

use std::collections::HashMap;
use std::io;
use std::sync::mpsc;
use std::time::Duration;

/// Which shell family `shell_path` belongs to, for startup-file-sourcing
/// purposes. Classified by the executable's file name (matching the crate's
/// existing convention — `$SHELL` and PTY spawn args are always full paths,
/// never just a bare command looked up on `PATH`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellKind {
    Bash,
    Fish,
    /// zsh, sh, dash, and anything else — treated like zsh: `-i -l -c`
    /// alone is trusted to source startup files correctly.
    Other,
}

fn classify_shell(shell_path: &str) -> ShellKind {
    match std::path::Path::new(shell_path)
        .file_name()
        .and_then(|n| n.to_str())
    {
        Some("bash") => ShellKind::Bash,
        Some("fish") => ShellKind::Fish,
        _ => ShellKind::Other,
    }
}

/// The startup-file-sourcing prefix to prepend to a `-c` script body, so it
/// reliably sources the user's interactive rc file regardless of what the
/// script itself does. Empty for shells where `-i -l` alone already does
/// the job (see the module doc comment).
fn startup_sourcing_prefix(kind: ShellKind) -> &'static str {
    match kind {
        ShellKind::Bash => r#"[ -f ~/.bashrc ] && . ~/.bashrc; "#,
        ShellKind::Fish | ShellKind::Other => "",
    }
}

/// Build `(shell_path, argv)` for running `script_body` as a `-c` command
/// under `shell_path` as a login+interactive shell, with the startup-file
/// guard for the shell's family prepended. `script_body` must be a single
/// self-contained command string (no positional-parameter placeholders) —
/// use [`login_wrapped_argv`] instead when the goal is execing a separate
/// program with its own argv.
pub fn login_shell_script_argv(shell_path: &str, script_body: &str) -> (String, Vec<String>) {
    let kind = classify_shell(shell_path);
    let script = format!("{}{}", startup_sourcing_prefix(kind), script_body);
    (
        shell_path.to_string(),
        vec!["-i".to_string(), "-l".to_string(), "-c".to_string(), script],
    )
}

/// Build argv for invoking `shell_path` as a login+interactive shell that,
/// after sourcing startup files, execs `program` with `args` via positional
/// parameters rather than string interpolation — so arguments containing
/// spaces/quotes pass through byte-for-byte, with no escaping required and
/// no injection risk.
///
/// Bash/zsh/sh/dash all support the POSIX `sh -c 'CMD' arg0 arg1 arg2`
/// convention (`$0`/`"$@"` inside `CMD` bind to the trailing args). Fish
/// does NOT: `fish -c 'CMD' arg1 arg2` makes ALL trailing args available as
/// `$argv` (no separate `$0`), so fish needs its own exec tail
/// (`$argv[1]` / `$argv[2..]`) — implemented per fish's documented `-c`/
/// `$argv` semantics, but not empirically verified against a live fish in
/// this environment (none installed, no package-install authorization
/// available at implementation time) — see tracking issue.
///
/// Returns `(shell_path, full_argv)` ready to hand to a process/PTY spawn
/// API that takes a program + arg list.
pub fn login_wrapped_argv(
    shell_path: &str,
    program: &str,
    args: &[String],
) -> (String, Vec<String>) {
    let kind = classify_shell(shell_path);
    let exec_tail = match kind {
        ShellKind::Fish => r#"exec $argv[1] $argv[2..]"#,
        ShellKind::Bash | ShellKind::Other => r#"exec "$0" "$@""#,
    };
    let script = format!("{}{}", startup_sourcing_prefix(kind), exec_tail);
    let mut wrapped = vec![
        "-i".to_string(),
        "-l".to_string(),
        "-c".to_string(),
        script,
        program.to_string(),
    ];
    wrapped.extend(args.iter().cloned());
    (shell_path.to_string(), wrapped)
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
    fn login_wrapped_argv_uses_fish_argv_syntax_not_posix_positional_params() {
        // #291: fish's `-c 'CMD' arg1 arg2` makes ALL trailing args available
        // as $argv (no separate $0 the way sh/bash/zsh have) — the POSIX
        // `exec "$0" "$@"` tail is invalid fish syntax and would silently
        // break if fish ever hit that path (the pre-fix code never
        // special-cased fish at all).
        let (shell, args) = login_wrapped_argv(
            "/usr/bin/fish",
            "claude",
            &["--resume".to_string(), "arg with spaces".to_string()],
        );
        assert_eq!(shell, "/usr/bin/fish");
        assert_eq!(args[3], r#"exec $argv[1] $argv[2..]"#);
        assert!(
            !args[3].contains("\"$0\"") && !args[3].contains("\"$@\""),
            "fish must never get the POSIX positional-param tail"
        );
        // Program + args still travel as separate, unmodified argv entries —
        // only the SCRIPT body differs per shell, not how program/args are
        // passed to the shell process.
        assert_eq!(&args[4..], &["claude", "--resume", "arg with spaces"]);
        // Fish is not bash, so no .bashrc guard either.
        assert!(!args[3].contains(".bashrc"));
    }

    #[test]
    fn login_shell_script_argv_applies_the_same_bashrc_guard_as_login_wrapped_argv() {
        let (shell, args) = login_shell_script_argv("/bin/bash", "echo $PATH");
        assert_eq!(shell, "/bin/bash");
        assert_eq!(
            args,
            vec![
                "-i",
                "-l",
                "-c",
                r#"[ -f ~/.bashrc ] && . ~/.bashrc; echo $PATH"#,
            ]
        );

        let (_, zsh_args) = login_shell_script_argv("/bin/zsh", "echo $PATH");
        assert_eq!(zsh_args[3], "echo $PATH", "zsh gets no extra prefix");

        let (_, fish_args) = login_shell_script_argv("/usr/bin/fish", "echo $PATH");
        assert_eq!(
            fish_args[3], "echo $PATH",
            "fish's -c script itself doesn't need the exec-tail treatment \
             login_wrapped_argv needs — only a plain script body here"
        );
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
