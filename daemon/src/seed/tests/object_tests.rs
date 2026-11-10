//! The second review's findings: the repository's caches and object store,
//! and the environment's `HOME`, are input too. Every repository is synthetic.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;

use super::git_tests::{git_in, git_in_os, long_note, note, Repo};
use super::plan_opts;
use crate::seed::*;

const T: i64 = 1_700_000_000;

fn plan_err(repo: &Path) -> String {
    match plan(&plan_opts(repo, "v1")) {
        Ok(p) => panic!("planned {} nodes; expected a refusal", p.nodes.len()),
        Err(e) => e.0,
    }
}

/// Two commits: `a.org` at T+100, `b.org` added at T+200.
fn two_commits() -> (Repo, String, String) {
    let r = Repo::new();
    r.write("a.org", &long_note("a"));
    let c1 = r.commit(T + 100);
    r.write("b.org", &long_note("b"));
    let c2 = r.commit(T + 200);
    r.git(&["tag", "v1"]);
    (r, c1, c2)
}

/// S-N1: a grafts file that is not UTF-8 passed a text check and git applied
/// it. Kills: reading the file as text.
#[test]
fn a_grafts_file_that_is_not_utf8_is_refused() {
    let (r, _, c2) = two_commits();
    let mut bytes = b"\xff\xfe\n".to_vec();
    bytes.extend(format!("{c2}\n").into_bytes());
    std::fs::write(r.path().join(".git/info/grafts"), bytes).unwrap();
    assert!(plan_err(r.path()).contains("info/grafts"));
}

/// S-N1, belt and braces: the pinned environment points git at no grafts
/// file at all. Kills: dropping `GIT_GRAFT_FILE`.
#[test]
fn the_pinned_environment_applies_no_grafts() {
    let (r, c1, c2) = two_commits();
    std::fs::write(r.path().join(".git/info/grafts"), format!("{c2}\n")).unwrap();
    let plain = git_in(r.path(), &["log", "-1", "--format=%P", &c2], T);
    assert_eq!(
        plain, "",
        "the graft is live for plain git (else this proves nothing)"
    );
    let out = GitRepo::new(r.path())
        .command()
        .args(["log", "-1", "--format=%P", &c2])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), c1);
}

/// Make `commit` a root in the repository's commit-graph file, as a forger
/// would: its first parent becomes "none" (`0x70000000`).
fn forge_commit_graph_root(git_dir: &Path, commit: &str) {
    let path = git_dir.join("objects/info/commit-graph");
    let mut b = std::fs::read(&path).unwrap();
    assert_eq!(&b[0..4], b"CGPH");
    let hash_len = if b[5] == 1 { 20 } else { 32 };
    let mut chunk: HashMap<[u8; 4], usize> = HashMap::new();
    for k in 0..b[6] as usize {
        let e = 8 + 12 * k;
        let off = u64::from_be_bytes(b[e + 4..e + 12].try_into().unwrap()) as usize;
        chunk.insert(b[e..e + 4].try_into().unwrap(), off);
    }
    let (oidf, oidl, cdat) = (chunk[b"OIDF"], chunk[b"OIDL"], chunk[b"CDAT"]);
    let n = u32::from_be_bytes(b[oidf + 255 * 4..oidf + 256 * 4].try_into().unwrap()) as usize;
    let want = hex::decode(commit).unwrap();
    let i = (0..n)
        .find(|i| b[oidl + i * hash_len..oidl + (i + 1) * hash_len] == want[..])
        .unwrap();
    let parent1 = cdat + i * (hash_len + 16) + hash_len;
    b[parent1..parent1 + 4].copy_from_slice(&0x7000_0000u32.to_be_bytes());
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&path, perms).unwrap();
    std::fs::write(&path, b).unwrap();
}

/// S-N2: a forged commit-graph rewrote parents. Kills: dropping
/// `-c core.commitGraph=false` (`a.org` would be dated by the forged root).
#[test]
fn a_forged_commit_graph_does_not_change_the_history() {
    let (r, _, c2) = two_commits();
    r.git(&["commit-graph", "write", "--reachable"]);
    forge_commit_graph_root(&r.path().join(".git"), &c2);
    let seen = git_in(r.path(), &["log", "--format=%H", "v1"], T);
    assert_eq!(
        seen.lines().count(),
        1,
        "the forgery is live for plain git (else this proves nothing)"
    );
    let plan = r.plan("v1");
    assert!(plan.ok(), "{:?}", plan.refusals);
    let a = plan.nodes.iter().find(|n| n.path == "a.org").unwrap();
    assert_eq!((a.created_at, a.modified_at), (T + 100, T + 100));
}

/// The path of the loose object `sha` in `repo`.
fn loose(repo: &Path, sha: &str) -> std::path::PathBuf {
    repo.join(".git/objects").join(&sha[..2]).join(&sha[2..])
}

