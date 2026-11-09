//! The third review: objects the plan reads besides blobs must be what their
//! names say, and the repository's config must not reach outside it. Every
//! repository is synthetic.

use std::path::{Path, PathBuf};

use super::git_tests::{git_in, long_note, note, Repo};
use super::plan_opts;
use crate::seed::*;

const T: i64 = 1_700_000_000;

fn plan_err(repo: &Path) -> String {
    match plan(&plan_opts(repo, "v1")) {
        Ok(p) => panic!(
            "planned {} nodes (ids {:?}); expected a refusal",
            p.nodes.len(),
            p.nodes
                .iter()
                .map(|n| n.file.id.clone())
                .collect::<Vec<_>>()
        ),
        Err(e) => {
            assert!(
                !e.0.contains("hint:"),
                "git advice leaked into a refusal: {}",
                e.0
            );
            e.0
        }
    }
}

fn loose(objects: &Path, sha: &str) -> PathBuf {
    objects.join(&sha[..2]).join(&sha[2..])
}

/// Put the loose object `from` under the name `to`, as a forger would.
fn overwrite(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
    if dst.exists() {
        let mut perms = std::fs::metadata(dst).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(dst, perms).unwrap();
    }
    std::fs::copy(src, dst).unwrap();
}

/// A repository with a page at `d/a.org`, tagged `v1`, and a forged tree for
/// `d` (listing a page with the id "forged") written into its object store.
/// Returns (repo, real tree of `d`, forged tree).
fn repo_with_forged_subtree() -> (Repo, String, String) {
    let r = Repo::new();
    r.write("d/a.org", &long_note("a"));
    r.commit(T);
    r.git(&["tag", "v1"]);
    let real = r.git(&["rev-parse", "v1:d"]);
    r.write("d/a.org", &note("forged", "contenido falso\n"));
    r.git(&["add", "d/a.org"]);
    let forged = r.git(&["write-tree", "--prefix=d/"]);
    r.git(&["read-tree", "v1"]);
    r.git(&["checkout", "--", "d/a.org"]);
    (r, real, forged)
}

/// S-H1, the reviewer's repro: a forged loose subtree listed a forged page,
/// and the plan said OK with id "forged". Kills: checking blobs only.
#[test]
fn a_forged_subtree_refuses_the_plan() {
    let (r, real, forged) = repo_with_forged_subtree();
    let objects = r.path().join(".git/objects");
    overwrite(&loose(&objects, &forged), &loose(&objects, &real));
    let listed = git_in(r.path(), &["ls-tree", "-r", "v1"], T);
    let forged_blob = git_in(r.path(), &["rev-parse", &format!("{forged}:a.org")], T);
    assert!(
        listed.contains(&forged_blob),
        "git reads the forgery (else this proves nothing)"
    );
    let err = plan_err(r.path());
    assert!(
        err.contains(&format!("tree {real} does not hash to its name")),
        "{err}"
    );
}

/// S-H1 through alternates: the real tree is missing from the repository and
/// an alternate object directory holds a forgery under its name.
#[test]
fn a_forged_tree_in_an_alternate_refuses_the_plan() {
    let (r, real, forged) = repo_with_forged_subtree();
    let objects = r.path().join(".git/objects");
    let alt = tempfile::tempdir().unwrap();
    overwrite(&loose(&objects, &forged), &loose(alt.path(), &real));
    std::fs::remove_file(loose(&objects, &real)).unwrap();
    std::fs::write(
        objects.join("info/alternates"),
        format!("{}\n", alt.path().display()),
    )
    .unwrap();
    let listed = git_in(r.path(), &["ls-tree", "-r", "v1"], T);
    assert!(
        listed.contains("d/a.org"),
        "git reads the tree from the alternate"
    );
    let err = plan_err(r.path());
    assert!(err.contains("does not hash to its name"), "{err}");
}

/// S-M1: a forged historical commit (the root, re-dated to 1998) changed
/// `created_at`. Kills: checking only the trees, or only the rev's commit.
#[test]
fn a_forged_historical_commit_refuses_the_plan() {
    let r = Repo::new();
    r.write("a.org", &long_note("a"));
    let c1 = r.commit(T + 100);
    r.write("b.org", &long_note("b"));
    r.commit(T + 200);
    r.git(&["tag", "v1"]);
    let text = r.git(&["cat-file", "commit", &c1]) + "\n";
    let forged_text = text.replace(&format!("{} +0000", T + 100), "900000000 +0000");
    assert_ne!(forged_text, text);
    let scratch = tempfile::tempdir().unwrap();
    std::fs::write(scratch.path().join("c"), forged_text).unwrap();
    let forged = r.git(&[
        "hash-object",
        "-t",
        "commit",
        "-w",
        &scratch.path().join("c").display().to_string(),
    ]);
    let objects = r.path().join(".git/objects");
    overwrite(&loose(&objects, &forged), &loose(&objects, &c1));
    let seen = git_in(r.path(), &["log", "--format=%ct", "-1", &c1], T);
    assert_eq!(
        seen, "900000000",
        "git reads the forgery (else this proves nothing)"
    );
    let err = plan_err(r.path());
    assert!(
        err.contains(&format!("commit {c1} does not hash to its name")),
        "{err}"
    );
}

/// An annotated tag is followed, and checked, down to its commit. Kills: a
/// verifier that stops at the tag.
#[test]
fn an_annotated_tag_plans() {
    let r = Repo::new();
    r.write("a.org", &long_note("a"));
    r.commit(T);
    r.git(&["tag", "-a", "-m", "publicación", "v1"]);
    let plan = r.plan("v1");
    assert!(plan.ok(), "{:?}", plan.refusals);
    assert!(
        plan.objects_verified >= 3,
        "tag, commit and root tree: {}",
        plan.objects_verified
    );
}

/// S-L1: an include in the repository's config reads files outside it, and
/// `~user/` reaches any account's home. Kills: not checking for includes.
#[test]
fn a_repository_config_with_includes_is_refused() {
    for (key, value) in [
        ("include.path", "~root/x.config"),
        ("includeIf.gitdir:/.path", "/tmp/x.config"),
    ] {
        let r = Repo::new();
        r.write("a.org", &long_note("a"));
        r.commit(T);
        r.git(&["tag", "v1"]);
        r.git(&["config", key, value]);
        let err = plan_err(r.path());
        assert!(err.contains("includes other files"), "{key}: {err}");
        assert!(err.contains("[include"), "{key}: names the section: {err}");
    }
}

/// S-L2: with `GIT_GRAFT_FILE` pinned, git prints a deprecation hint into
/// every error; a refusal must carry git's error, not its advice. Kills:
/// dropping `advice.graftFileDeprecated=false`.
#[test]
fn a_surfaced_git_error_carries_no_advice() {
    let r = Repo::new();
    r.write("a.org", &long_note("a"));
    let c1 = r.commit(T + 100);
    r.write("b.org", &long_note("b"));
    r.commit(T + 200);
    r.git(&["tag", "v1"]);
    std::fs::remove_file(loose(&r.path().join(".git/objects"), &c1)).unwrap();
    let err = plan_err(r.path());
    assert!(!err.is_empty());
}
