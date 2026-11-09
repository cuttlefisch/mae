//! Test 7: planning from a real local repository at a tag. Every repository is
//! a fresh synthetic one in a temp dir; no global or system git config is read
//! by the commands that build it.

use std::path::Path;
use std::process::Command;

use super::page;
use crate::seed::*;

struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    fn new() -> Repo {
        let r = Repo {
            dir: tempfile::tempdir().unwrap(),
        };
        r.git(&["init", "-q", "-b", "main"]);
        r
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn git_with_time(&self, args: &[&str], time: i64) -> String {
        let stamp = format!("{time} +0000");
        let mut c = Command::new("git");
        c.arg("-C").arg(self.path());
        c.args([
            "-c",
            "user.name=seed-test",
            "-c",
            "user.email=seed@test.invalid",
        ]);
        c.args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"]);
        c.args(args);
        for var in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
        ] {
            c.env_remove(var);
        }
        c.env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
        c.env("GIT_AUTHOR_DATE", &stamp)
            .env("GIT_COMMITTER_DATE", &stamp);
        let out = c.output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn git(&self, args: &[&str]) -> String {
        self.git_with_time(args, 1_600_000_000)
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn commit(&self, time: i64) -> String {
        self.git(&["add", "-A"]);
        self.git_with_time(&["commit", "-q", "-m", "c"], time);
        self.git(&["rev-parse", "HEAD"])
    }

    fn plan(&self, rev: &str) -> SeedPlan {
        plan(&PlanOptions {
            repo: self.path().to_path_buf(),
            rev: rev.into(),
            include: Vec::new(),
            exclude: Vec::new(),
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            require_canonical: false,
            code_property: None,
        })
        .unwrap()
    }
}

fn note(id: &str, body: &str) -> String {
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

/// Q6: each file's creation time is the last commit that changed it at the
/// tag, from ONE `git log` pass. Kills: the tag's own time for every file,
/// the first commit's time, or counting commits after the tag.
#[test]
fn created_at_is_each_files_last_commit_time_at_the_tag() {
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
    let times: Vec<(&str, i64, bool)> = plan
        .nodes
        .iter()
        .map(|n| (n.path.as_str(), n.created_at, n.created_at_from_git))
        .collect();
    assert_eq!(
        times,
        vec![
            ("a.org", 1_700_000_500, true),
            ("dir/b.org", 1_700_000_000, true),
            ("dir/c.org", 1_700_000_900, true)
        ]
    );
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
        let opts = PlanOptions {
            repo: r.path().to_path_buf(),
            rev: rev.into(),
            include: Vec::new(),
            exclude: Vec::new(),
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            require_canonical: false,
            code_property: None,
        };
        assert!(plan(&opts).is_err(), "{rev:?} must not plan");
    }
}

/// The reader cannot be redirected to another repository by the environment
/// it was started in (a hook's or a worktree build's `GIT_DIR`).
#[test]
fn git_redirection_variables_are_removed() {
    let cmd = GitRepo::new("/r").command();
    let removed: Vec<String> = cmd
        .get_envs()
        .filter(|(_, v)| v.is_none())
        .map(|(k, _)| k.to_string_lossy().into_owned())
        .collect();
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_COMMON_DIR",
    ] {
        assert!(removed.iter().any(|r| r == var), "{var} is not removed");
    }
}