/// S-N3: git does not check a loose object against its name, so a forged
/// file under a real sha was read as that blob. Kills: dropping the hash
/// check on selected blobs.
#[test]
fn a_selected_blob_that_does_not_hash_to_its_name_refuses_the_plan() {
    let r = Repo::new();
    r.write("a.org", &long_note("a"));
    r.commit(T);
    r.git(&["tag", "v1"]);
    let real = r.git(&["rev-parse", "v1:a.org"]);
    std::fs::write(r.path().join("forged.org"), note("a", "contenido falso\n")).unwrap();
    let forged = r.git(&["hash-object", "-w", "forged.org"]);
    std::fs::remove_file(r.path().join("forged.org")).unwrap();
    let target = loose(r.path(), &real);
    let mut perms = std::fs::metadata(&target).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&target, perms).unwrap();
    std::fs::copy(loose(r.path(), &forged), &target).unwrap();
    let shown = git_in(r.path(), &["cat-file", "-p", &real], T);
    assert!(
        shown.contains("contenido falso"),
        "git reads the forgery (else this proves nothing)"
    );
    assert!(plan_err(r.path()).contains("does not hash to its name"));
}

/// The hash check speaks the repository's object format. Kills: hashing every
/// repository with sha1.
#[test]
fn a_sha256_repository_plans() {
    let dir = tempfile::tempdir().unwrap();
    git_in(
        dir.path(),
        &["init", "-q", "--object-format=sha256", "-b", "main"],
        T,
    );
    std::fs::write(dir.path().join("a.org"), long_note("a")).unwrap();
    git_in(dir.path(), &["add", "-A"], T);
    git_in(dir.path(), &["commit", "-q", "-m", "c"], T);
    git_in(dir.path(), &["tag", "v1"], T);
    let plan = plan(&plan_opts(dir.path(), "v1")).unwrap();
    assert!(plan.ok(), "{:?}", plan.refusals);
    assert_eq!(plan.nodes[0].file.id, "a");
}

/// S-N4: a historical path that is not UTF-8 (`a\xff.org`) decoded lossily to
/// `a\u{FFFD}.org` and was counted as a change to the real page of that name.
/// Kills: comparing decoded paths.
#[cfg(unix)]
#[test]
fn a_non_utf8_historical_path_is_not_a_change_to_a_real_one() {
    use std::os::unix::ffi::OsStrExt;
    let r = Repo::new();
    r.write("a\u{FFFD}.org", &long_note("real"));
    r.commit(T + 100);
    std::fs::write(r.path().join("x.org"), long_note("other")).unwrap();
    let blob = r.git(&["hash-object", "-w", "x.org"]);
    std::fs::remove_file(r.path().join("x.org")).unwrap();
    let raw = OsStr::from_bytes(b"a\xff.org");
    let info = format!("100644,{blob},");
    let mut cacheinfo = info.into_bytes();
    cacheinfo.extend_from_slice(b"a\xff.org");
    let cacheinfo = OsStr::from_bytes(&cacheinfo);
    let run = |args: &[&OsStr], t: i64| assert!(git_in_os(r.path(), args, t).0, "{args:?}");
    run(
        &[
            OsStr::new("update-index"),
            OsStr::new("--add"),
            OsStr::new("--cacheinfo"),
            cacheinfo,
        ],
        T,
    );
    run(
        &[
            OsStr::new("commit"),
            OsStr::new("-q"),
            OsStr::new("-m"),
            OsStr::new("add"),
        ],
        T + 200,
    );
    run(
        &[
            OsStr::new("update-index"),
            OsStr::new("--force-remove"),
            raw,
        ],
        T,
    );
    run(
        &[
            OsStr::new("commit"),
            OsStr::new("-q"),
            OsStr::new("-m"),
            OsStr::new("rm"),
        ],
        T + 300,
    );
    r.git(&["tag", "v1"]);
    let plan = r.plan("v1");
    assert!(plan.ok(), "{:?}", plan.refusals);
    let last: i64 = r
        .git(&["log", "-1", "--format=%ct", "v1", "--", "a\u{FFFD}.org"])
        .parse()
        .unwrap();
    assert_eq!(last, T + 100);
    assert_eq!(plan.nodes[0].modified_at, last);
}

/// S-N5: the history pass reads only `*.org` diff entries, however many
/// other files change. Kills: dropping the `*.org` pathspec.
#[test]
fn the_history_pass_reads_only_org_entries() {
    let r = Repo::new();
    r.write("a.org", &long_note("a"));
    for i in 0..30 {
        r.write(&format!("assets/f{i}.txt"), "x\n");
    }
    r.commit(T + 100);
    for i in 0..30 {
        r.write(&format!("assets/f{i}.txt"), "y\n");
    }
    r.commit(T + 200);
    r.git(&["tag", "v1"]);
    let plan = r.plan("v1");
    assert!(plan.ok(), "{:?}", plan.refusals);
    assert_eq!(plan.history_diff_entries, 1, "only the one .org add");
}

/// Note 6: `HOME` is pinned, so a repository's `include.path=~/…` cannot pull
/// the operator's own configuration back in. On a machine with a
/// `~/.gitconfig` this fails if `HOME` is inherited. Kills: inheriting `HOME`.
#[test]
fn a_home_relative_include_in_the_repository_is_inert() {
    let r = Repo::new();
    r.git(&["config", "include.path", "~/.gitconfig"]);
    let out = GitRepo::new(r.path())
        .command()
        .args(["config", "--list", "--show-origin"])
        .output()
        .unwrap();
    let listing = String::from_utf8_lossy(&out.stdout);
    let leaked = listing
        .lines()
        .filter(|l| !l.starts_with("file:.git/config") && !l.starts_with("command line:"))
        .count();
    assert_eq!(
        leaked, 0,
        "{leaked} config lines came from outside the repository"
    );
}
