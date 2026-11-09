//! `mae-daemon kb seed`: plan the seed of a native KB from a tagged commit.
//!
//! Only `--dry-run` exists in this version. It reads git, writes nothing, and
//! prints what a seed would create and every reason it must not (see
//! `mae_daemon::seed`). Without `--dry-run` the command refuses before reading
//! anything: applying a plan is not built yet.
//!
//! @ai-caution: [security] An operator command, like `token mint`: seeding names
//! a KB's owner. It must not become an MCP tool or an agent-reachable RPC
//! (CLAUDE.md principle #16).

use std::path::PathBuf;

use mae_daemon::seed::{self, PlanOptions};

const USAGE: &str = "usage: mae-daemon kb seed --dry-run --git <repo> --rev <tag-or-commit>\n\
     \x20   [--include <glob>]... [--exclude <path>]... [--require-canonical]\n\
     \x20   [--max-body-bytes <n>] [--code-property <KEY>] [--json]\n\
     \x20   [--name <name>] [--owner <principal>] [--owner-label <label>] [--kb-id <id>]\n\
     \x20 --include: repo-relative glob, repeatable; `**` crosses directories, `*` does not\n\
     \x20            (default: **/*.org)\n\
     \x20 --exclude: an exact selected path to leave out, repeatable; always reported";

const APPLY_NOT_BUILT: &str = "error: apply not built yet: only `mae-daemon kb seed --dry-run` \
     exists in this version. Nothing was read or written.";

/// `mae-daemon kb <action> …`. Exit code: 0 plan OK, 1 refused or failed, 2 usage.
pub(crate) fn run_kb(rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("seed") => run_seed(&rest[1..]),
        _ => {
            eprintln!("{USAGE}");
            2
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct SeedArgs {
    dry_run: bool,
    git: Option<PathBuf>,
    rev: Option<String>,
    include: Vec<String>,
    exclude: Vec<String>,
    max_body_bytes: Option<usize>,
    require_canonical: bool,
    json: bool,
    name: Option<String>,
    owner: Option<String>,
    owner_label: Option<String>,
    kb_id: Option<String>,
    code_property: Option<String>,
}

fn run_seed(args: &[String]) -> i32 {
    // Parsing reads nothing. `--dry-run` is a flag of the parse, not a word
    // searched for in the raw arguments: `--name --dry-run` names a KB
    // "--dry-run" (refused as a value) rather than passing the guard.
    let parsed = match parse_seed_args(args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            return 2;
        }
    };
    if !parsed.dry_run {
        eprintln!("{APPLY_NOT_BUILT}");
        return 2;
    }
    let opts = PlanOptions {
        repo: parsed.git.clone().unwrap_or_default(),
        rev: parsed.rev.clone().unwrap_or_default(),
        include: parsed.include.clone(),
        exclude: parsed.exclude.clone(),
        max_body_bytes: parsed
            .max_body_bytes
            .unwrap_or(seed::DEFAULT_MAX_BODY_BYTES),
        require_canonical: parsed.require_canonical,
        code_property: parsed.code_property.clone(),
    };
    match seed::plan(&opts) {
        Ok(plan) => {
            if parsed.json {
                let report = seed::render_json(&plan, &opts.exclude);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).unwrap_or_default()
                );
            } else {
                print!("{}", seed::render_text(&plan));
            }
            if plan.ok() {
                0
            } else {
                1
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

/// `--flag value`, `--flag=value`, and the three switches. Unknown flags are
/// errors: a typo must not plan something other than what was asked.
fn parse_seed_args(args: &[String]) -> Result<SeedArgs, String> {
    let mut a = SeedArgs::default();
    let mut i = 0;
    while i < args.len() {
        let (flag, inline) = match args[i].split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (args[i].as_str(), None),
        };
        i += 1;
        if let Some(switch) = switch(&mut a, flag) {
            if inline.is_some() {
                return Err(format!("{flag} takes no value"));
            }
            *switch = true;
            continue;
        }
        let value = match inline {
            Some(v) => v,
            None => {
                i += 1;
                args.get(i - 1)
                    .filter(|v| !v.starts_with("--"))
                    .cloned()
                    .ok_or_else(|| format!("{flag} needs a value"))?
            }
        };
        set_value(&mut a, flag, value)?;
    }
    validate(a)
}

/// The boolean flags.
fn switch<'a>(a: &'a mut SeedArgs, flag: &str) -> Option<&'a mut bool> {
    match flag {
        "--dry-run" => Some(&mut a.dry_run),
        "--require-canonical" => Some(&mut a.require_canonical),
        "--json" => Some(&mut a.json),
        _ => None,
    }
}

