//! Real runs against the fixture crate: verdicts, the control, resume, and
//! the restore after a kill or a signal.

mod common;
use common::*;
use std::time::{Duration, Instant};

fn count(calls: &[String], line: &str) -> usize {
    calls.iter().filter(|c| c.as_str() == line).count()
}

#[test]
fn same_length_mutants_back_to_back_are_each_rebuilt_and_judged() {
    // `a` and `b` edit the same file to the same length, one right after the
    // other. Had cargo kept `a`'s binary for `b`, `b` would read as caught.
    let fx = Fx::committed("back-to-back", &(caught_a() + &survives_b()));
    let out = fx.mutate(&["run"]);
    assert_eq!(
        out.code(),
        Some(1),
        "a survivor fails the run: {}",
        out.all()
    );
    assert!(out.stdout().contains("caught\ta\t"), "{}", out.all());
    assert!(out.stdout().contains("SURVIVED\tb\t"), "{}", out.all());
    assert!(
        out.stdout().contains("1 of 2 caught; survivors: [b]"),
        "{}",
        out.all()
    );
    assert_eq!(fx.read("src/lib.rs"), FIXTURE_LIB, "restored after the run");
}

#[test]
fn a_build_that_recompiled_nothing_is_rebuilt_not_judged() {
    // The hook makes `b`'s first build see a source file older than the last
    // build, as a coarse or skewed clock would: cargo keeps `a`'s binary.
    let fx = Fx::new("stale-build", &(caught_a() + &survives_b()));
    fx.write(
        "hook.sh",
        "if [ \"$1\" = build ] && [ \"$MUTATE_ID\" = b ] && [ ! -e .backdated ]; then\n\
         touch -t 200101010000 src/lib.rs; : > .backdated; fi\n",
    );
    fx.commit("fixture");
    let out = fx.mutate(&["run"]);
    assert!(
        fx.dir.join(".backdated").exists(),
        "the hook must have fired"
    );
    assert!(out.stdout().contains("SURVIVED\tb\t"), "{}", out.all());
    assert_eq!(
        count(&fx.calls(), "b build"),
        2,
        "rebuilt once: {:?}",
        fx.calls()
    );
    assert_eq!(fx.read("src/lib.rs"), FIXTURE_LIB);
}

#[test]
fn a_compile_failure_is_unviable_not_caught() {
    let fx = Fx::committed("unviable", &mutant("c", "{ 1 }", "{ \"\" }", "one_is_one"));
    let out = fx.mutate(&["run"]);
    assert_eq!(out.code(), Some(1), "{}", out.all());
    assert!(out.stdout().contains("unviable\tc\t"), "{}", out.all());
    assert!(
        out.stdout().contains("0 of 1 caught; survivors: []"),
        "{}",
        out.all()
    );
    assert!(out.stdout().contains("unviable: [c]"), "{}", out.all());
    assert_eq!(
        count(&fx.calls(), "c test"),
        0,
        "no test step after a failed build"
    );
}

#[test]
fn a_failing_control_aborts_before_any_mutant() {
    let fx = Fx::committed(
        "control-fails",
        &mutant("a", "{ 1 }", "{ 2 }", "always_fails"),
    );
    let out = fx.mutate(&["run"]);
    assert_eq!(out.code(), Some(2), "{}", out.all());
    assert!(out.stdout().contains("CONTROL FAILED"), "{}", out.all());
    assert!(
        fx.calls().iter().all(|c| c.starts_with("control ")),
        "{:?}",
        fx.calls()
    );
    assert_eq!(fx.read("src/lib.rs"), FIXTURE_LIB);
}

#[test]
fn a_control_that_runs_no_test_aborts() {
    let fx = Fx::committed(
        "control-empty",
        &mutant("a", "{ 1 }", "{ 2 }", "no_such_test"),
    );
    let out = fx.mutate(&["run"]);
    assert_eq!(out.code(), Some(2), "{}", out.all());
    assert!(out.stdout().contains("CONTROL FAILED") && out.stdout().contains("0 tests run"));
    assert_eq!(count(&fx.calls(), "a build"), 0, "{:?}", fx.calls());
}

#[test]
fn resume_skips_recorded_ids_and_only_reruns_the_named_one() {
    let fx = Fx::committed("resume", &(caught_a() + &survives_b()));
    fx.mutate(&["run"]);
    let first = fx.calls().len();

    let out = fx.mutate(&["run"]);
    assert!(
        out.stdout().contains("0 to run, 2 already recorded"),
        "{}",
        out.all()
    );
    assert!(
        out.stdout().contains("1 of 2 caught; survivors: [b]"),
        "{}",
        out.all()
    );
    assert_eq!(fx.calls().len(), first, "nothing re-ran: {:?}", fx.calls());

    let out = fx.mutate(&["run", "--only", "b"]);
    assert!(out.stdout().contains("1 to run"), "{}", out.all());
    let calls = fx.calls();
    assert_eq!(
        calls[first..],
        ["b build", "b test"],
        "control already recorded"
    );
}

