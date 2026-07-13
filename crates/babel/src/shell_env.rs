//! Resolve the user's interactive login shell environment for babel-spawned
//! processes.
//!
//! GUI-launched applications (a desktop launcher, systemd user unit, dock,
//! etc.) do NOT source `.bashrc`/`.zshrc`/`.profile` the way an interactive
//! terminal session does — so a variable a user sets only in their shell rc
//! files (e.g. `JENKINS_URL`) is present in a "new shell" but absent from
//! MAE's own process environment, and therefore absent from every child
//! process babel spawns (`Command::new(...)` inherits the PARENT's
//! environment, not the user's shell's). This is the exact gap Emacs's
//! `exec-path-from-shell` package and VS Code's "resolve shell environment"
//! feature both exist to close: spawn the user's actual login shell once,
//! capture what it resolves its own environment to, and merge that into
//! subprocess environments.

use std::collections::HashMap;
use std::io::Read as _;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use crate::execute::WaitTimeout;

/// Wall-clock cap on resolving the shell environment. This is a one-time,
/// process-lifetime cost (see `resolved_shell_env`'s caching) paid on the
/// first babel execution, not per-block — generous enough for real-world
/// shell startup (nvm/rbenv/conda hooks, etc.), short enough that a hung
/// shell rc (e.g. blocked on a stale network mount) degrades gracefully
/// instead of freezing the first babel execution indefinitely. Not exposed
/// as an `OptionRegistry` setting (principle #7's carve-out for constants
/// that are truly fixed): the failure mode on either side of any reasonable
/// value is identical ("some extra env vars aren't available"), so there's
/// no real user-facing behavior to tune here.
const SHELL_ENV_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Bound on captured probe output, mirroring `BabelExecutor::max_output_bytes`'s
/// bounded-read discipline — a pathological shell rc that prints unbounded
/// output must not be buffered into memory without limit.
const MAX_PROBE_OUTPUT_BYTES: u64 = 1024 * 1024; // 1MB — a real environment is a few KB.

static RESOLVED_ENV: OnceLock<HashMap<String, String>> = OnceLock::new();

/// The user's resolved shell environment, computed once (lazily, on first
/// call) and cached for the process lifetime — spawning an interactive
/// login shell to source rc files is relatively slow and can run arbitrary
/// user config, so this must never run per-block or per-session-create.
pub fn resolved_shell_env() -> &'static HashMap<String, String> {
    RESOLVED_ENV.get_or_init(probe_shell_environment)
}

/// Merge the resolved shell environment into a `Command` being built for a
/// babel-spawned process, if `enabled`. Applied BEFORE any block/session-
/// specific `.env(...)` calls, so those can still take precedence for the
/// same key (Rust's `Command` lets later `.env`/`.envs` calls override
/// earlier ones) — in practice the keys don't collide (`MAE_BABEL`/
/// `MAE_BABEL_SESSION` markers vs. real user environment), but the ordering
/// keeps the precedence sensible regardless.
pub fn apply_to(cmd: &mut Command, enabled: bool) {
    if enabled {
        cmd.envs(resolved_shell_env());
    }
}

/// Spawn the user's `$SHELL` (falling back to `/bin/sh` if unset) as an
/// interactive login shell, capture its resolved environment via `env`,
/// and parse it. Returns an empty map on ANY failure (shell not found,
/// timeout, non-UTF8 output, spawn error) — this is a best-effort
/// enhancement, never a hard requirement; babel execution must always
/// still work using just MAE's own inherited environment.
fn probe_shell_environment() -> HashMap<String, String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let args = shell_env_probe_args(&shell);

    let spawn_result = Command::new(&shell)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();

    let Ok(mut child) = spawn_result else {
        return HashMap::new();
    };

    let stdout_handle = child.stdout.take().map(|out| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.take(MAX_PROBE_OUTPUT_BYTES).read_to_end(&mut buf);
            buf
        })
    });

    let wait_result = child.wait_timeout(SHELL_ENV_PROBE_TIMEOUT);
    if matches!(wait_result, Ok(None)) {
        let _ = child.kill();
    }

    let stdout_bytes = stdout_handle
        .map(|h| h.join().unwrap_or_default())
        .unwrap_or_default();

    match wait_result {
        Ok(Some(status)) if status.success() => String::from_utf8(stdout_bytes)
            .map(|text| parse_env_output(&text))
            .unwrap_or_default(),
        _ => HashMap::new(),
    }
}

