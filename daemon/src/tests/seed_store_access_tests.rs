//! The operator commands that open the collab store (`kb seed` apply, `kb
//! export`, `restore`) and the serving daemon exclude each other, and the CLI
//! applies and exports end to end. Every path is inside a temp dir
//! (`isolated`), and the git repository is synthetic and built here.

use std::path::Path;
use std::process::Command;

use super::isolated;
use crate::seed_apply_cli::open_exclusive;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// The daemon holds the store lock once its doc store is open, and an
/// operator command is then refused; while an operator holds it, the daemon
/// does not open the store. Kills: the serve path not taking the lock, and
/// `open_exclusive` not taking it.
#[tokio::test]
async fn the_daemon_and_an_operator_command_exclude_each_other() {
    let tmp = tempfile::tempdir().unwrap();
    let config = isolated(tmp.path());
    let held = open_exclusive(&config).unwrap();
    assert!(
        crate::init_doc_store(&config).await.is_err(),
        "the daemon must not open a store an operator command holds"
    );
    drop(held);
    assert!(matches!(crate::init_doc_store(&config).await, Ok(Some(_))));
    let err = open_exclusive(&config)
        .err()
        .expect("the daemon holds the store now");
    assert!(err.contains("in use"), "{err}");
}

/// A daemon from before the lock does not take it, so the operator side also
/// refuses when something answers on the control socket. Kills: dropping the
/// socket check.
#[tokio::test]
async fn an_answering_control_socket_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let config = isolated(tmp.path());
    let _listener = std::os::unix::net::UnixListener::bind(&config.socket).unwrap();
    let err = open_exclusive(&config).err().expect("a daemon answers");
    assert!(err.contains("answering"), "{err}");
}

/// `restore` refuses a store another process holds (its doc comment always
/// said it would; nothing checked), and restores the same artifact once the
/// store is free. Kills: `restore` opening the store without the lock.
#[tokio::test]
async fn restore_refuses_a_held_store() {
    use mae_daemon::doc_editors::Attribution;
    let tmp = tempfile::tempdir().unwrap();
    let config = isolated(tmp.path());
    let source = mae_daemon::doc_store::DocStore::new(
        std::sync::Arc::new(mae_daemon::storage::SqliteBackend::open_memory().unwrap()),
        500,
    );
    let mut coll = mae_sync::kb::KbCollectionDoc::new_owned("cp", "SHA256:o", "o");
    let _ = coll.add_node("n", "N");
    let by = Attribution::Principal("SHA256:o");
    source
        .share_doc("kbc:cp", &coll.encode_state(), by)
        .await
        .unwrap();
    let node = mae_sync::kb::KbNodeDoc::new("n", "N", "body", &[]);
    let name = mae_sync::kb_node_doc_name("cp", "n");
    source.share_doc(&name, &node.encode(), by).await.unwrap();
    let artifact = tmp.path().join("cp.bin");
    std::fs::write(
        &artifact,
        mae_daemon::checkpoint::export_kb(&source, "cp")
            .await
            .unwrap(),
    )
    .unwrap();
    let restore = args(&[artifact.to_str().unwrap()]);
    let held = open_exclusive(&config).unwrap();
    assert_eq!(
        crate::checkpoint_cli::run_restore(&config, &restore).await,
        1
    );
    drop(held);
    assert_eq!(
        crate::checkpoint_cli::run_restore(&config, &restore).await,
        0
    );
}

fn git(dir: &Path, a: &[&str]) -> String {
    let out = Command::new("git")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", "1600000000 +0000")
        .env("GIT_COMMITTER_DATE", "1600000000 +0000")
        .args(["-c", "user.name=t", "-c", "user.email=t@test.invalid"])
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .arg("-C")
        .arg(dir)
        .args(a)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {a:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

const PAGE: &str = ":PROPERTIES:\n:ID: 0b000000-0000-4000-8000-000000000001\n:Zed: z\n:Alpha: a\n:END:\n#+title: Page\nBody.\n";

/// The CLI: apply with the reviewed hash, re-run (no-op), refuse a wrong hash,
/// then export the file back byte for byte.
#[tokio::test]
async fn the_cli_applies_reruns_and_exports() {
    let tmp = tempfile::tempdir().unwrap();
    let config = isolated(tmp.path());
    let id_dir = config.collab.auth.identity_dir().unwrap();
    mae_mcp::identity::Identity::load_or_generate(&id_dir, "daemon").unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("p")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("p/page.org"), PAGE).unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "c"]);
    git(&repo, &["tag", "v1"]);
    let opts = crate::seed_cli::plan_options_for_test(&repo, "v1");
    let hash = mae_daemon::seed::plan(&opts).unwrap().plan_hash().unwrap();
    let r = repo.to_str().unwrap();
    let seed = |h: &str| {
        args(&[
            "seed",
            "--git",
            r,
            "--rev",
            "v1",
            "--kb-id",
            "k",
            "--plan-hash",
            h,
        ])
    };
    assert_eq!(
        crate::seed_cli::run_kb(&config, &seed(&"0".repeat(64))).await,
        1
    );
    assert_eq!(crate::seed_cli::run_kb(&config, &seed(&hash)).await, 0);
    assert_eq!(crate::seed_cli::run_kb(&config, &seed(&hash)).await, 0);
    let out = tmp.path().join("out");
    let export = args(&["export", "--kb-id", "k", "--out", out.to_str().unwrap()]);
    assert_eq!(crate::seed_cli::run_kb(&config, &export).await, 0);
    assert_eq!(
        std::fs::read_to_string(out.join("p/page.org")).unwrap(),
        PAGE
    );
    assert_eq!(
        crate::seed_cli::run_kb(&config, &export).await,
        1,
        "now non-empty"
    );
}