#[test]
fn a_verdict_recorded_on_other_code_is_not_reused() {
    let fx = Fx::committed("stale-result", &caught_a());
    fx.mutate(&["run"]);
    fx.write(
        "src/other.rs",
        "pub fn seven() -> i32 { 7 }\n// newer code\n",
    );
    fx.commit("newer code");
    let out = fx.mutate(&["run"]);
    assert!(
        out.stdout().contains("1 to run, 0 already recorded"),
        "{}",
        out.all()
    );
    assert_eq!(count(&fx.calls(), "a test"), 2, "{:?}", fx.calls());
}

#[test]
fn a_run_killed_mid_mutant_is_restored_on_the_next_start() {
    let fx = Fx::new("killed", &caught_a());
    // SIGKILL the tool itself during the mutant's build: nothing in it runs.
    fx.write(
        "hook.sh",
        "if [ \"$1\" = build ] && [ \"$MUTATE_ID\" != control ]; then kill -KILL $PPID; exit 1; fi\n",
    );
    fx.commit("fixture");
    let out = fx.mutate(&["run"]);
    assert_eq!(out.code(), None, "killed by a signal: {}", out.all());
    assert!(
        fx.read("src/lib.rs").contains("{ 2 }"),
        "the mutant is still on disk"
    );
    assert!(fx.state("journal.toml").exists());

    let out = fx.mutate(&["--check"]);
    assert_eq!(out.code(), Some(0), "{}", out.all());
    assert!(out.stdout().contains("restored"), "{}", out.all());
    assert_eq!(fx.read("src/lib.rs"), FIXTURE_LIB);
    assert!(!fx.state("journal.toml").exists());
}

#[test]
fn sigterm_mid_mutant_restores_and_exits() {
    let fx = Fx::new("sigterm", &caught_a());
    fx.write(
        "hook.sh",
        "if [ \"$1\" = build ] && [ \"$MUTATE_ID\" != control ]; then : > started; exec sleep 60; fi\n",
    );
    fx.commit("fixture");
    let mut child = fx
        .command(&["run"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    while !fx.dir.join("started").exists() {
        assert!(Instant::now() < deadline, "the mutant build never started");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        fx.read("src/lib.rs").contains("{ 2 }"),
        "mutated while the build runs"
    );
    let pid = child.id().to_string();
    let killed = Instant::now();
    std::process::Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .unwrap();
    let status = child.wait().unwrap();
    assert!(
        killed.elapsed() < Duration::from_secs(30),
        "stopped the build, did not wait it out"
    );
    assert_eq!(status.code(), Some(130));
    assert_eq!(fx.read("src/lib.rs"), FIXTURE_LIB);
    assert!(!fx.state("journal.toml").exists());
}

#[test]
fn a_killed_build_is_retried_once() {
    let fx = Fx::new("killed-build-retried", &caught_a());
    // The first build of `a` dies by a signal, as an OOM kill would.
    fx.write(
        "hook.sh",
        "if [ \"$1\" = build ] && [ \"$MUTATE_ID\" = a ] && [ ! -e started ]; then\n\
         : > started; kill -KILL $$; fi\n",
    );
    fx.commit("fixture");
    let out = fx.mutate(&["run"]);
    assert_eq!(out.code(), Some(0), "{}", out.all());
    assert!(out.stdout().contains("caught\ta\t"), "{}", out.all());
    assert_eq!(count(&fx.calls(), "a build"), 2, "{:?}", fx.calls());
}

#[test]
fn a_build_killed_every_time_is_no_result_not_a_verdict() {
    let fx = Fx::new("killed-build-twice", &caught_a());
    fx.write(
        "hook.sh",
        "if [ \"$1\" = build ] && [ \"$MUTATE_ID\" = a ]; then kill -KILL $$; fi\n",
    );
    fx.commit("fixture");
    let out = fx.mutate(&["run"]);
    assert_eq!(out.code(), Some(1), "{}", out.all());
    assert!(out.stdout().contains("no-result\ta\t"), "{}", out.all());
    assert!(
        out.stdout().contains("0 of 1 caught; survivors: []"),
        "{}",
        out.all()
    );
    assert!(out.stdout().contains("no result: [a]"), "{}", out.all());
    assert_eq!(
        count(&fx.calls(), "a build"),
        2,
        "retried once: {:?}",
        fx.calls()
    );
    assert_eq!(fx.read("src/lib.rs"), FIXTURE_LIB);
}

#[test]
fn an_equivalent_mutant_is_listed_and_never_run() {
    let area = caught_a().replace(
        "[[mutant.edit]]",
        "equivalent = \"no test can tell\"\n[[mutant.edit]]",
    );
    let fx = Fx::committed("equivalent", &area);
    let out = fx.mutate(&["run"]);
    assert_eq!(out.code(), Some(0), "{}", out.all());
    assert!(out.stdout().contains("0 of 0 caught"), "{}", out.all());
    assert!(
        out.stdout()
            .contains("equivalent, not run: a (no test can tell)"),
        "{}",
        out.all()
    );
    assert!(fx.calls().is_empty(), "{:?}", fx.calls());
}