fn set_value(a: &mut SeedArgs, flag: &str, value: String) -> Result<(), String> {
    match flag {
        "--git" => a.git = Some(PathBuf::from(value)),
        "--rev" => a.rev = Some(value),
        "--include" => a.include.push(value),
        "--exclude" => a.exclude.push(value),
        "--max-body-bytes" => {
            let n: usize = value
                .parse()
                .map_err(|_| format!("--max-body-bytes {value:?} is not a number"))?;
            a.max_body_bytes = Some(n);
        }
        "--name" => a.name = Some(value),
        "--owner" => a.owner = Some(value),
        "--owner-label" => a.owner_label = Some(value),
        "--kb-id" => a.kb_id = Some(value),
        "--code-property" => a.code_property = Some(value),
        other => return Err(format!("unknown argument '{other}'")),
    }
    Ok(())
}

fn validate(a: SeedArgs) -> Result<SeedArgs, String> {
    if a.git.is_none() {
        return Err("--git <repo> is required".into());
    }
    if a.rev.as_deref().is_none_or(str::is_empty) {
        return Err("--rev <tag-or-commit> is required".into());
    }
    if a.max_body_bytes == Some(0) {
        return Err("--max-body-bytes must be greater than zero".into());
    }
    if let Some(id) = &a.kb_id {
        if !mae_sync::kb_id_is_addressable(id) {
            return Err(format!(
                "--kb-id {id:?} is not addressable (it must not contain ':')"
            ));
        }
    }
    if let Some(k) = &a.code_property {
        if k.is_empty() || k.contains(':') || k.chars().any(char::is_whitespace) {
            return Err(format!("--code-property {k:?} is not a property key"));
        }
    }
    let mut seen = std::collections::HashSet::new();
    if let Some(dup) = a.exclude.iter().find(|x| !seen.insert(*x)) {
        return Err(format!("--exclude {dup:?} is given twice"));
    }
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// Without `--dry-run` the command refuses before resolving anything: the
    /// repository named here does not exist, and the refusal is still the
    /// apply message (exit 2), not a git error (exit 1).
    #[test]
    fn without_dry_run_it_refuses_before_reading() {
        let code = run_seed(&args(&["--git", "/nonexistent/repo", "--rev", "v1"]));
        assert_eq!(code, 2);
        let code = run_seed(&args(&["--git=/nonexistent/repo", "--rev=v1", "--json"]));
        assert_eq!(code, 2);
    }

    #[test]
    fn a_missing_repo_with_dry_run_is_an_error_not_a_plan() {
        let code = run_seed(&args(&[
            "--dry-run",
            "--git",
            "/nonexistent/repo",
            "--rev",
            "v1",
        ]));
        assert_eq!(code, 1);
    }

    #[test]
    fn repeatable_flags_and_both_value_forms_parse() {
        let a = parse_seed_args(&args(&[
            "--dry-run",
            "--git=/r",
            "--rev",
            "v1",
            "--include",
            "notes/**/*.org",
            "--include=extra/*.org",
            "--exclude",
            "notes/a.org",
            "--exclude=notes/b.org",
            "--json",
        ]))
        .unwrap();
        assert_eq!(a.include, vec!["notes/**/*.org", "extra/*.org"]);
        assert_eq!(a.exclude, vec!["notes/a.org", "notes/b.org"]);
        assert!(a.json);
    }

    /// `--dry-run` must be parsed as the flag it is. Kills: searching the raw
    /// arguments for the word, which `--name --dry-run` passed.
    #[test]
    fn dry_run_is_a_flag_not_a_word_in_the_arguments() {
        let code = run_seed(&args(&[
            "--git",
            "/nonexistent/repo",
            "--rev",
            "v1",
            "--name",
            "--dry-run",
        ]));
        assert_eq!(code, 2);
        assert!(parse_seed_args(&args(&[
            "--git",
            "/r",
            "--rev",
            "v1",
            "--name",
            "--dry-run"
        ]))
        .is_err());
        assert!(parse_seed_args(&args(&["--dry-run=yes", "--git", "/r", "--rev", "v1"])).is_err());
        let a = parse_seed_args(&args(&["--git", "/r", "--rev", "v1", "--name", "kb"])).unwrap();
        assert!(!a.dry_run);
    }

    #[test]
    fn unknown_flags_duplicates_and_bad_values_are_usage_errors() {
        for bad in [
            &["--dry-run", "--git", "/r", "--rev", "v1", "--skip", "x"][..],
            &[
                "--dry-run",
                "--git",
                "/r",
                "--rev",
                "v1",
                "--exclude",
                "a",
                "--exclude",
                "a",
            ],
            &["--dry-run", "--git", "/r", "--rev", "v1", "--kb-id", "a:b"],
            &[
                "--dry-run",
                "--git",
                "/r",
                "--rev",
                "v1",
                "--max-body-bytes",
                "0",
            ],
            &["--dry-run", "--rev", "v1"],
            &["--dry-run", "--git", "/r", "--rev"],
        ] {
            assert!(
                parse_seed_args(&args(bad)).is_err(),
                "{bad:?} must be refused"
            );
        }
    }
}
