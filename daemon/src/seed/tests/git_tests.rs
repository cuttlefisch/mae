//! Test 7: planning from a real local repository at a tag. Every repository is
//! a fresh synthetic one in a temp dir; no global or system git config is read
//! by the commands that build it.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use super::{page, plan_opts};
use crate::seed::*;

/// Run the TEST's own git: an empty environment, no global or system config
/// (the developer's must not leak in, and their commits must never be signed
/// with the owner's key), fixed dates, and local clones allowed.
pub(super) fn git_in(dir: &Path, args: &[&str], time: i64) -> String {
    let stamp = format!("{time} +0000");
    let mut c = Command::new("git");
    c.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        c.env("PATH", path);
    }
    c.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    c.env("GIT_AUTHOR_DATE", &stamp)
        .env("GIT_COMMITTER_DATE", &stamp);
    c.arg("-C").arg(dir);
    for cfg in [
        "user.name=seed-test",
        "user.email=seed@test.invalid",
        "commit.gpgsign=false",
        "tag.gpgsign=false",
        "core.hooksPath=/dev/null",
        "protocol.file.allow=always",
        "uploadpack.allowFilter=true",
    ] {
        c.args(["-c", cfg]);
    }
    c.args(args);
    let out = c.output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

pub(super) struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    pub(super) fn new() -> Repo {
        let r = Repo {
            dir: tempfile::tempdir().unwrap(),
        };
        r.git(&["init", "-q", "-b", "main"]);
        r
    }

    pub(super) fn path(&self) -> &Path {
        self.dir.path()
    }

    pub(super) fn git_with_time(&self, args: &[&str], time: i64) -> String {
        git_in(self.path(), args, time)
    }

    pub(super) fn git(&self, args: &[&str]) -> String {
        self.git_with_time(args, 1_600_000_000)
    }

    pub(super) fn write(&self, rel: &str, text: &str) {
        let p = self.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    pub(super) fn commit(&self, time: i64) -> String {
        self.git(&["add", "-A"]);
        self.git_with_time(&["commit", "-q", "-m", "c"], time);
        self.git(&["rev-parse", "HEAD"])
    }

    pub(super) fn plan(&self, rev: &str) -> SeedPlan {
        plan(&plan_opts(self.path(), rev)).unwrap()
    }
}

pub(super) fn note(id: &str, body: &str) -> String {
    page(&[("ID", id)], Some(id), None, body)
}

/// Kills: reading files from the working copy or the index instead of the
/// tagged commit's blobs.
#[test]
fn the_plan_reads_the_tag_not_the_working_copy() {
    let r = Repo::new();
    r.write("notes/a.org", &note("a", "uno\n"));
    r.write("notes/b.org", &note("b", "dos\n"));
    r.commit(1_700_000_000);
    r.git(&["tag", "v1"]);
    let clean = r.plan("v1");
    assert!(clean.ok(), "{:?}", clean.refusals);

    r.write("notes/a.org", "not even an org page\r\n"); // unstaged edit
    r.write("notes/c.org", &note("c", "untracked\n"));
    r.write("notes/b.org", &note("b", "staged edit\n"));
    r.git(&["add", "notes/b.org"]);
    let dirty = r.plan("v1");
    assert_eq!(dirty.plan_hash(), clean.plan_hash());
    assert_eq!(dirty.nodes, clean.nodes);
    assert_eq!(dirty.nodes[0].file.body, "uno\n");
}

/// A tag moved to another commit is visible in the source and the hash.
#[test]
fn a_moved_tag_changes_the_commit_and_the_hash() {
    let r = Repo::new();
    r.write("a.org", &note("a", "uno\n"));
    let first = r.commit(1_700_000_000);
    r.git(&["tag", "v1"]);
    let before = r.plan("v1");
    r.write("a.org", &note("a", "otro\n"));
    let second = r.commit(1_700_000_100);
    r.git(&["tag", "-f", "v1"]);
    let after = r.plan("v1");
    assert_eq!(before.source.commit, first);
    assert_eq!(after.source.commit, second);
    assert_ne!(before.plan_hash(), after.plan_hash());
}

