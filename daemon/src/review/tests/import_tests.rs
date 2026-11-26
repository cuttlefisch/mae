//! ADR-113 E4 and B6: `kb import` of signed history, from a real directory
//! (org sources, PDFs, an exact SHA256SUMS) into a real store, read back
//! through the review reads and the signed chain. Each test names the guard
//! it kills.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::act_tests::{policy, requested, status};
use super::attach_tests::with_blobs;
use super::*;
use crate::review::import::{self, ApplyInput, ImportError};

fn page(id: &str, body: &str) -> Vec<u8> {
    format!(":PROPERTIES:\n:ID: {id}\n:END:\n#+title: Procedure {id}\n{body}\n").into_bytes()
}

fn pdf(tag: &str) -> Vec<u8> {
    format!("%PDF-1.7\n% {tag}\n%%EOF\n").into_bytes()
}

fn sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

/// An import directory: `import.json`, the files, and SHA256SUMS over them
/// (or `sums` as given).
struct Dir {
    dir: tempfile::TempDir,
}

impl Dir {
    fn new(import: &Value, files: &BTreeMap<&str, Vec<u8>>, sums: Option<String>) -> Dir {
        let dir = tempfile::tempdir().unwrap();
        for (p, b) in files {
            let full = dir.path().join(p);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, b).unwrap();
        }
        std::fs::write(dir.path().join("import.json"), import.to_string()).unwrap();
        let sums = sums.unwrap_or_else(|| {
            files
                .iter()
                .map(|(p, b)| format!("{}  {p}\n", sha(b)))
                .collect()
        });
        std::fs::write(dir.path().join("SHA256SUMS"), sums).unwrap();
        Dir { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }
}

/// Two revisions of n1: Rev 0 superseded by Rev 1, in force, both signed.
fn history() -> (Value, BTreeMap<&'static str, Vec<u8>>) {
    let files = BTreeMap::from([
        ("src/n1-r0.org", page("n1", "The first approved text.")),
        ("src/n1-r1.org", page("n1", "The second approved text.")),
        ("signed/n1-r0.pdf", pdf("signed rev 0")),
        ("signed/n1-r1.pdf", pdf("signed rev 1")),
        ("rendered/n1-r1.pdf", pdf("rendered rev 1")),
    ]);
    let import = json!({"format": "mae-import/1", "kb_id": KB, "pages": [{
    "node_id": "n1", "doc_code": "PRO-1", "revisions": [
        {"rev": 0, "state": "superseded", "source": "src/n1-r0.org",
         "signed_pdf": "signed/n1-r0.pdf", "approved_by_name": "Example Approver",
         "approved_on": "1999-12-20", "effective_at": "2000-01-01T00:00:00.000Z",
         "ended_at": "2001-01-01T00:00:00.000Z"},
        {"rev": 1, "state": "in_force", "source": "src/n1-r1.org",
         "signed_pdf": "signed/n1-r1.pdf", "rendered_pdf": "rendered/n1-r1.pdf",
         "approved_by_name": "Example Approver", "effective_at": "2001-01-01T00:00:00.000Z"}
    ]}]});
    (import, files)
}

const CAP: u64 = 1 << 20;

async fn run(env: &Env, dir: &Path, hash: Option<&str>) -> Result<Value, ImportError> {
    let plan = import::plan(&env.store, dir, KB, CAP).await?;
    let Some(h) = hash else {
        return Ok(import::dry_run_json(&plan));
    };
    let input = ApplyInput {
        import_hash: h,
        os_user: "tester",
        limits: crate::seed::AttachmentLimits::default(),
        quota: None,
    };
    let id = import::apply(&env.store, plan, input).await?;
    Ok(json!({ "ok": true, "event_id": id }))
}

/// Dry run, then apply with its hash.
async fn import_dir(env: &Env, dir: &Path) -> Result<Value, ImportError> {
    let dry = run(env, dir, None).await?;
    run(env, dir, dry["import_hash"].as_str()).await
}

fn kind(r: Result<Value, ImportError>) -> String {
    r.expect_err("a refusal").kind.to_string()
}

