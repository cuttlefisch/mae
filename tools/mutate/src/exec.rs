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
    if let Ok(p) = std::env::var("MUTATE_PREFIX") {
        return p.split_whitespace().map(fill).collect();
    }
    if std::env::var("MUTATE_QUEUE").as_deref() == Ok("off") || !on_path("trabajo") {
        return Vec::new();
    }
    ["trabajo", "{class}", "mae:mutants:{area}", "--"]
        .iter()
        .map(|s| fill(s))
        .collect()
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
    let output = String::from_utf8_lossy(&fs::read(log).unwrap_or_default()).into_owned();
    Ok(Done {
        success: status.success(),
        secs: start.elapsed().as_secs_f64(),
        output,
        log: log.to_path_buf(),
    })
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
