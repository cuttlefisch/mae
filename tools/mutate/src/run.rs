//! A mutation run: control runs first, then each mutant built and tested apart,
//! each verdict streamed and persisted, then the summary.

use crate::config::{self, Lists, Mutant, Runner};
use crate::exec::{self, Done, Stop};
use crate::git;
use crate::journal;
use crate::results::{self, Record, Results};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub struct Run<'a> {
    pub root: &'a Path,
    pub state: PathBuf,
    pub lists: &'a Lists,
    tree: String,
    results: Results,
}

/// Exit codes: 0 every mutant caught; 1 a survivor, unviable, no-result or
/// unrun mutant; 2 refused before anything ran (dirty file, failed control).
pub fn run(
    root: &Path,
    state: PathBuf,
    lists: &Lists,
    selected: &[&Mutant],
    only: &[String],
    fresh: bool,
) -> Result<u8, Stop> {
    let (equivalent, runnable): (Vec<&Mutant>, Vec<&Mutant>) =
        selected.iter().partition(|m| m.is_equivalent());
    let touched: BTreeSet<String> = runnable.iter().flat_map(|m| m.files()).collect();
    let dirty = git::dirty(root, &touched)?;
    if !dirty.is_empty() {
        eprintln!("refusing: files a mutant edits have uncommitted changes: {dirty:?}");
        return Ok(2);
    }
    let mut r = Run {
        root,
        tree: git::tree_state(root)?,
        results: Results::open(&state, fresh)?,
        state,
        lists,
    };
    let todo: Vec<&Mutant> = runnable
        .iter()
        .copied()
        .filter(|m| only.contains(&m.id) || r.results.current(&m.id, &r.fingerprint(m)).is_none())
        .collect();
    println!(
        "{} to run, {} already recorded on this code",
        todo.len(),
        runnable.len() - todo.len()
    );
    if !r.controls(&todo)? {
        return Ok(2);
    }
    for (i, m) in todo.iter().enumerate() {
        r.one(m, i + 1, todo.len())?;
    }
    Ok(r.summary(&runnable, &equivalent))
}

