//! Running one build or test command: template expansion, the queue prefix,
//! output to a log file, and interruption.

use crate::journal::signals;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Expand a runner template: an argument that is exactly `{targets}` becomes
/// the targets (zero or more arguments); `{filter}` is substituted anywhere.
pub fn expand(template: &[String], targets: &[String], filter: &str) -> Vec<String> {
    let mut out = Vec::new();
    for arg in template {
        if arg == "{targets}" {
            out.extend(targets.iter().cloned());
        } else {
            out.push(arg.replace("{filter}", filter));
        }
    }
    out
}

/// The command prefix every build and test runs under.
///
/// - `MUTATE_PREFIX` set: split on whitespace, `{class}` and `{area}` filled in.
/// - `MUTATE_QUEUE=off`: none (CI, and the tool's own tests).
/// - otherwise `trabajo <class> mae:mutants:<area> --` when `trabajo` is on
///   PATH, so builds share the machine's job queue; else none.
pub fn prefix(class: &str, area: &str) -> Vec<String> {
    let fill = |s: &str| s.replace("{class}", class).replace("{area}", area);
    match queue(&env, on_path("trabajo")) {
        Queue::Explicit(p) => p.split_whitespace().map(fill).collect(),
        Queue::None => Vec::new(),
        Queue::Trabajo => ["trabajo", "{class}", "mae:mutants:{area}", "--"]
            .iter()
            .map(|s| fill(s))
            .collect(),
    }
}

/// Which prefix applies, from the environment (`env`) and whether `trabajo`
/// is on PATH.
#[derive(Debug, PartialEq, Eq)]
pub enum Queue {
    Explicit(String),
    None,
    Trabajo,
}

pub fn queue(env: &dyn Fn(&str) -> Option<String>, trabajo_on_path: bool) -> Queue {
    if let Some(p) = env("MUTATE_PREFIX") {
        return Queue::Explicit(p);
    }
    if env("MUTATE_QUEUE").as_deref() == Some("off") || !trabajo_on_path {
        return Queue::None;
    }
    Queue::Trabajo
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Refuse to queue from inside a Slurm job. `trabajo` asks Slurm for a job
/// step, and inside a job (`SLURM_JOB_ID` set) that step is placed in the
/// enclosing job, whose CPUs and memory the tool itself already holds: it
/// waits there until the job's time limit kills everything, with no output.
/// Only the default prefix is refused; an explicit `MUTATE_PREFIX` is the
/// caller's choice.
pub fn check_not_nested(
    env: &dyn Fn(&str) -> Option<String>,
    trabajo_on_path: bool,
) -> Result<(), String> {
    match (queue(env, trabajo_on_path), env("SLURM_JOB_ID")) {
        (Queue::Trabajo, Some(job)) => Err(format!(
            "running inside Slurm job {job}, so each queued step would wait for room in this \
             same job until its time limit. Run mae-mutate outside the queue (it queues its own \
             steps), or set MUTATE_QUEUE=off to run every step in this job."
        )),
        _ => Ok(()),
    }
}

/// [`check_not_nested`] against this process's environment.
pub fn check_queue() -> Result<(), String> {
    check_not_nested(&env, on_path("trabajo"))
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

pub struct Done {
    pub success: bool,
    pub secs: f64,
    pub output: String,
    pub log: PathBuf,
}

pub enum Stop {
    Interrupted,
    Error(String),
}

impl From<String> for Stop {
    fn from(e: String) -> Self {
        Stop::Error(e)
    }
}

/// Run `argv` in `cwd` with stdout and stderr into `log`, polling for exit so
/// a SIGINT/SIGTERM to the tool stops the child and unwinds to the restore.
/// `MUTATE_ID` tells the command which mutant (or `control`) it serves.
pub fn run(cwd: &Path, argv: &[String], id: &str, log: &Path) -> Result<Done, Stop> {
    let err = |e: std::io::Error| Stop::Error(format!("{}: {e}", log.display()));
    let file = fs::File::create(log).map_err(err)?;
    let start = Instant::now();
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .env("MUTATE_ID", id)
        .stdin(Stdio::null())
        .stdout(file.try_clone().map_err(err)?)
        .stderr(file)
        .spawn()
        .map_err(|e| Stop::Error(format!("{}: {e}", argv[0])))?;
    let status = loop {
        if signals::interrupted() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Stop::Interrupted);
        }
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(Stop::Error(format!("{}: {e}", argv[0]))),
        }
    };
    if signals::interrupted() {
        return Err(Stop::Interrupted);
    }
    // Markers are matched against the text, not the terminal styling: under
    // `CARGO_TERM_COLOR=always` (which CI sets) cargo writes
    // "\x1b[1m\x1b[92m   Compiling\x1b[0m fixture", which holds no
    // "Compiling ", so every mutant read as "recompiled nothing". The log
    // keeps the bytes as written.
    let output = strip_ansi(&String::from_utf8_lossy(&fs::read(log).unwrap_or_default()));
    Ok(Done {
        success: status.success(),
        secs: start.elapsed().as_secs_f64(),
        output,
        log: log.to_path_buf(),
    })
}

/// The text of a command's output with ANSI escape sequences removed: CSI
/// (`ESC [ ... final byte`: colours and styles), OSC (`ESC ] ... BEL` or
/// `ESC ] ... ESC \`: hyperlinks, titles), and any other two-character
/// `ESC x`.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Sum of every `{n}` matched by the patterns, e.g. `"{n} tests run"` or
/// `"running {n} test"` (which matches "running 3 tests" too).
pub fn count_tests(patterns: &[String], output: &str) -> u64 {
    patterns.iter().map(|p| count_one(p, output)).sum()
}

