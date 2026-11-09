//! Dates on a history with branches and merges, against per-file git. Every
//! repository is synthetic.

use std::collections::HashMap;

use super::git_tests::{git_in_status, long_note, note, Repo};
use crate::seed::*;

const T: i64 = 1_700_000_000;

/// The scenario, one page per case:
/// - `a`: edited on a branch (merged cleanly), then again on main;
/// - `b`: edited on main only;
/// - `c`: edited on both sides, the conflict resolved in the merge;
/// - `d`: edited on the branch, the edit DISCARDED by the merge;
/// - `e`: edited on a second branch, merged cleanly;
/// - `f`: created on the second branch.
fn merge_repo() -> Repo {
    let r = Repo::new();
    for id in ["a", "b", "c", "d", "e"] {
        r.write(&format!("{id}.org"), &long_note(id));
    }
    let root = r.commit(T + 100);
    r.git(&["checkout", "-q", "-b", "br1", &root]);
    r.write("a.org", &(long_note("a") + "rama 1\n"));
    r.commit(T + 200);
    r.write("c.org", &(long_note("c") + "rama 1\n"));
    r.write("d.org", &(long_note("d") + "rama 1, descartada\n"));
    r.commit(T + 300);
    r.git(&["checkout", "-q", "main"]);
    r.write("b.org", &(long_note("b") + "principal\n"));
    r.write("c.org", &(long_note("c") + "principal\n"));
    r.commit(T + 400);
    let (clean, _) = git_in_status(
        r.path(),
        &["merge", "-q", "--no-ff", "-m", "m1", "br1"],
        T + 500,
    );
    assert!(!clean, "the merge must stop on the conflict in c.org");
    r.write("c.org", &(long_note("c") + "resuelta\n"));
    r.git(&["checkout", "HEAD", "--", "d.org"]); // discard the branch's edit to d
    r.git(&["add", "-A"]);
    r.git_with_time(&["commit", "-q", "--no-edit"], T + 500);
    r.git(&["checkout", "-q", "-b", "br2"]);
    r.write("e.org", &(long_note("e") + "rama 2\n"));
    r.write("f.org", &note("f", "nueva en la rama 2\n"));
    r.commit(T + 600);
    r.git(&["checkout", "-q", "main"]);
    r.write("a.org", &(long_note("a") + "rama 1\nprincipal\n"));
    r.commit(T + 700);
    r.git_with_time(&["merge", "-q", "--no-ff", "-m", "m2", "br2"], T + 800);
    r.git(&["tag", "v1"]);
    r
}

/// `modified_at` equals `git log -1 --format=%ct -- PATH` for every page,
/// including the conflict resolved in a merge (the review's 3000-vs-5000
/// case) and the branch edit the merge discarded; `created_at` is the first
/// appearance of each id, a branch-created page dated by its branch commit.
/// Kills: following merges' first parents only, treating a merge like a
/// one-parent commit, and dating by walk order.
#[test]
fn times_on_a_merge_history_agree_with_per_file_git() {
    let r = merge_repo();
    let plan = r.plan("v1");
    assert!(plan.ok(), "{:?}", plan.refusals);
    let got: HashMap<&str, (i64, i64)> = plan
        .nodes
        .iter()
        .map(|n| (n.path.as_str(), (n.created_at, n.modified_at)))
        .collect();
    for path in ["a.org", "b.org", "c.org", "d.org", "e.org", "f.org"] {
        let last: i64 = r
            .git(&["log", "-1", "--format=%ct", "v1", "--", path])
            .parse()
            .unwrap();
        assert_eq!(got[path].1, last, "{path}: modified_at against git log -1");
    }
    let want = [
        ("a.org", (T + 100, T + 700)),
        ("b.org", (T + 100, T + 400)),
        ("c.org", (T + 100, T + 500)),
        ("d.org", (T + 100, T + 100)),
        ("e.org", (T + 100, T + 600)),
        ("f.org", (T + 600, T + 600)),
    ];
    for (path, times) in want {
        assert_eq!(got[path], times, "{path}");
    }
}

/// A historical version whose drawer ends beyond the read prefix carries no
/// id for dating: only a bounded prefix of each historical blob is held.
/// Kills: reading whole historical blobs.
#[test]
fn only_a_bounded_prefix_of_each_historical_version_is_read() {
    let r = Repo::new();
    let huge = "x".repeat(HISTORY_PREFIX_BYTES + 100);
    r.write(
        "p.org",
        &format!(":PROPERTIES:\n:ID: p\n:LARGO: {huge}\n:END:\nantes\n"),
    );
    r.commit(T + 100);
    r.write("p.org", &note("p", "después\n"));
    r.commit(T + 200);
    r.git(&["tag", "v1"]);
    let plan = r.plan("v1");
    assert!(plan.ok(), "{:?}", plan.refusals);
    assert_eq!(
        plan.nodes[0].created_at,
        T + 200,
        "the first version's drawer was past the prefix"
    );
}