/// Creation is the file's first add, modification its last change, both at
/// the tag. Kills: swapping first and last, using the tag's own time, or
/// counting a commit made after the tag.
#[test]
fn created_and_modified_are_the_first_add_and_last_change_at_the_tag() {
    let r = Repo::new();
    r.write("a.org", &note("a", "1\n"));
    r.write("dir/b.org", &note("b", "1\n"));
    r.commit(1_700_000_000);
    r.write("a.org", &note("a", "2\n"));
    r.commit(1_700_000_500);
    r.write("dir/c.org", &note("c", "1\n"));
    r.commit(1_700_000_900);
    r.git(&["tag", "v1"]);
    r.write("dir/b.org", &note("b", "after the tag\n"));
    r.commit(1_700_009_999);
    let plan = r.plan("v1");
    let times: Vec<(&str, i64, i64)> = plan
        .nodes
        .iter()
        .map(|n| (n.path.as_str(), n.created_at, n.modified_at))
        .collect();
    assert_eq!(
        times,
        vec![
            ("a.org", 1_700_000_000, 1_700_000_500),
            ("dir/b.org", 1_700_000_000, 1_700_000_000),
            ("dir/c.org", 1_700_000_900, 1_700_000_900)
        ]
    );
    assert!(
        plan.nodes
            .iter()
            .all(|n| n.created_source == TimeSource::GitId
                && n.modified_source == TimeSource::GitPath)
    );
}

/// A page with an id and twenty distinct lines, so git's similarity index
/// sees a rename or copy as one.
pub(super) fn long_note(id: &str) -> String {
    let body: String = (1..=20).map(|i| format!("línea {i} de {id}\n")).collect();
    note(id, &body)
}

const T: i64 = 1_700_000_000;

/// The history the comparison runs over, one scenario per file.
fn history_repo() -> Repo {
    let r = Repo::new();
    for id in ["plain", "old-name", "re", "gone", "src", "b1", "b2", "b3"] {
        r.write(&format!("{id}.org"), &long_note(id));
    }
    r.commit(T + 100);
    r.git(&["mv", "old-name.org", "renamed.org"]);
    r.commit(T + 200); // a pure rename
    r.git(&["mv", "re.org", "re2.org"]);
    r.write("re2.org", &(long_note("re") + "una línea más\n"));
    r.commit(T + 300); // a rename and an edit in one commit
    r.git(&["rm", "-q", "gone.org"]);
    r.commit(T + 400);
    r.write("gone.org", &long_note("gone-again"));
    r.commit(T + 500); // the same path added again
    r.write(
        "copy.org",
        &long_note("src").replace(":ID: src", ":ID: copy"),
    );
    r.commit(T + 600); // a copy (its id changed, so it is a distinct page)
    std::fs::create_dir_all(r.path().join("bulk")).unwrap();
    // New basenames: git pairs a file that keeps its basename across
    // directories cheaply, before its rename limit applies, so only a renamed
    // basename exercises the limit `-l0` lifts.
    for b in ["b1", "b2", "b3"] {
        r.git(&["mv", &format!("{b}.org"), &format!("bulk/moved-{b}.org")]);
        r.write(
            &format!("bulk/moved-{b}.org"),
            &(long_note(b) + "editada\n"),
        );
    }
    r.commit(T + 700); // several moves with edits in one commit
    r.write("plain.org", &(long_note("plain") + "cambio\n"));
    r.commit(T + 800);
    r.git(&["tag", "v1"]);
    r.write("renamed.org", &(long_note("old-name") + "después\n"));
    r.commit(T + 900); // after the tag: must not count
    r
}

/// What per-file `git log` says: `--diff-filter=A --follow … | tail -1` for
/// creation, `-1` for the last change.
fn oracle(r: &Repo, path: &str) -> (i64, i64) {
    let created = r.git(&[
        "log",
        "--diff-filter=A",
        "--follow",
        "--format=%ct",
        "v1",
        "--",
        path,
    ]);
    let modified = r.git(&["log", "-1", "--format=%ct", "v1", "--", path]);
    let last = created.lines().last().unwrap().parse().unwrap();
    (last, modified.parse().unwrap())
}