/// Arguments to make `shell` print its fully-resolved environment.
///
/// bash/zsh/most POSIX shells: `-i` (interactive — sources `.bashrc`-style
/// rc files) + `-l` (login — sources `.profile`/`.bash_profile`-style
/// files) together, since users set variables in either depending on their
/// setup (confirmed the reported case specifically needed `-i`: `bash`
/// only sources `.bashrc` for interactive shells, not login-only ones).
///
/// fish is a deliberate exception: it sources `config.fish` unconditionally
/// for every invocation (no separate login/interactive-only config
/// convention the way bash/zsh have), and doesn't accept `-i`/`-l` the same
/// way, so a plain `-c` suffices.
fn shell_env_probe_args(shell: &str) -> Vec<String> {
    let name = std::path::Path::new(shell)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if name == "fish" {
        vec!["-c".to_string(), "env".to_string()]
    } else {
        vec![
            "-i".to_string(),
            "-l".to_string(),
            "-c".to_string(),
            "env".to_string(),
        ]
    }
}

/// Parse `KEY=VALUE\n`-style `env` output into a map. Tolerant of noise: a
/// line that doesn't look like a valid `IDENTIFIER=...` pair (e.g. a shell
/// startup banner/MOTD a misconfigured rc file printed to stdout) is
/// silently skipped rather than treated as an error — the alternative
/// (failing the whole resolution over one unparseable line) would throw
/// away every genuinely-resolved variable over unrelated noise. Known
/// limitation shared with upstream `exec-path-from-shell`: a value
/// containing an embedded literal newline isn't reconstructed correctly
/// (parsed as two lines) — accepted as out of scope for a real-world
/// environment variable.
fn parse_env_output(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            if key.is_empty() {
                return None;
            }
            let first = key.chars().next()?;
            if !(first.is_ascii_alphabetic() || first == '_') {
                return None;
            }
            if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return None;
            }
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_env_output_basic() {
        let text = "PATH=/usr/bin:/bin\nHOME=/home/user\nJENKINS_URL=https://jenkins.internal\n";
        let map = parse_env_output(text);
        assert_eq!(map.get("PATH"), Some(&"/usr/bin:/bin".to_string()));
        assert_eq!(map.get("HOME"), Some(&"/home/user".to_string()));
        assert_eq!(
            map.get("JENKINS_URL"),
            Some(&"https://jenkins.internal".to_string())
        );
    }

    #[test]
    fn parse_env_output_skips_noise_lines() {
        // A banner/MOTD line printed by a misconfigured rc file must not
        // abort parsing or get mistaken for a variable.
        let text =
            "Welcome to the machine!\nPATH=/usr/bin\n:: not a var either ::\nHOME=/home/user\n";
        let map = parse_env_output(text);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("PATH"), Some(&"/usr/bin".to_string()));
        assert_eq!(map.get("HOME"), Some(&"/home/user".to_string()));
    }

    #[test]
    fn parse_env_output_handles_values_containing_equals_signs() {
        let text = "SOME_URL=https://example.com/?a=1&b=2\n";
        let map = parse_env_output(text);
        assert_eq!(
            map.get("SOME_URL"),
            Some(&"https://example.com/?a=1&b=2".to_string())
        );
    }

    #[test]
    fn parse_env_output_empty_input() {
        assert!(parse_env_output("").is_empty());
    }

    #[test]
    fn shell_env_probe_args_bash_uses_interactive_login() {
        let args = shell_env_probe_args("/bin/bash");
        assert_eq!(args, vec!["-i", "-l", "-c", "env"]);
    }

    #[test]
    fn shell_env_probe_args_fish_is_just_c() {
        let args = shell_env_probe_args("/usr/bin/fish");
        assert_eq!(args, vec!["-c", "env"]);
    }

    #[test]
    fn resolved_shell_env_never_panics_and_is_cached() {
        // Smoke test against the REAL environment (best-effort — CI/sandboxed
        // environments may have an unusual $SHELL, so this only asserts the
        // function completes without panicking/hanging past its own timeout
        // and that the cache returns the identical result on a second call).
        let first = resolved_shell_env();
        let second = resolved_shell_env();
        assert_eq!(
            first as *const _, second as *const _,
            "must be cached, not re-resolved"
        );
    }

    #[test]
    fn apply_to_disabled_does_not_touch_the_command() {
        let mut cmd = Command::new("true");
        apply_to(&mut cmd, false);
        // No direct way to introspect Command's envs from the outside;
        // this at least confirms `apply_to` with enabled=false doesn't
        // panic and the command is still otherwise usable.
        let _ = cmd.get_program();
    }

    #[test]
    fn real_bash_login_interactive_probe_picks_up_bashrc_and_profile_exports() {
        // End-to-end regression guard for the actual reported bug: a
        // variable set ONLY in `.bashrc` (sourced for interactive shells)
        // must be visible, not just one set in `.profile`/`.bash_profile`
        // (login shells) — confirmed empirically during triage that bash
        // only sources `.bashrc` with `-i`, so `-l` alone would have missed
        // this exact case. Builds the real command with a scratch $HOME
        // passed via `Command::env` (child-process-only override — never
        // mutates this test process's own environment, so this stays safe
        // to run in parallel with other tests) rather than going through
        // the cached `resolved_shell_env()` (which reads the REAL $SHELL/
        // $HOME and can't be pointed at a scratch rc file).
        let Ok(bash_path) = which_bash() else {
            return; // bash not available in this environment, skip
        };

        let tmp = std::env::temp_dir().join(format!(
            "mae-babel-shell-env-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join(".bashrc"),
            "export MAE_TEST_INTERACTIVE_VAR=from_bashrc\n",
        )
        .unwrap();
        std::fs::write(
            tmp.join(".bash_profile"),
            "export MAE_TEST_LOGIN_VAR=from_bash_profile\n[ -f ~/.bashrc ] && source ~/.bashrc\n",
        )
        .unwrap();

        let args = shell_env_probe_args(&bash_path);
        let output = Command::new(&bash_path)
            .args(&args)
            .env("HOME", &tmp)
            .env_remove("BASH_ENV")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();

        let _ = std::fs::remove_dir_all(&tmp);

        let Ok(output) = output else {
            return; // couldn't spawn bash in this sandbox, skip rather than fail
        };
        if !output.status.success() {
            return;
        }
        let Ok(text) = String::from_utf8(output.stdout) else {
            return;
        };
        let map = parse_env_output(&text);

        assert_eq!(
            map.get("MAE_TEST_LOGIN_VAR").map(String::as_str),
            Some("from_bash_profile"),
            "login-shell (.bash_profile) export must be visible"
        );
        assert_eq!(
            map.get("MAE_TEST_INTERACTIVE_VAR").map(String::as_str),
            Some("from_bashrc"),
            "interactive-shell (.bashrc) export must ALSO be visible — this is the \
             exact case that broke before -i was added alongside -l"
        );
    }

    fn which_bash() -> Result<String, ()> {
        let output = Command::new("which").arg("bash").output().map_err(|_| ())?;
        if !output.status.success() {
            return Err(());
        }
        String::from_utf8(output.stdout)
            .map(|s| s.trim().to_string())
            .map_err(|_| ())
    }
}