fn count_one(pattern: &str, output: &str) -> u64 {
    let Some((pre, post)) = pattern.split_once("{n}") else {
        return 0;
    };
    let mut total = 0;
    if pre.is_empty() {
        for (i, _) in output.match_indices(post) {
            let head = &output.as_bytes()[..i];
            let digits = head.iter().rev().take_while(|b| b.is_ascii_digit()).count();
            total += parse(&output[i - digits..i]);
        }
        return total;
    }
    for (i, _) in output.match_indices(pre) {
        let rest = &output[i + pre.len()..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && rest[digits..].starts_with(post) {
            total += parse(&rest[..digits]);
        }
    }
    total
}

fn parse(s: &str) -> u64 {
    s.parse().unwrap_or(0)
}

/// Last lines of a command's output, for a verdict that needs explaining.
pub fn tail(output: &str, n: usize) -> String {
    let lines: Vec<&str> = output.trim_end().lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    /// The deadlock this guards against cost two runs of the full time limit
    /// each: inside a Slurm job, the default `trabajo` prefix is refused.
    #[test]
    fn queueing_from_inside_a_slurm_job_is_refused() {
        let nested = env_of(&[("SLURM_JOB_ID", "7414")]);
        let err = check_not_nested(&nested, true).unwrap_err();
        assert!(
            err.contains("Slurm job 7414") && err.contains("MUTATE_QUEUE=off"),
            "{err}"
        );
        // Not nested, the queue is used as before.
        assert_eq!(check_not_nested(&env_of(&[]), true), Ok(()));
        // Inside a job but not queueing: nothing to wait for.
        let off = env_of(&[("SLURM_JOB_ID", "7414"), ("MUTATE_QUEUE", "off")]);
        assert_eq!(check_not_nested(&off, true), Ok(()));
        assert_eq!(
            check_not_nested(&nested, false),
            Ok(()),
            "no trabajo on PATH"
        );
        // An explicit prefix is the caller's choice.
        let explicit = env_of(&[("SLURM_JOB_ID", "7414"), ("MUTATE_PREFIX", "env X=1")]);
        assert_eq!(check_not_nested(&explicit, true), Ok(()));
    }

    #[test]
    fn the_prefix_follows_the_environment() {
        let q = |v: &[(&str, &str)], on| queue(&env_of(v), on);
        assert_eq!(q(&[], true), Queue::Trabajo);
        assert_eq!(q(&[], false), Queue::None);
        assert_eq!(q(&[("MUTATE_QUEUE", "off")], true), Queue::None);
        assert_eq!(
            q(&[("MUTATE_PREFIX", "a b"), ("MUTATE_QUEUE", "off")], true),
            Queue::Explicit("a b".into())
        );
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn counts_both_nextest_forms_and_sums_cargo_binaries() {
        let nextest = s(&["{n} tests run", "{n} test run"]);
        assert_eq!(
            count_tests(&nextest, "Summary [ 1s] 12 tests run: 12 passed"),
            12
        );
        assert_eq!(
            count_tests(&nextest, "Summary [ 1s] 1 test run: 1 passed"),
            1
        );
        assert_eq!(count_tests(&nextest, "error: no tests to run"), 0);
        let cargo = s(&["running {n} test"]);
        let out = "running 3 tests\n...\nrunning 0 tests\nrunning 1 test\n";
        assert_eq!(count_tests(&cargo, out), 4);
    }

    #[test]
    fn styling_is_stripped_so_markers_match_coloured_output() {
        // As cargo and nextest write them under CARGO_TERM_COLOR=always.
        let cargo = "\x1b[1m\x1b[92m   Compiling\x1b[0m fixture v0.1.0 (/x)\n";
        assert!(!cargo.contains("Compiling "), "no plain marker as written");
        assert_eq!(strip_ansi(cargo), "   Compiling fixture v0.1.0 (/x)\n");
        let nextest = "\x1b[32;1m     Summary\x1b[0m [ 0.01s] \x1b[1m12\x1b[0m tests run";
        let n = s(&["{n} tests run"]);
        assert_eq!(count_tests(&n, nextest), 0, "a styled count is not read");
        assert_eq!(count_tests(&n, &strip_ansi(nextest)), 12);
        let links = "\x1b]8;;file:///x\x1b\\lib.rs\x1b]8;;\x1b\\ and \x1b]8;;f\x07a\x1b]8;;\x07";
        assert_eq!(strip_ansi(links), "lib.rs and a");
        let plain = "é running 3 tests [ok] ~\n";
        assert_eq!(
            strip_ansi(plain),
            plain,
            "text without escapes is unchanged"
        );
    }

    #[test]
    fn targets_expand_to_zero_or_more_args_and_filter_substitutes() {
        let t = s(&["cargo", "nextest", "run", "{targets}", "-E", "{filter}"]);
        assert_eq!(
            expand(&t, &s(&["-p", "x", "--lib"]), "test(a)"),
            s(&["cargo", "nextest", "run", "-p", "x", "--lib", "-E", "test(a)"])
        );
        assert_eq!(
            expand(&t, &[], "f"),
            s(&["cargo", "nextest", "run", "-E", "f"])
        );
    }
}
