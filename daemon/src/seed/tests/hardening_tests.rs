//! The repository is input, and so is the environment the seeder runs in.
//! None of it may run code, fetch, substitute objects or change an answer.
//! Every repository here is synthetic.

use std::path::Path;

use super::git_tests::{git_in, long_note, Repo};
use super::plan_opts;
use crate::seed::*;

const T: i64 = 1_700_000_000;

fn plan_err(repo: &Path) -> String {
    match plan(&plan_opts(repo, "v1")) {
        Ok(p) => panic!("planned {} nodes; expected a refusal", p.nodes.len()),
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

fn tagged_repo() -> Repo {
    let r = Repo::new();
    r.write("notes/a.org", &long_note("a"));
    r.commit(T);
    r.git(&["tag", "v1"]);
    r
}

/// git starts from an empty environment with the system and global config
/// off, and the overrides that stop code from running are on the command line.
/// Kills: inheriting the caller's environment, reading the operator's config.
#[test]
fn every_git_call_starts_from_a_pinned_environment() {
    let cmd = GitRepo::new("/r").command();
    let envs: Vec<(String, Option<String>)> = cmd
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();
    for (k, v) in [
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_NO_REPLACE_OBJECTS", "1"),
        ("GIT_NO_LAZY_FETCH", "1"),
        ("HOME", "/dev/null"),
        ("GIT_GRAFT_FILE", "/dev/null"),
    ] {
        assert!(
            envs.contains(&(k.to_string(), Some(v.to_string()))),
            "{k}={v} missing"
        );
    }
    for (k, _) in &envs {
        assert!(![
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_CONFIG_PARAMETERS",
            "GIT_INDEX_FILE"
        ]
        .contains(&k.as_str()));
    }
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    for want in [
        "core.fsmonitor=false",
        "protocol.allow=never",
        "core.attributesFile=/dev/null",
        "core.commitGraph=false",
        "core.multiPackIndex=false",
    ] {
        assert!(
            args.iter().any(|a| a == want),
            "-c {want} missing from {args:?}"
        );
    }
    // Behaviourally: git sees no system or global configuration at all.
    let r = tagged_repo();
    let out = GitRepo::new(r.path())
        .command()
        .args(["config", "--list", "--show-scope"])
        .output()
        .unwrap();
    let listing = String::from_utf8_lossy(&out.stdout);
    assert!(
        !listing
            .lines()
            .any(|l| l.starts_with("global") || l.starts_with("system")),
        "{listing}"
    );
}

/// A repository's `core.fsmonitor` names a command; git runs it when it looks
/// at the work tree (confirmed for `check-attr`). Kills: dropping
/// `-c core.fsmonitor=false`.
#[cfg(unix)]
#[test]
fn a_repository_fsmonitor_command_is_never_run() {
    use std::os::unix::fs::PermissionsExt;
    let r = tagged_repo();
    let marker = r.path().join("fsmonitor-ran");
    let script = r.path().join(".git/fsmonitor.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    r.git(&["config", "core.fsmonitor", &script.display().to_string()]);
    let plan = r.plan("v1");
    assert!(plan.ok(), "{:?}", plan.refusals);
    assert!(!marker.exists(), "the repository's fsmonitor command ran");
}

/// `git replace` makes one object stand in for another. Kills: dropping
/// `--no-replace-objects` / `GIT_NO_REPLACE_OBJECTS`.
#[test]
fn replace_refs_do_not_substitute_a_page() {
    let r = tagged_repo();
    let real = r.git(&["rev-parse", "v1:notes/a.org"]);
    std::fs::write(r.path().join("forged.org"), long_note("forged")).unwrap();
    let forged = r.git(&["hash-object", "-w", "forged.org"]);
    std::fs::remove_file(r.path().join("forged.org")).unwrap();
    r.git(&["replace", &real, &forged]);
    let plan = r.plan("v1");
    assert_eq!(plan.nodes[0].file.id, "a", "the replaced blob was read");
}

/// A shallow clone has no history before its cut, so its dates are wrong.
#[test]
fn a_shallow_clone_is_refused() {
    let src = tagged_repo();
    let dst = tempfile::tempdir().unwrap();
    let url = format!("file://{}", src.path().display());
    git_in(
        dst.path(),
        &["clone", "-q", "--depth", "1", "--branch", "v1", &url, "c"],
        T,
    );
    assert!(plan_err(&dst.path().join("c")).contains("shallow"));
}

/// A partial clone fetches missing blobs from its remote on demand (confirmed
/// over the network). Kills: dropping the promisor check.
#[test]
fn a_partial_clone_is_refused_before_anything_is_read() {
    let src = tagged_repo();
    let dst = tempfile::tempdir().unwrap();
    let url = format!("file://{}", src.path().display());
    git_in(
        dst.path(),
        &[
            "clone",
            "-q",
            "--filter=blob:none",
            "--no-checkout",
            &url,
            "c",
        ],
        T,
    );
    assert!(plan_err(&dst.path().join("c")).contains("partial clone"));
}

/// A bare repository is a valid source, named by its own directory.
#[test]
fn a_bare_clone_plans() {
    let src = tagged_repo();
    let dst = tempfile::tempdir().unwrap();
    let url = format!("file://{}", src.path().display());
    git_in(dst.path(), &["clone", "-q", "--bare", &url, "b.git"], T);
    let plan = plan(&plan_opts(&dst.path().join("b.git"), "v1")).unwrap();
    assert!(plan.ok(), "{:?}", plan.refusals);
    assert_eq!(plan.nodes.len(), 1);
}

/// Attribute checks are relative to the path given (confirmed broken for a
/// subdirectory). Kills: dropping the top-level check.
#[test]
fn a_subdirectory_of_the_repository_is_refused() {
    let r = tagged_repo();
    assert!(plan_err(&r.path().join("notes")).contains("top level"));
}

/// `.git/info/attributes` changes attributes for every path and no flag turns
/// it off (confirmed it changed results); `info/grafts` rewrites parents.
#[test]
fn local_attributes_and_grafts_are_refused() {
    let r = tagged_repo();
    std::fs::write(r.path().join(".git/info/attributes"), "").unwrap();
    assert!(r.plan("v1").ok(), "an empty file changes nothing");
    std::fs::write(r.path().join(".git/info/attributes"), "# only a comment\n").unwrap();
    assert!(
        plan_err(r.path()).contains("info/attributes"),
        "any byte refuses"
    );
    std::fs::write(r.path().join(".git/info/attributes"), "*.org eol=crlf\n").unwrap();
    assert!(plan_err(r.path()).contains("info/attributes"));
    std::fs::remove_file(r.path().join(".git/info/attributes")).unwrap();
    let head = r.git(&["rev-parse", "HEAD"]);
    std::fs::write(r.path().join(".git/info/grafts"), format!("{head}\n")).unwrap();
    assert!(plan_err(r.path()).contains("info/grafts"));
}
