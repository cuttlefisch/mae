//! MAE Mutate: declared, re-runnable mutation checks.
//!
//! Every test that guards a mechanism must be shown to fail without that
//! mechanism. The mutants that show it are declared in `mutants/<area>.toml`
//! (an id, a runner, the filter naming the tests that must fail, and exact
//! find/replace edits), with the runners defined once in
//! `mutants/runners.toml`. See `tools/mutate/README.md` for the format.
//!
//! Usage (from anywhere in the repository):
//!   mae-mutate --check                      every anchor matches exactly once
//!   mae-mutate list [selection]             the selected ids
//!   mae-mutate run  [selection] [--fresh]   control runs, then each mutant
//! selection: --area A, --only ID[,ID], --since REF, --shard i/N (repeatable
//! where it makes sense; --root DIR overrides the repository root).

mod config;
mod exec;
mod git;
mod journal;
mod results;
mod run;
mod select;

use exec::Stop;
use select::Selection;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(PartialEq)]
enum Cmd {
    Check,
    List,
    Run,
}

struct Args {
    cmd: Cmd,
    root: Option<PathBuf>,
    sel: Selection,
    fresh: bool,
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut a = Args {
        cmd: Cmd::Check,
        root: None,
        sel: Selection::default(),
        fresh: false,
    };
    let mut cmd = None;
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--check" | "check" => cmd = Some(Cmd::Check),
            "list" => cmd = Some(Cmd::List),
            "run" => cmd = Some(Cmd::Run),
            "--root" => a.root = Some(PathBuf::from(value()?)),
            "--area" => a.sel.areas.push(value()?),
            "--only" => a.sel.only.extend(value()?.split(',').map(str::to_string)),
            "--since" => a.sel.since = Some(value()?),
            "--shard" => a.sel.shard = Some(select::parse_shard(&value()?)?),
            "--fresh" => a.fresh = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    a.cmd = cmd.ok_or("expected --check, list or run")?;
    Ok(a)
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let code = match parse_args(&argv) {
        Ok(args) => match real_main(args) {
            Ok(c) => c,
            Err(Stop::Interrupted) => {
                eprintln!("interrupted; files restored");
                130
            }
            Err(Stop::Error(e)) => {
                eprintln!("mae-mutate: {e}");
                2
            }
        },
        Err(e) => {
            eprintln!("mae-mutate: {e}\nusage: mae-mutate [--root DIR] (--check | list | run) [--area A] [--only ID,..] [--since REF] [--shard i/N] [--fresh]");
            2
        }
    };
    ExitCode::from(code)
}

fn real_main(args: Args) -> Result<u8, Stop> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let root = match args.root {
        Some(r) => git::toplevel(&r)?,
        None => git::toplevel(&cwd)?,
    };
    let state = git::state_dir(&root)?;
    std::fs::create_dir_all(&state).map_err(|e| format!("{}: {e}", state.display()))?;
    let _lock = journal::Lock::acquire(&state)?;
    journal::signals::install();
    // First, before any anchor is read: a run killed mid-mutant left its
    // mutant on disk, and the journal knows the original.
    for f in journal::restore(&state)? {
        println!("restored {f} from the journal of an interrupted run");
    }
    let lists = config::load(&root)?;
    let problems = config::check_structure(&root, &lists);
    if !problems.is_empty() {
        return Err(Stop::Error(problems.join("\n")));
    }
    let selected = select::select(&root, &lists.mutants, &args.sel)?;
    let anchors = config::check_anchors(&root, &selected);
    if !anchors.is_empty() {
        return Err(Stop::Error(anchors.join("\n")));
    }
    match args.cmd {
        Cmd::Check => {
            println!(
                "{} mutants: ids unique, runners known, anchors match once",
                selected.len()
            );
            Ok(0)
        }
        Cmd::List => {
            for m in &selected {
                let eq = if m.is_equivalent() {
                    "\tequivalent"
                } else {
                    ""
                };
                let reference = m.reference.as_deref().unwrap_or("-");
                println!("{}\t{}\t{reference}{eq}", m.id, m.runner);
            }
            Ok(0)
        }
        Cmd::Run => run::run(&root, state, &lists, &selected, &args.sel.only, args.fresh),
    }
}