/// E4: an import records each revision with its frozen source, signed PDF,
/// attested approver and code, in force or superseded as stated, and one
/// signed `import` event; a second run is refused. Kills: the revisions not
/// written, the attested name lost, the chain not extended, a re-import.
#[tokio::test]
async fn an_import_records_signed_history_once() {
    let env = env().await;
    let _blobs = with_blobs(&env);
    policy(&env, json!({})).await;
    let (import, files) = history();
    let dir = Dir::new(&import, &files, None);
    let before = verified_events(&env).await;
    let out = import_dir(&env, dir.path()).await.unwrap();
    assert!(out["event_id"].is_string(), "{out}");

    let s = status(&env, "n1").await;
    assert_eq!(s["doc_code"], json!("PRO-1"));
    let revs = s["revisions"].as_array().unwrap();
    assert_eq!(revs.len(), 2);
    assert_eq!(
        (revs[0]["state"].clone(), revs[1]["state"].clone()),
        (json!("superseded"), json!("in_force"))
    );
    assert_eq!(revs[1]["replaces"], revs[0]["revision_id"]);
    assert_eq!(
        revs[0]["imported"]["approved_by_name"],
        json!("Example Approver")
    );
    assert_eq!(revs[0]["imported"]["approved_on"], json!("1999-12-20"));
    assert_eq!(
        s["in_force"]["signed_pdf_sha256"],
        json!(sha(&files["signed/n1-r1.pdf"]))
    );
    assert_eq!(s["in_force"]["approval_record"], json!("signed_pdf"));

    let p = json!({"kb_id": KB, "revision": revs[1]["revision_id"]});
    let read = call(&env, &env.owner, "kb/query.get", p).await.unwrap();
    assert!(read["body"]
        .as_str()
        .unwrap()
        .contains("The second approved text."));
    assert_eq!(read["revision"]["attachments"].as_array().unwrap().len(), 2);

    assert_eq!(
        verified_events(&env).await,
        before + 1,
        "one import event, chain verifies"
    );
    let ev = event_objects(&env).await;
    let e = ev.iter().find(|e| e["kind"] == json!("import")).unwrap();
    assert_eq!(e["actor"]["os_user"], json!("tester"));
    assert_eq!(e["payload"]["revisions"].as_array().unwrap().len(), 2);
    assert_eq!(
        e["payload"]["revisions"][1]["approved_by_name"],
        json!("Example Approver")
    );

    assert_eq!(kind(import_dir(&env, dir.path()).await), "import_overlap");
    assert_eq!(
        status(&env, "n1").await["revisions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

/// E4: the manifest is exact, and a refusal writes nothing. Kills each of the
/// manifest checks (hash, a missing line, an unused line, a malformed line, a
/// duplicate, a missing file) on its own.
#[tokio::test]
async fn the_manifest_is_exact_and_a_refusal_writes_nothing() {
    let env = env().await;
    let _blobs = with_blobs(&env);
    policy(&env, json!({})).await;
    let (import, files) = history();
    let good: String = files
        .iter()
        .map(|(p, b)| format!("{}  {p}\n", sha(b)))
        .collect();
    let line = |p: &str| format!("{}  {p}\n", sha(&files[p]));
    let without = |p: &str| good.replace(&line(p), "");
    let cases: Vec<(String, &str)> = vec![
        (
            good.replace(&sha(&files["signed/n1-r0.pdf"]), &"0".repeat(64)),
            "hash",
        ),
        (without("signed/n1-r0.pdf"), "missing_line"),
        (
            format!("{good}{}  extra.pdf\n", "a".repeat(64)),
            "unused_line",
        ),
        (format!("{good}not a line\n"), "malformed_line"),
        (format!("{good}{}", line("src/n1-r0.org")), "duplicate"),
        (
            format!("{good}{}  ../outside.pdf\n", "a".repeat(64)),
            "malformed_line",
        ),
    ];
    for (sums, reason) in cases {
        let dir = Dir::new(&import, &files, Some(sums));
        let e = run(&env, dir.path(), None).await.unwrap_err();
        assert_eq!(
            (e.kind, e.data["reason"].clone()),
            ("manifest_mismatch", json!(reason)),
            "{e}"
        );
    }
    let mut gone = files.clone();
    gone.remove("rendered/n1-r1.pdf");
    let dir = Dir::new(&import, &gone, Some(good));
    let e = run(&env, dir.path(), None).await.unwrap_err();
    assert_eq!(e.data["reason"], json!("missing_file"));
    assert_eq!(
        status(&env, "n1").await["revisions"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

/// E4's content rules: every PDF a PDF, every page a node of the KB whose
/// source names it, revisions in order with one in force and it last, an
/// attested name. Kills each check.
#[tokio::test]
async fn an_import_with_a_wrong_file_or_order_is_refused() {
    let env = env().await;
    let _blobs = with_blobs(&env);
    let (import, files) = history();
    let mut not_pdf = files.clone();
    not_pdf.insert("signed/n1-r0.pdf", b"a scan saved as text".to_vec());
    let dir = Dir::new(&import, &not_pdf, None);
    assert_eq!(kind(run(&env, dir.path(), None).await), "attachment_type");

    let edit = |f: &dyn Fn(&mut Value)| {
        let mut i = import.clone();
        f(&mut i);
        i
    };
    for (bad, want) in [
        (
            edit(&|i| i["pages"][0]["node_id"] = json!("absent")),
            "node_not_in_kb",
        ),
        (
            edit(&|i| i["pages"][0]["revisions"][0]["state"] = json!("in_force")),
            "invalid_params",
        ),
        (
            edit(&|i| i["pages"][0]["revisions"][1]["rev"] = json!(0)),
            "invalid_params",
        ),
        // In force but not last, and no ended_at: only the order rule refuses it.
        (
            edit(&|i| {
                let r0 = &mut i["pages"][0]["revisions"][0];
                r0["state"] = json!("in_force");
                r0.as_object_mut().unwrap().remove("ended_at");
            }),
            "invalid_params",
        ),
        (
            edit(&|i| i["pages"][0]["revisions"][0]["approved_by_name"] = json!("  ")),
            "invalid_params",
        ),
        (
            edit(&|i| {
                i["pages"][0]["revisions"][1]["effective_at"] = json!("2999-01-01T00:00:00.000Z")
            }),
            "invalid_params",
        ),
        (
            edit(&|i| {
                i["pages"][0]["revisions"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("ended_at");
            }),
            "invalid_params",
        ),
        (edit(&|i| i["kb_id"] = json!("another")), "invalid_params"),
    ] {
        let dir = Dir::new(&bad, &files, None);
        assert_eq!(kind(run(&env, dir.path(), None).await), want, "{bad}");
    }
    let mut other_id = files.clone();
    other_id.insert("src/n1-r0.org", page("n2", "Another node's text."));
    let dir = Dir::new(&import, &other_id, None);
    let e = run(&env, dir.path(), None).await.unwrap_err();
    assert_eq!(
        (e.kind, e.data["reason"].clone()),
        ("invalid_params", json!("the :ID: is another node's"))
    );
}

/// B6: an import fills the past but never lands beside a revision a node
/// holds in review; a code another node holds is refused; the apply needs the
/// dry run's hash. Kills the overlap rule, the code check, the hash check.
#[tokio::test]
async fn an_import_never_overlaps_or_takes_another_nodes_code() {
    let env = env().await;
    let _blobs = with_blobs(&env);
    let (import, files) = history();
    let dir = Dir::new(&import, &files, None);
    assert_eq!(
        kind(run(&env, dir.path(), Some(&"0".repeat(64))).await),
        "import_hash_mismatch"
    );

    policy(&env, json!({})).await;
    requested(&env, &["n1"]).await;
    let e = run(&env, dir.path(), None).await.unwrap_err();
    assert_eq!(e.kind, "import_overlap");
    assert!(e.data["latest_existing"].is_string());

    let mut on_n2 = import.clone();
    on_n2["pages"][0]["node_id"] = json!("n2");
    let mut f2 = files.clone();
    f2.insert("src/n1-r0.org", page("n2", "Rev 0 of n2."));
    f2.insert("src/n1-r1.org", page("n2", "Rev 1 of n2."));
    let first = Dir::new(&on_n2, &f2, None);
    import_dir(&env, first.path()).await.unwrap();
    let mut on_n3 = on_n2.clone();
    on_n3["pages"][0]["node_id"] = json!("n3");
    let mut f3 = files.clone();
    f3.insert("src/n1-r0.org", page("n3", "Rev 0 of n3."));
    f3.insert("src/n1-r1.org", page("n3", "Rev 1 of n3."));
    let second = Dir::new(&on_n3, &f3, None);
    assert_eq!(kind(run(&env, second.path(), None).await), "doc_code_taken");
}

/// Principle #16: no network method reaches the import. Kills a method that
/// exposes it.
#[test]
fn no_network_method_reaches_the_import() {
    for m in crate::kb_query::METHODS
        .iter()
        .chain(crate::review::METHODS)
    {
        assert!(!m.contains("import"), "{m}");
    }
}

fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

/// E12 for the CLI: the dry run, the apply and the refusals carry exactly the
/// fields `cli_import.json` pins. Kills a drift between the fixture and the
/// command's `--json` output.
#[tokio::test]
async fn the_import_answers_in_its_fixtures_shape() {
    let path = format!(
        "{}/../docs/verification/fixtures/review-api/cli_import.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let f: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let env = env().await;
    let _blobs = with_blobs(&env);
    let (import, files) = history();
    let dir = Dir::new(&import, &files, None);
    let dry = run(&env, dir.path(), None).await.unwrap();
    assert_eq!(keys(&dry), keys(&f["dry_run_output"]));
    let hash = dry["import_hash"].as_str().unwrap().to_string();
    let applied = run(&env, dir.path(), Some(&hash)).await.unwrap();
    let mut applied = applied;
    applied["import_hash"] = json!(hash);
    assert_eq!(keys(&applied), keys(&f["apply_output"]));

    let refusal = |case: &str| {
        f["refusals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["case"].as_str().unwrap().starts_with(case))
            .unwrap()["output"]
            .clone()
    };
    let good: String = files
        .iter()
        .map(|(p, b)| format!("{}  {p}\n", sha(b)))
        .collect();
    let tampered = good.replace(&sha(&files["signed/n1-r0.pdf"]), &"0".repeat(64));
    let d = Dir::new(&import, &files, Some(tampered));
    let e = run(&env, d.path(), None).await.unwrap_err().to_json();
    let want = refusal("a PDF's bytes do not match");
    assert_eq!(keys(&e["error"]), keys(&want["error"]));
    let d = Dir::new(
        &import,
        &files,
        Some(format!("{good}{}  extra.pdf\n", "a".repeat(64))),
    );
    let e = run(&env, d.path(), None).await.unwrap_err().to_json();
    assert_eq!(
        keys(&e["error"]),
        keys(&refusal("a manifest line names a file no revision uses")["error"])
    );
    let e = run(&env, dir.path(), None).await.unwrap_err().to_json();
    assert_eq!(
        keys(&e["error"]),
        keys(&refusal("the node already holds")["error"])
    );
}

/// All or nothing under a race: a review request lands on the node between
/// the plan and the write. The write re-checks inside its transaction and
/// refuses, writing nothing. Kills an apply that trusts its plan's checks.
#[tokio::test]
async fn the_apply_rechecks_inside_its_transaction() {
    let env = env().await;
    let _blobs = with_blobs(&env);
    policy(&env, json!({})).await;
    let (import, files) = history();
    let dir = Dir::new(&import, &files, None);
    let plan = import::plan(&env.store, dir.path(), KB, CAP).await.unwrap();
    let hash = plan.checked.import_hash.clone();
    requested(&env, &["n1"]).await;
    let input = ApplyInput {
        import_hash: &hash,
        os_user: "tester",
        limits: crate::seed::AttachmentLimits::default(),
        quota: None,
    };
    let e = import::apply(&env.store, plan, input).await.unwrap_err();
    assert_eq!(e.kind, "import_overlap");
    let revs = status(&env, "n1").await["revisions"].clone();
    assert_eq!(
        revs.as_array().unwrap().len(),
        1,
        "only the request's own revision"
    );
}

/// One page, all superseded: the past only.
fn past_only(
    node: &str,
    rev: i64,
    from: &str,
    to: &str,
) -> (Value, BTreeMap<&'static str, Vec<u8>>) {
    let files = BTreeMap::from([
        ("src/p.org", page(node, "A past text.")),
        ("signed/p.pdf", pdf(&format!("signed {node} {rev} {from}"))),
    ]);
    let import = json!({"format": "mae-import/1", "kb_id": KB, "pages": [{
        "node_id": node, "revisions": [{"rev": rev, "state": "superseded",
            "source": "src/p.org", "signed_pdf": "signed/p.pdf",
            "approved_by_name": "Example Approver", "effective_at": from, "ended_at": to}]}]});
    (import, files)
}

/// Review findings: an import never lands inside an ended revision's window,
/// never reuses or goes back on its revision numbers, and two pages of one
/// import never claim one code. Kills the overlap rule reading only starts,
/// the missing rev check, and the in-import code check.
#[tokio::test]
async fn an_import_respects_ended_windows_numbers_and_one_code_per_page() {
    let env = env().await;
    let _blobs = with_blobs(&env);
    let (i0, f0) = past_only(
        "n1",
        0,
        "2000-01-01T00:00:00.000Z",
        "2005-01-01T00:00:00.000Z",
    );
    import_dir(&env, Dir::new(&i0, &f0, None).path())
        .await
        .unwrap();

    let (inside, f1) = past_only(
        "n1",
        1,
        "2003-01-01T00:00:00.000Z",
        "2004-01-01T00:00:00.000Z",
    );
    let e = run(&env, Dir::new(&inside, &f1, None).path(), None)
        .await
        .unwrap_err();
    assert_eq!(e.kind, "import_overlap", "{e}");
    let (renumbered, f2) = past_only(
        "n1",
        0,
        "2006-01-01T00:00:00.000Z",
        "2007-01-01T00:00:00.000Z",
    );
    let e = run(&env, Dir::new(&renumbered, &f2, None).path(), None)
        .await
        .unwrap_err();
    assert_eq!(
        (e.kind, e.data["param"].clone()),
        ("invalid_params", json!("rev"))
    );

    let files = BTreeMap::from([
        ("src/a.org", page("n2", "A.")),
        ("src/b.org", page("n3", "B.")),
        ("signed/a.pdf", pdf("a")),
        ("signed/b.pdf", pdf("b")),
    ]);
    let one = |node: &str, src: &str, pdf: &str| {
        json!({"node_id": node, "doc_code": "PRO-9",
        "revisions": [{"rev": 0, "state": "in_force", "source": src, "signed_pdf": pdf,
            "approved_by_name": "Example Approver", "effective_at": "2000-01-01T00:00:00.000Z"}]})
    };
    let both = json!({"format": "mae-import/1", "kb_id": KB, "pages": [
        one("n2", "src/a.org", "signed/a.pdf"), one("n3", "src/b.org", "signed/b.pdf")]});
    let e = run(&env, Dir::new(&both, &files, None).path(), None)
        .await
        .unwrap_err();
    assert_eq!(e.kind, "invalid_params", "{e}");
}

/// Review finding: the imported text is not the live node's, so the next
/// review's author window covers the node's whole record. A person who
/// edited the live node before the import is an author of the next
/// revision. Kills an author window that starts at the import.
#[tokio::test]
async fn an_import_does_not_start_the_next_reviews_author_window() {
    let env = env().await;
    let _blobs = with_blobs(&env);
    policy(&env, json!({})).await;
    let viewer = env.viewer.clone();
    edit_with(&env.store, KB, "n1", &viewer, |d| {
        d.set_title("The viewer's draft")
    })
    .await;
    let (import, files) = history();
    import_dir(&env, Dir::new(&import, &files, None).path())
        .await
        .unwrap();
    let (_, revs) = requested(&env, &["n1"]).await;
    let p = json!({"kb_id": KB, "revision_id": revs[0]});
    let r = super::act_tests::as_(
        &env,
        &super::act_tests::approver(&env),
        "kb/review.approve",
        p,
    )
    .await;
    assert_eq!(err_kind(r), "author_cannot_approve");
}

/// Review finding: a path with a `.` part, and a file that is not a regular
/// file (a link), are refused. Kills `safe_path` passing `.` and a read that
/// follows links.
#[tokio::test]
async fn only_plain_paths_and_regular_files_are_read() {
    let env = env().await;
    let _blobs = with_blobs(&env);
    let (import, files) = history();
    let good: String = files
        .iter()
        .map(|(p, b)| format!("{}  {p}\n", sha(b)))
        .collect();
    let dotted = good.replace("  src/n1-r0.org", "  src/./n1-r0.org");
    let e = run(&env, Dir::new(&import, &files, Some(dotted)).path(), None)
        .await
        .unwrap_err();
    assert_eq!(e.data["reason"], json!("malformed_line"), "{e}");

    let dir = Dir::new(&import, &files, None);
    let target = dir.path().join("signed/n1-r0.pdf");
    let real = dir.path().join("elsewhere.pdf");
    std::fs::rename(&target, &real).unwrap();
    std::os::unix::fs::symlink(&real, &target).unwrap();
    let e = run(&env, dir.path(), None).await.unwrap_err();
    assert_eq!(
        (e.kind, e.data["reason"].clone()),
        ("manifest_mismatch", json!("missing_file"))
    );
}