/// The one-pass walk against per-file `git log --follow`. Every page here
/// keeps its id along its path, so id and path agree, and the walk must agree
/// with `--follow` on renames (pure, with an edit, and several in one commit,
/// under a rename limit that would make git skip them). It differs, by
/// decision and pinned here, where the id changed: a path re-added under a
/// new id, and a copy that took its own id. Kills: swapping first and last, a
/// walk that does not follow renames, dropping `-l0`, and continuing a deleted
/// path's lineage into its re-add.
#[test]
fn the_history_walk_agrees_with_git_log_follow_except_where_decided() {
    let r = history_repo();
    let paths = [
        "plain.org",
        "renamed.org",
        "re2.org",
        "gone.org",
        "src.org",
        "copy.org",
        "bulk/moved-b1.org",
        "bulk/moved-b2.org",
        "bulk/moved-b3.org",
    ];
    let expected: Vec<(i64, i64)> = paths.iter().map(|p| oracle(&r, p)).collect();
    // A rename limit git would apply to the walk; `-l0` must override it.
    r.git(&["config", "diff.renameLimit", "1"]);
    let plan = r.plan("v1");
    assert!(plan.ok(), "{:?}", plan.refusals);
    let walked: HashMap<&str, (i64, i64)> = plan
        .nodes
        .iter()
        .map(|n| (n.path.as_str(), (n.created_at, n.modified_at)))
        .collect();
    for (path, (follow_created, last_change)) in paths.iter().zip(expected) {
        let (created, modified) = walked[path];
        assert_eq!(modified, last_change, "{path}: modified_at");
        match *path {
            "gone.org" => {
                assert_eq!(follow_created, T + 100, "--follow reaches the first add");
                assert_eq!(created, T + 500, "{path}: the re-add is the creation");
            }
            "copy.org" => {
                assert_eq!(
                    follow_created,
                    T + 100,
                    "--follow follows the copy to its source"
                );
                assert_eq!(created, T + 600, "{path}: a copy is created by the copy");
            }
            _ => assert_eq!(
                created, follow_created,
                "{path}: created_at agrees with --follow"
            ),
        }
    }
    assert_eq!(walked["renamed.org"], (T + 100, T + 200));
    assert_eq!(walked["re2.org"], (T + 100, T + 300));
    assert_eq!(walked["bulk/moved-b2.org"], (T + 100, T + 700));
    assert_eq!(walked["plain.org"], (T + 100, T + 800));
}

/// Attributes are read from the commit, not the working copy. Kills: a check
/// against the working copy's `.gitattributes`, and no check at all.
#[test]
fn checkout_attributes_of_the_commit_are_refused() {
    let r = Repo::new();
    r.write("a.org", &note("a", "x\n"));
    r.write(".gitattributes", "* text=auto\n");
    r.commit(1_700_000_000);
    r.git(&["tag", "ok"]);
    r.write(".gitattributes", "*.org eol=crlf\n"); // uncommitted: irrelevant
    assert!(
        r.plan("ok").ok(),
        "text=auto alone does not change checkout bytes"
    );

    r.write(".gitattributes", "*.org eol=crlf\nnotes/** filter=lfs\n");
    r.write("notes/b.org", &note("b", "y\n"));
    r.commit(1_700_000_100);
    r.git(&["tag", "bad"]);
    let plan = r.plan("bad");
    let got: Vec<(&str, &str)> = plan
        .refusals
        .iter()
        .filter(|x| x.kind == ProblemKind::CheckoutAttribute)
        .map(|x| (x.path.as_str(), x.detail.split(' ').next().unwrap()))
        .collect();
    assert!(got.contains(&("a.org", "eol=crlf")), "{got:?}");
    assert!(got.contains(&("notes/b.org", "filter=lfs")), "{got:?}");
}

#[cfg(unix)]
#[test]
fn symlinks_and_gitlinks_in_the_tree_are_refused() {
    let r = Repo::new();
    r.write("a.org", &note("a", "x\n"));
    std::os::unix::fs::symlink("a.org", r.path().join("link.org")).unwrap();
    let head = r.commit(1_700_000_000);
    r.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{head},sub.org"),
    ]);
    r.git_with_time(&["commit", "-q", "-m", "gitlink"], 1_700_000_100);
    r.git(&["tag", "v1"]);
    let kinds: Vec<(String, ProblemKind)> = r
        .plan("v1")
        .refusals
        .into_iter()
        .map(|x| (x.path, x.kind))
        .collect();
    assert!(
        kinds.contains(&("link.org".into(), ProblemKind::Symlink)),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&("sub.org".into(), ProblemKind::Submodule)),
        "{kinds:?}"
    );
}

#[test]
fn a_revision_that_is_not_a_commit_is_an_error() {
    let r = Repo::new();
    r.write("a.org", &note("a", "x\n"));
    r.commit(1_700_000_000);
    let tree = r.git(&["rev-parse", "HEAD^{tree}"]);
    for rev in ["no-such-tag", tree.as_str(), "-n", ""] {
        assert!(
            plan(&plan_opts(r.path(), rev)).is_err(),
            "{rev:?} must not plan"
        );
    }
}

