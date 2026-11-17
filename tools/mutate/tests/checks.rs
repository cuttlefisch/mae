//! The commands that run no cargo: `--check`, `list`, `--since`, `--shard`,
//! the lock and the dirty-file refusal.

mod common;
use common::*;
use std::collections::BTreeSet;

#[test]
fn an_anchor_found_zero_times_is_refused() {
    let fx = Fx::committed("anchor-zero", &mutant("z", "{ 9 }", "{ 2 }", "one_is_one"));
    let out = fx.mutate(&["--check"]);
    assert_eq!(out.code(), Some(2), "{}", out.all());
    assert!(
        out.stderr()
            .contains("z: anchor found 0 times in src/lib.rs"),
        "{}",
        out.all()
    );
}

#[test]
fn an_anchor_found_twice_is_refused() {
    let fx = Fx::committed("anchor-twice", &mutant("t", "pub fn", "fn", "one_is_one"));
    let out = fx.mutate(&["--check"]);
    assert_eq!(out.code(), Some(2), "{}", out.all());
    assert!(
        out.stderr()
            .contains("t: anchor found 2 times in src/lib.rs"),
        "{}",
        out.all()
    );
}

#[test]
fn a_second_edit_is_checked_against_the_first_ones_result() {
    // Alone, each anchor occurs once; after the first edit the second's text
    // occurs twice, which is what the mutant would really meet.
    let two = "[[mutant]]\nid = \"m\"\nrunner = \"fx\"\nfilter = \"one_is_one\"\n\
               [[mutant.edit]]\nfile = \"src/lib.rs\"\nfind = '{ 5 }'\nreplace = '{ 1 }'\n\
               [[mutant.edit]]\nfile = \"src/lib.rs\"\nfind = '{ 1 }'\nreplace = '{ 2 }'\n";
    let fx = Fx::committed("anchor-sequential", two);
    let out = fx.mutate(&["--check"]);
    assert_eq!(out.code(), Some(2), "{}", out.all());
    assert!(
        out.stderr().contains("m: anchor found 2 times"),
        "{}",
        out.all()
    );
}

#[test]
fn a_valid_list_passes_the_check() {
    let fx = Fx::committed("check-ok", &(caught_a() + &survives_b()));
    let out = fx.mutate(&["--check"]);
    assert_eq!(out.code(), Some(0), "{}", out.all());
    assert!(out.stdout().contains("2 mutants"), "{}", out.all());
}

#[test]
fn duplicate_ids_and_unknown_runners_are_refused() {
    let dup = caught_a() + &mutant("a", "{ 5 }", "{ 6 }", "one_is_one");
    let out = Fx::committed("dup-id", &dup).mutate(&["--check"]);
    assert_eq!(out.code(), Some(2), "{}", out.all());
    assert!(out.stderr().contains("duplicate id a"), "{}", out.all());

    let unknown = caught_a().replace("runner = \"fx\"", "runner = \"nope\"");
    let out = Fx::committed("unknown-runner", &unknown).mutate(&["--check"]);
    assert_eq!(out.code(), Some(2), "{}", out.all());
    assert!(out.stderr().contains("unknown runner"), "{}", out.all());
}

#[test]
fn a_dirty_file_a_mutant_touches_is_refused_before_anything_runs() {
    let fx = Fx::committed("dirty", &caught_a());
    let edited = FIXTURE_LIB.to_string() + "// local edit\n";
    fx.write("src/lib.rs", &edited);
    let out = fx.mutate(&["run"]);
    assert_eq!(out.code(), Some(2), "{}", out.all());
    assert!(
        out.stderr().contains("uncommitted changes"),
        "{}",
        out.all()
    );
    assert!(fx.calls().is_empty(), "nothing may run: {:?}", fx.calls());
    assert_eq!(
        fx.read("src/lib.rs"),
        edited,
        "the local edit must be untouched"
    );
}

fn ids(out: &Out) -> Vec<String> {
    out.stdout()
        .lines()
        .map(|l| l.split('\t').next().unwrap().to_string())
        .collect()
}

#[test]
fn since_selects_by_the_files_a_mutant_edits() {
    let area = caught_a() + &mutant_in("o", "src/other.rs", "{ 7 }", "{ 8 }", "one_is_one");
    let fx = Fx::committed("since", &area);
    fx.git(&["tag", "base"]);
    fx.write("src/other.rs", "pub fn seven() -> i32 { 7 }\n// changed\n");
    fx.commit("touch other");
    assert_eq!(
        ids(&fx.mutate(&["list", "--since", "base"])),
        ["o"],
        "committed change"
    );
    assert!(
        ids(&fx.mutate(&["list", "--since", "HEAD"])).is_empty(),
        "nothing since HEAD"
    );

    fx.write("src/lib.rs", &(FIXTURE_LIB.to_string() + "// wip\n"));
    assert_eq!(
        ids(&fx.mutate(&["list", "--since", "HEAD"])),
        ["a"],
        "working tree change"
    );
    fx.git(&["checkout", "--", "src/lib.rs"]);

    fx.write("mutants/fx.toml", &(area + "# edited\n"));
    assert_eq!(
        ids(&fx.mutate(&["list", "--since", "HEAD"])),
        ["a", "o"],
        "list change"
    );
}

#[test]
fn shards_partition_by_id_and_are_stable() {
    let area: String = (0..24)
        .map(|i| mutant(&format!("m{i}"), "{ 1 }", "{ 2 }", "x"))
        .collect();
    let fx = Fx::committed("shard", &area);
    let all: BTreeSet<String> = ids(&fx.mutate(&["list"])).into_iter().collect();
    assert_eq!(all.len(), 24);
    let mut seen = BTreeSet::new();
    for i in 1..=3 {
        let arg = format!("{i}/3");
        let shard = ids(&fx.mutate(&["list", "--shard", &arg]));
        assert!(
            !shard.is_empty() && shard.len() < 24,
            "shard {i}: {shard:?}"
        );
        assert_eq!(shard, ids(&fx.mutate(&["list", "--shard", &arg])), "stable");
        for id in shard {
            assert!(seen.insert(id.clone()), "{id} in two shards");
        }
    }
    assert_eq!(seen, all, "every mutant in exactly one shard");
    assert_eq!(fx.mutate(&["list", "--shard", "0/3"]).code(), Some(2));
    assert_eq!(fx.mutate(&["list", "--shard", "4/3"]).code(), Some(2));
}

#[test]
fn a_live_lock_is_refused_and_a_dead_ones_is_taken_over() {
    let fx = Fx::committed("lock", &caught_a());
    assert_eq!(fx.mutate(&["--check"]).code(), Some(0));
    fx.write(".git/mae-mutate/lock", &std::process::id().to_string());
    let out = fx.mutate(&["--check"]);
    assert_eq!(out.code(), Some(2), "{}", out.all());
    assert!(
        out.stderr().contains("another mae-mutate run"),
        "{}",
        out.all()
    );

    let mut gone = std::process::Command::new("true").spawn().unwrap();
    let dead = gone.id();
    gone.wait().unwrap();
    fx.write(".git/mae-mutate/lock", &dead.to_string());
    let out = fx.mutate(&["--check"]);
    assert_eq!(out.code(), Some(0), "{}", out.all());
    assert!(!fx.state("lock").exists(), "released on exit");
}