impl Run<'_> {
    fn runner(&self, m: &Mutant) -> &Runner {
        &self.lists.runners[&m.runner]
    }

    fn fingerprint(&self, m: &Mutant) -> String {
        let runner = format!("{:?}", self.runner(m));
        format!(
            "{:016x}",
            git::fnv(&[&self.tree, &format!("{m:?}"), &runner])
        )
    }

    fn command(&self, m: &Mutant, template: &[String]) -> Vec<String> {
        let runner = self.runner(m);
        let mut argv = exec::prefix(&runner.queue_class, &m.area);
        argv.extend(exec::expand(template, m.targets(runner), &m.filter));
        argv
    }

    fn exec(&self, m: &Mutant, template: &[String], id: &str, phase: &str) -> Result<Done, Stop> {
        let logs = self.state.join("logs");
        std::fs::create_dir_all(&logs).map_err(|e| format!("{}: {e}", logs.display()))?;
        let safe: String = id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let log = logs.join(format!("{safe}.{phase}.log"));
        let cwd = self.root.join(&self.runner(m).workspace);
        exec::run(&cwd, &self.command(m, template), id, &log)
    }

    /// Every distinct (runner, targets, filter) must pass on the healthy code
    /// and run at least one test. Otherwise no verdict below would mean
    /// anything: a filter that selects nothing "survives" every mutant, and an
    /// already-failing test "catches" every one.
    fn controls(&mut self, todo: &[&Mutant]) -> Result<bool, Stop> {
        let mut seen = BTreeSet::new();
        for m in todo {
            let runner = self.runner(m);
            let key = format!(
                "{}\0{}\0{}",
                m.runner,
                m.targets(runner).join("\0"),
                m.filter
            );
            if !seen.insert(key.clone()) {
                continue;
            }
            let id = format!("control:{}:{:016x}", m.runner, git::fnv(&[&key]));
            let fp = format!(
                "{:016x}",
                git::fnv(&[&self.tree, &key, &format!("{runner:?}")])
            );
            if self.results.current(&id, &fp).is_some() {
                continue;
            }
            let build = self.exec(m, &runner.build, "control", "control-build")?;
            let test = if build.success {
                Some(self.exec(m, &runner.test, "control", "control-test")?)
            } else {
                None
            };
            let ran = test
                .as_ref()
                .map_or(0, |t| exec::count_tests(&runner.count, &t.output));
            let last = test.as_ref().unwrap_or(&build);
            if !last.success || ran == 0 {
                println!(
                    "CONTROL FAILED: runner {} targets {:?} filter {:?}: {}, {ran} tests run. \
                     Fix the filter or the tests first.\n{}\nlog: {}",
                    m.runner,
                    m.targets(runner),
                    m.filter,
                    if last.success { "passed" } else { "failed" },
                    exec::tail(&last.output, 15),
                    last.log.display()
                );
                return Ok(false);
            }
            println!(
                "control ok: {ran} tests pass for {} {:?}",
                m.runner, m.filter
            );
            let secs = (build.secs, last.secs);
            self.record(results::CONTROL_OK, &id, secs, fp)?;
        }
        Ok(true)
    }

    fn record(
        &mut self,
        verdict: &str,
        id: &str,
        secs: (f64, f64),
        fp: String,
    ) -> Result<(), Stop> {
        Ok(self.results.append(Record {
            verdict: verdict.to_string(),
            id: id.to_string(),
            build: secs.0,
            test: secs.1,
            fingerprint: fp,
        })?)
    }

    fn one(&mut self, m: &Mutant, i: usize, n: usize) -> Result<(), Stop> {
        let files = config::apply(self.root, m)?;
        journal::record(&self.state, self.root, &files)?;
        let mut guard = Restore {
            state: &self.state,
            armed: true,
        };
        for (path, _, mutated) in &files {
            journal::write_file(&self.root.join(path), mutated)?;
        }
        let judged = self.judge(m, &files);
        guard.finish()?;
        drop(guard);
        let (verdict, build, test, done) = judged?;
        println!(
            "{verdict}\t{}\tbuild {build:.0}s\ttest {test:.0}s\t[{i}/{n}]",
            m.id
        );
        if verdict != results::CAUGHT && verdict != results::SURVIVED {
            if let Some(d) = done {
                println!("{}\nlog: {}", exec::tail(&d.output, 15), d.log.display());
            }
        }
        let fp = self.fingerprint(m);
        self.record(verdict, &m.id, (build, test), fp)
    }

    #[allow(clippy::type_complexity)]
    fn judge(
        &self,
        m: &Mutant,
        files: &[(String, String, String)],
    ) -> Result<(&'static str, f64, f64, Option<Done>), Stop> {
        let (built, build_s) = self.build_step(m, files)?;
        match built {
            Err(verdict) => Ok((verdict.0, build_s, 0.0, verdict.1)),
            Ok(()) => {
                let (verdict, test_s, done) = self.test_step(m)?;
                Ok((verdict, build_s, test_s, done))
            }
        }
    }

    /// Build the test binaries with the mutant in place. A build that did not
    /// recompile anything ran nothing new: its binary is from an earlier
    /// mutant or the healthy code, so the edited files' mtimes are bumped and
    /// it is rebuilt once; it is never judged as is.
    #[allow(clippy::type_complexity)]
    fn build_step(
        &self,
        m: &Mutant,
        files: &[(String, String, String)],
    ) -> Result<(Result<(), (&'static str, Option<Done>)>, f64), Stop> {
        let runner = self.runner(m);
        let (mut secs, mut kills, mut bumped) = (0.0, 0, false);
        loop {
            let d = self.exec(m, &runner.build, &m.id, "build")?;
            secs += d.secs;
            if d.success {
                if runner.rebuilt.is_empty() || d.output.contains(&runner.rebuilt) {
                    return Ok((Ok(()), secs));
                }
                if bumped {
                    println!("{}: the build recompiled nothing, twice", m.id);
                    return Ok((Err((results::NO_RESULT, Some(d))), secs));
                }
                for (path, _, _) in files {
                    journal::bump_mtime(&self.root.join(path))?;
                }
                bumped = true;
                continue;
            }
            if runner
                .unviable
                .iter()
                .any(|u| d.output.contains(u.as_str()))
            {
                return Ok((Err((results::UNVIABLE, Some(d))), secs));
            }
            if kills >= runner.retry_on_kill {
                return Ok((Err((results::NO_RESULT, Some(d))), secs));
            }
            kills += 1;
        }
    }

    /// Run the filtered tests. Exit 0 is a survivor; a caught marker is a
    /// catch; anything else (killed, timed out with no marker) is retried and
    /// then reported as no result, never counted either way.
    fn test_step(&self, m: &Mutant) -> Result<(&'static str, f64, Option<Done>), Stop> {
        let runner = self.runner(m);
        let (mut secs, mut kills) = (0.0, 0);
        loop {
            let d = self.exec(m, &runner.test, &m.id, "test")?;
            secs += d.secs;
            if d.success {
                return Ok((results::SURVIVED, secs, None));
            }
            if runner.caught.iter().any(|c| d.output.contains(c.as_str())) {
                return Ok((results::CAUGHT, secs, None));
            }
            if runner
                .unviable
                .iter()
                .any(|u| d.output.contains(u.as_str()))
            {
                return Ok((results::UNVIABLE, secs, Some(d)));
            }
            if kills >= runner.retry_on_kill {
                return Ok((results::NO_RESULT, secs, Some(d)));
            }
            kills += 1;
        }
    }

    fn summary(&self, runnable: &[&Mutant], equivalent: &[&Mutant]) -> u8 {
        let rows: Vec<(&Mutant, Option<&Record>)> = runnable
            .iter()
            .map(|m| (*m, self.results.current(&m.id, &self.fingerprint(m))))
            .collect();
        let with = |v: &str| -> Vec<&str> {
            rows.iter()
                .filter(|(_, r)| r.map(|r| r.verdict.as_str()) == Some(v))
                .map(|(m, _)| m.id.as_str())
                .collect()
        };
        let not_run: Vec<&str> = rows
            .iter()
            .filter(|(_, r)| r.is_none())
            .map(|(m, _)| m.id.as_str())
            .collect();
        let (survivors, unviable, none) = (
            with(results::SURVIVED),
            with(results::UNVIABLE),
            with(results::NO_RESULT),
        );
        println!(
            "{} of {} caught; survivors: [{}]",
            with(results::CAUGHT).len(),
            rows.len(),
            survivors.join(", ")
        );
        for (label, ids) in [
            ("unviable", &unviable),
            ("no result", &none),
            ("not run", &not_run),
        ] {
            if !ids.is_empty() {
                println!("{label}: [{}]", ids.join(", "));
            }
        }
        for m in equivalent {
            println!(
                "equivalent, not run: {} ({})",
                m.id,
                m.equivalent.as_deref().unwrap_or("")
            );
        }
        print_times(&rows);
        let clean = survivors.is_empty() && unviable.is_empty() && none.is_empty();
        u8::from(!(clean && not_run.is_empty()))
    }
}