/// Q6 by id: a document's creation is the first commit in which its `:ID:`
/// appeared, at any path. Each page below tells path-based dating apart from
/// id-based dating. Kills: dating by path instead of by id, and keeping the
/// LAST appearance of an id instead of the first.
#[test]
fn created_at_follows_the_id_not_the_path() {
    let r = Repo::new();
    r.write("notes/x/moved.org", &long_note("moved"));
    r.write("restored.org", &long_note("restored"));
    r.write("regen.org", &long_note("regen-old"));
    r.write("a/first.org", &long_note("wanders"));
    r.commit(T + 100);
    r.git(&["rm", "-q", "notes/x/moved.org", "restored.org"]);
    r.commit(T + 200); // a folder move begins as a delete, and a page is removed
    r.write(
        "notes/y/moved.org",
        &(long_note("moved") + "editada al moverla\n"),
    );
    r.write("restored.org", &long_note("restored"));
    r.commit(T + 300); // the move completes as an add; the page is restored
    r.write("regen.org", &long_note("regen-new"));
    r.write("b/second.org", &(long_note("wanders") + "ahora aquí\n"));
    r.commit(T + 400); // regenerated under a new id; the id appears at path B
    r.git(&["rm", "-q", "a/first.org"]);
    r.commit(T + 500); // and is gone from path A
    r.git(&["tag", "v1"]);
    let plan = r.plan("v1");
    assert!(plan.ok(), "{:?}", plan.refusals);
    let got: HashMap<&str, (i64, i64)> = plan
        .nodes
        .iter()
        .map(|n| (n.path.as_str(), (n.created_at, n.modified_at)))
        .collect();
    assert_eq!(
        got["notes/y/moved.org"],
        (T + 100, T + 300),
        "a move by delete + add keeps the date"
    );
    assert_eq!(
        got["restored.org"],
        (T + 100, T + 300),
        "a restore keeps the date"
    );
    assert_eq!(
        got["regen.org"],
        (T + 400, T + 400),
        "a new id at the same path is a new document"
    );
    assert_eq!(
        got["b/second.org"],
        (T + 100, T + 400),
        "the id's first path counts"
    );
    assert!(plan
        .nodes
        .iter()
        .all(|n| n.created_source == TimeSource::GitId));
    assert!(
        plan.history_blobs_read >= 7,
        "every version of every page was read once"
    );
}

/// Copies: one that still shares its source's id at the rev is a duplicate-id
/// refusal; one that later took its own id is created when that id appears,
/// not when the copy was made. Kills: dating a copy by its path's add.
#[test]
fn a_copy_is_refused_while_it_shares_an_id_and_dated_by_its_own_id_after() {
    let r = Repo::new();
    r.write("src.org", &long_note("src"));
    r.commit(T + 100);
    r.write("copy.org", &long_note("src"));
    r.commit(T + 200);
    r.git(&["tag", "shared"]);
    r.write(
        "copy.org",
        &long_note("src").replace(":ID: src", ":ID: copy"),
    );
    r.commit(T + 300);
    r.git(&["tag", "own"]);

    let shared = r.plan("shared");
    let dup: Vec<&str> = shared
        .refusals
        .iter()
        .filter(|x| x.kind == ProblemKind::DuplicateId)
        .map(|x| x.path.as_str())
        .collect();
    assert_eq!(dup, vec!["copy.org", "src.org"]);

    let own = r.plan("own");
    assert!(own.ok(), "{:?}", own.refusals);
    let got: HashMap<&str, (i64, i64)> = own
        .nodes
        .iter()
        .map(|n| (n.path.as_str(), (n.created_at, n.modified_at)))
        .collect();
    assert_eq!(got["src.org"], (T + 100, T + 100));
    assert_eq!(
        got["copy.org"],
        (T + 300, T + 300),
        "created when its own id first appears"
    );
}

/// End to end: a selected file larger than the read limit is refused from its
/// `cat-file --batch-check` size and never read.
#[test]
fn an_oversized_page_in_the_repository_is_refused_from_its_size() {
    let r = Repo::new();
    r.write("a.org", &long_note("a"));
    r.write(
        "big.org",
        &note("big", &"x".repeat(HEADER_ALLOWANCE_BYTES + 200)),
    );
    r.commit(1_700_000_000);
    r.git(&["tag", "v1"]);
    let mut opts = plan_opts(r.path(), "v1");
    opts.max_body_bytes = 100;
    let plan = plan(&opts).unwrap();
    let big: Vec<_> = plan
        .refusals
        .iter()
        .filter(|x| x.path == "big.org")
        .collect();
    assert_eq!(big.len(), 1, "{:?}", plan.refusals);
    assert_eq!(big[0].kind, ProblemKind::BodyTooLarge);
    assert!(big[0].detail.contains("not read"), "{}", big[0].detail);
}