fn print_times(rows: &[(&Mutant, Option<&Record>)]) {
    let recs: Vec<&Record> = rows.iter().filter_map(|(_, r)| *r).collect();
    let build: f64 = recs.iter().map(|r| r.build).sum();
    let test: f64 = recs.iter().map(|r| r.test).sum();
    println!(
        "time: build {:.1} min, test {:.1} min over {} mutants",
        build / 60.0,
        test / 60.0,
        recs.len()
    );
    let mut slow = recs.clone();
    slow.sort_by(|a, b| (b.build + b.test).total_cmp(&(a.build + a.test)));
    let slowest: Vec<String> = slow
        .iter()
        .take(5)
        .map(|r| format!("{} {:.0}s", r.id, r.build + r.test))
        .collect();
    if !slowest.is_empty() {
        println!("slowest: {}", slowest.join("; "));
    }
}

/// Restores the journalled originals when a mutant is done, however it ends:
/// `finish` on the normal path, `Drop` on an error or a panic.
struct Restore<'a> {
    state: &'a Path,
    armed: bool,
}

impl Restore<'_> {
    fn finish(&mut self) -> Result<(), String> {
        self.armed = false;
        journal::restore(self.state).map(|_| ())
    }
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        if self.armed {
            if let Err(e) = journal::restore(self.state) {
                eprintln!("RESTORE FAILED, the journal is kept for the next start: {e}");
            }
        }
    }
}
