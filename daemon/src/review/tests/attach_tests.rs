//! ADR-113 PR-6: a revision's attachments through the methods a client calls,
//! with a real attachment store holding real uploads. Each test names the
//! guard it kills.

use std::sync::Arc;

use serde_json::{json, Value};

use super::act_tests::{approver, as_, policy, requested, signed_in, status, NOW};
use super::fixture_shape_tests::{assert_shape, example_result, fixture, refusal};
use super::*;
use crate::blob_store::{process::ProcessLimits, BlobStore, MediaType};
use crate::oauth_self_issue::PrincipalKind;

/// The env with an attachment store, as the listener opens one.
fn with_blobs(env: &Env) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    env.store.set_blob_store(Arc::new(
        BlobStore::open(&dir.path().join("blobs")).unwrap(),
    ));
    dir
}

/// Upload `bytes` to the KB as the blob route does (process, stage, commit),
/// on a blocking worker as the route runs it.
async fn put(env: &Env, bytes: &[u8]) -> String {
    let blobs = Arc::clone(env.store.blob_store().unwrap());
    let bytes = bytes.to_vec();
    tokio::task::spawn_blocking(move || {
        let limits = ProcessLimits {
            max_pixels: 1 << 24,
            max_frames: 8,
        };
        let p = crate::blob_store::process::process(bytes, &limits).unwrap();
        let staged = blobs.stage(&p.bytes).unwrap();
        let meta = blobs
            .commit(KB, staged, (p.media_type, p.width, p.height), None)
            .unwrap();
        meta.sha256
    })
    .await
    .unwrap()
}

/// A row the upload path would never write: `bytes` stored under `media`.
async fn forge(env: &Env, bytes: &[u8], media: MediaType) -> String {
    let blobs = Arc::clone(env.store.blob_store().unwrap());
    let bytes = bytes.to_vec();
    tokio::task::spawn_blocking(move || {
        let staged = blobs.stage(&bytes).unwrap();
        blobs
            .commit(KB, staged, (media, None, None), None)
            .unwrap()
            .sha256
    })
    .await
    .unwrap()
}

fn pdf(tag: &str) -> Vec<u8> {
    format!("%PDF-1.7\n% {tag}\n%%EOF\n").into_bytes()
}

async fn rev_source(env: &Env, revision: &str) -> String {
    let p = json!({"kb_id": KB, "revision": revision});
    let r = call(env, &env.owner, "kb/query.get", p).await.unwrap();
    // The frozen source's sha256 is the revision's; read it from the record.
    let s = call(
        env,
        &env.owner,
        "kb/review.status",
        json!({"kb_id": KB, "node_id": r["node_id"]}),
    )
    .await
    .unwrap();
    s["revisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["revision_id"] == json!(revision))
        .unwrap()["source_sha256"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn attach_rendered(
    env: &Env,
    who: &str,
    rev: &str,
    blob: &str,
    source: &str,
) -> Result<Value, McpError> {
    let p = json!({"kb_id": KB, "revision_id": rev, "blob_sha256": blob, "source_sha256": source});
    call(env, who, "kb/review.attach_rendered_pdf", p).await
}

async fn attach_signed(
    env: &Env,
    who: &ValidatedPrincipal,
    rev: &str,
    blob: &str,
) -> Result<Value, McpError> {
    let p = json!({"kb_id": KB, "revision_id": rev, "blob_sha256": blob});
    as_(env, who, "kb/review.attach_signed_pdf", p).await
}

async fn attachments(env: &Env, rev: &str) -> Value {
    let p = json!({"kb_id": KB, "revision": rev});
    let r = call(env, &env.owner, "kb/query.get", p).await.unwrap();
    r["revision"]["attachments"].clone()
}

/// B1, B2, D6, O13 on the rendered PDF. Kills: anyone but the requester
/// attaching; a declared source not checked; the type check (media and the
/// `%PDF-` head, each alone); write-once; the same-bytes retry.
#[tokio::test]
async fn a_rendered_pdf_is_written_once_by_the_requester_from_the_frozen_source() {
    let env = env().await;
    let _dir = with_blobs(&env);
    policy(&env, json!({})).await;
    let (_, revs) = requested(&env, &["n1", "n2"]).await;
    let (src, other) = (
        rev_source(&env, &revs[0]).await,
        rev_source(&env, &revs[1]).await,
    );
    let a = put(&env, &pdf("rendering A")).await;

    let viewer = env.viewer.clone();
    assert_eq!(
        err_kind(attach_rendered(&env, &viewer, &revs[0], &a, &src).await),
        "role_forbidden"
    );
    let e = attach_rendered(&env, &env.editor, &revs[0], &a, &other)
        .await
        .unwrap_err();
    assert_eq!(kind(&e), "source_mismatch");
    assert_eq!(e.data.unwrap()["expected"], json!(src));
    let text = put(&env, b"plain text, not a PDF").await;
    assert_eq!(
        err_kind(attach_rendered(&env, &env.editor, &revs[0], &text, &src).await),
        "attachment_type"
    );
    let posing = forge(&env, b"not a PDF at all, stored as one", MediaType::Pdf).await;
    assert_eq!(
        err_kind(attach_rendered(&env, &env.editor, &revs[0], &posing, &src).await),
        "attachment_type"
    );
    let headed = forge(&env, &pdf("typed wrong"), MediaType::Other).await;
    assert_eq!(
        err_kind(attach_rendered(&env, &env.editor, &revs[0], &headed, &src).await),
        "attachment_type"
    );
    let absent = "ab".repeat(32);
    assert_eq!(
        err_kind(attach_rendered(&env, &env.editor, &revs[0], &absent, &src).await),
        "blob_missing"
    );

    let first = attach_rendered(&env, &env.editor, &revs[0], &a, &src)
        .await
        .unwrap();
    let again = attach_rendered(&env, &env.editor, &revs[0], &a, &src)
        .await
        .unwrap();
    assert_eq!(first, again, "the same bytes answer the first attach");
    let b = put(&env, &pdf("rendering B")).await;
    let e = attach_rendered(&env, &env.editor, &revs[0], &b, &src)
        .await
        .unwrap_err();
    assert_eq!(
        (kind(&e), e.data.unwrap()["current"].clone()),
        ("attachment_exists".into(), json!(a))
    );

    let list = attachments(&env, &revs[0]).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["sha256"], json!(a));
    assert_eq!(list[0]["content_type"], json!("application/pdf"));
    assert_eq!(list[0]["annulled"], Value::Null);
    let ev = event_objects(&env).await;
    let at = ev
        .iter()
        .filter(|e| e["kind"] == json!("attach"))
        .collect::<Vec<_>>();
    assert_eq!(at.len(), 1, "a retry records nothing");
    assert_eq!(at[0]["payload"]["source_sha256"], json!(src));
    assert_eq!(at[0]["actor"]["principal"], json!(env.editor));
}

/// B3: a signed PDF only from a listed approver who is a person, and only
/// once the revision is approved; afterwards in every later state. Kills:
/// the person check, the approver check, a pending state allowed.
#[tokio::test]
async fn a_signed_pdf_needs_a_listed_person_and_an_approved_revision() {
    let env = env().await;
    let _dir = with_blobs(&env);
    policy(&env, json!({})).await;
    let (rq, revs) = requested(&env, &["n1"]).await;
    let s1 = put(&env, &pdf("signed 1")).await;
    let e = attach_signed(&env, &approver(&env), &revs[0], &s1)
        .await
        .unwrap_err();
    assert_eq!(
        (kind(&e), e.data.unwrap()["from"].clone()),
        ("invalid_transition".into(), json!("in_review"))
    );

    let p = json!({"kb_id": KB, "revision_id": revs[0]});
    as_(&env, &approver(&env), "kb/review.approve", p)
        .await
        .unwrap();
    let agent = signed_in(&env.viewer, &["approvers"], PrincipalKind::Agent);
    assert_eq!(
        err_kind(attach_signed(&env, &agent, &revs[0], &s1).await),
        "agent_forbidden"
    );
    let unlisted = signed_in(&env.editor, &[], PrincipalKind::Person);
    assert_eq!(
        err_kind(attach_signed(&env, &unlisted, &revs[0], &s1).await),
        "not_approver"
    );
    attach_signed(&env, &approver(&env), &revs[0], &s1)
        .await
        .unwrap();

    let f = json!({"kb_id": KB, "request_id": rq, "effective_at": NOW});
    as_(&env, &approver(&env), "kb/review.enter_into_force", f)
        .await
        .unwrap();
    let st = status(&env, "n1").await;
    assert_eq!(st["in_force"]["signed_pdf_sha256"], json!(s1));

    // A second revision supersedes the first; the first, superseded, still
    // takes its (first) signed PDF when it had none: here n2's path.
    let (rq2, revs2) = requested(&env, &["n1"]).await;
    let p = json!({"kb_id": KB, "revision_id": revs2[0]});
    as_(&env, &approver(&env), "kb/review.approve", p)
        .await
        .unwrap();
    let f = json!({"kb_id": KB, "request_id": rq2, "effective_at": NOW});
    as_(&env, &approver(&env), "kb/review.enter_into_force", f)
        .await
        .unwrap();
    let d = json!({"kb_id": KB, "node_id": "n1", "effective_at": NOW, "reason": "Retired."});
    as_(&env, &approver(&env), "kb/review.derogate", d)
        .await
        .unwrap();
    let s2 = put(&env, &pdf("signed 2, after derogation")).await;
    attach_signed(&env, &approver(&env), &revs2[0], &s2)
        .await
        .unwrap();
}

/// E3, O19: under `signed_pdf` and `in_force_on_approval`, the set waits for
/// every member's signed PDF, and the attach that gives the last one puts the
/// whole set in force. Kills: `signed_pdf_required` from the record instead
/// of the attachments, and a set put in force before its last PDF.
#[tokio::test]
async fn under_signed_pdf_the_last_signed_pdf_puts_the_set_in_force() {
    let env = env().await;
    let _dir = with_blobs(&env);
    policy(
        &env,
        json!({"approval_record": "signed_pdf", "in_force_on_approval": true,
        "reason": "The signed original is the record."}),
    )
    .await;
    let (rq, revs) = requested(&env, &["n1", "n2"]).await;
    for r in &revs {
        let p = json!({"kb_id": KB, "revision_id": r});
        let a = as_(&env, &approver(&env), "kb/review.approve", p)
            .await
            .unwrap();
        assert_eq!(a["in_force_since"], Value::Null);
    }
    assert_eq!(
        status(&env, "n1").await["approved"]["waiting_for"],
        json!("signed_pdf")
    );
    let f = json!({"kb_id": KB, "request_id": rq, "effective_at": NOW});
    let e = as_(
        &env,
        &approver(&env),
        "kb/review.enter_into_force",
        f.clone(),
    )
    .await
    .unwrap_err();
    assert_eq!(e.data.unwrap()["missing"], json!(revs));

    let a1 = attach_signed(
        &env,
        &approver(&env),
        &revs[0],
        &put(&env, &pdf("s1")).await,
    )
    .await
    .unwrap();
    assert_eq!(a1["in_force_since"], Value::Null);
    let e = as_(&env, &approver(&env), "kb/review.enter_into_force", f)
        .await
        .unwrap_err();
    assert_eq!(e.data.unwrap()["missing"], json!([revs[1]]));
    assert_eq!(status(&env, "n2").await["in_force"], Value::Null);
    let s2 = put(&env, &pdf("s2")).await;
    let a2 = attach_signed(&env, &approver(&env), &revs[1], &s2)
        .await
        .unwrap();
    assert!(a2["in_force_since"].is_string(), "{a2}");
    for n in ["n1", "n2"] {
        assert!(
            status(&env, n).await["in_force"].is_object(),
            "{n} in force"
        );
    }
    assert_eq!(
        status(&env, "n2").await["in_force"]["signed_pdf_sha256"],
        json!(s2)
    );
}

async fn annul(
    env: &Env,
    who: &str,
    rev: &str,
    kind: &str,
    reason: &str,
) -> Result<Value, McpError> {
    let p = json!({"kb_id": KB, "revision_id": rev, "kind": kind, "reason": reason});
    call(env, who, "kb/review.annul_attachment", p).await
}

/// B1 annulment: by the owner or the uploader, with a reason; the bytes stay
/// listed, annulled, and the kind takes a new attach. Kills: annulment by
/// anyone, annulment that removes the row, a new attach still refused.
#[tokio::test]
async fn a_wrong_file_is_annulled_never_replaced() {
    let env = env().await;
    let _dir = with_blobs(&env);
    policy(&env, json!({})).await;
    let (_, revs) = requested(&env, &["n1"]).await;
    let (rev, src) = (revs[0].clone(), rev_source(&env, &revs[0]).await);
    let (a, b) = (
        put(&env, &pdf("wrong template")).await,
        put(&env, &pdf("right")).await,
    );
    attach_rendered(&env, &env.editor, &rev, &a, &src)
        .await
        .unwrap();
    let (editor, owner, viewer) = (env.editor.clone(), env.owner.clone(), env.viewer.clone());
    assert_eq!(
        err_kind(annul(&env, &viewer, &rev, "rendered_pdf", "x").await),
        "role_forbidden"
    );
    assert_eq!(
        err_kind(annul(&env, &editor, &rev, "rendered_pdf", "").await),
        "invalid_params"
    );
    assert_eq!(
        err_kind(annul(&env, &editor, &rev, "photo", "x").await),
        "invalid_params"
    );
    annul(
        &env,
        &editor,
        &rev,
        "rendered_pdf",
        "Rendered from the wrong template.",
    )
    .await
    .unwrap();
    let e = annul(&env, &owner, &rev, "rendered_pdf", "again")
        .await
        .unwrap_err();
    assert_eq!(e.data.unwrap()["reason"], json!("no_attachment"));
    attach_rendered(&env, &env.editor, &rev, &b, &src)
        .await
        .unwrap();
    let list = attachments(&env, &rev).await;
    assert_eq!(list[0]["sha256"], json!(a));
    assert_eq!(
        list[0]["annulled"]["reason"],
        json!("Rendered from the wrong template.")
    );
    assert_eq!(list[0]["annulled"]["by"], json!(editor));
    assert_eq!(
        (list[1]["sha256"].clone(), list[1]["annulled"].clone()),
        (json!(b), Value::Null)
    );
    // The owner annuls what someone else uploaded.
    annul(&env, &owner, &rev, "rendered_pdf", "Owner's correction.")
        .await
        .unwrap();
    let blobs = env.store.referenced_blobs(KB).await.unwrap();
    assert!(
        blobs.hashes.contains(&a) && blobs.hashes.contains(&b),
        "annulled ones stay referenced"
    );
}

/// B7: with `approval_requires_rendered_pdf`, an approval names the current
/// rendered PDF, records it, and that PDF can no longer be annulled. Kills:
/// the switch ignored, a mismatch accepted, the annulment guard removed.
#[tokio::test]
async fn an_approval_binds_its_rendered_pdf() {
    let env = env().await;
    let _dir = with_blobs(&env);
    policy(&env, json!({"approval_requires_rendered_pdf": true})).await;
    let (_, revs) = requested(&env, &["n1"]).await;
    let src = rev_source(&env, &revs[0]).await;
    let ap = |sha: Option<&str>| {
        let mut p = json!({"kb_id": KB, "revision_id": revs[0]});
        if let Some(s) = sha {
            p["rendered_pdf_sha256"] = json!(s);
        }
        p
    };
    let a = put(&env, &pdf("rendered")).await;
    let other = put(&env, &pdf("something else")).await;
    assert_eq!(
        err_kind(as_(&env, &approver(&env), "kb/review.approve", ap(None)).await),
        "invalid_params"
    );
    assert_eq!(
        err_kind(as_(&env, &approver(&env), "kb/review.approve", ap(Some(&a))).await),
        "rendered_pdf_required"
    );
    attach_rendered(&env, &env.editor, &revs[0], &a, &src)
        .await
        .unwrap();
    let e = as_(&env, &approver(&env), "kb/review.approve", ap(Some(&other)))
        .await
        .unwrap_err();
    assert_eq!(
        (kind(&e), e.data.unwrap()["current"].clone()),
        ("rendered_pdf_mismatch".into(), json!(a))
    );
    as_(&env, &approver(&env), "kb/review.approve", ap(Some(&a)))
        .await
        .unwrap();
    let ev = event_objects(&env).await;
    let approve = ev.iter().find(|e| e["kind"] == json!("approve")).unwrap();
    assert_eq!(approve["payload"]["rendered_pdf_sha256"], json!(a));
    let p = json!({"kb_id": KB, "revision_id": revs[0], "kind": "rendered_pdf", "reason": "x"});
    let e = call(&env, &env.owner, "kb/review.annul_attachment", p)
        .await
        .unwrap_err();
    assert_eq!(
        (kind(&e), e.data.unwrap()["from"].clone()),
        ("invalid_transition".into(), json!("approved"))
    );
}

/// B1 under a race: two attaches of different bytes at once store exactly
/// one. Kills a write-once check that a concurrent writer can pass.
#[tokio::test]
async fn two_concurrent_attaches_of_different_bytes_store_one() {
    let env = env().await;
    let _dir = with_blobs(&env);
    policy(&env, json!({})).await;
    let (_, revs) = requested(&env, &["n1"]).await;
    let src = rev_source(&env, &revs[0]).await;
    let (a, b) = (put(&env, &pdf("one")).await, put(&env, &pdf("two")).await);
    let (x, y) = tokio::join!(
        attach_rendered(&env, &env.editor, &revs[0], &a, &src),
        attach_rendered(&env, &env.editor, &revs[0], &b, &src)
    );
    assert_eq!(
        usize::from(x.is_ok()) + usize::from(y.is_ok()),
        1,
        "{x:?} {y:?}"
    );
    assert_eq!(
        attachments(&env, &revs[0]).await.as_array().unwrap().len(),
        1
    );
}

/// The record holds even against SQL: the unique index allows one current
/// attachment per kind, a delete is refused, and the only update allowed is
/// an annulment. Kills dropping any of them.
#[tokio::test]
async fn the_database_keeps_the_attachment_record() {
    let env = env().await;
    let _dir = with_blobs(&env);
    policy(&env, json!({})).await;
    let (_, revs) = requested(&env, &["n1"]).await;
    let src = rev_source(&env, &revs[0]).await;
    let a = put(&env, &pdf("kept")).await;
    attach_rendered(&env, &env.editor, &revs[0], &a, &src)
        .await
        .unwrap();
    let id = revs[0].clone();
    for sql in [
        "DELETE FROM kb_review_attachments".to_string(),
        format!("UPDATE kb_review_attachments SET sha256 = '{}'", "00".repeat(32)),
        "UPDATE kb_review_attachments SET state = 'annulled'".to_string(),
        format!(
            "INSERT INTO kb_review_attachments (kb_id, revision_id, kind, seq, sha256, size,
             content_type, uploader, uploader_kind, at, event_seq, state)
             VALUES ('{KB}', '{id}', 'rendered_pdf', 9, '{}', 1, 'application/pdf', 'x', 'person', 0, 1, 'current')",
            "11".repeat(32)
        ),
    ] {
        let shown = sql.clone();
        let r = env.store.review_write(move |c| Ok(c.execute(&sql))).await.unwrap();
        assert!(r.is_err(), "allowed: {shown}");
    }
    assert_eq!(attachments(&env, &revs[0]).await[0]["sha256"], json!(a));
}

/// E12: the rendered-PDF attach and the annulment answer in their fixtures'
/// shapes, and refuse with the fixtures' kinds and fields. Kills a drift
/// between the contract mae-web builds against and the daemon.
#[tokio::test]
async fn the_rendered_pdf_and_annulment_answer_in_their_fixtures_shape() {
    let env = env().await;
    let _dir = with_blobs(&env);
    policy(&env, json!({})).await;
    let (_, revs) = requested(&env, &["n1", "n2"]).await;
    let src = rev_source(&env, &revs[0]).await;
    let other = rev_source(&env, &revs[1]).await;
    let rendered = fixture("attach_rendered_pdf");
    let a = put(&env, &pdf("wrong")).await;
    let r = attach_rendered(&env, &env.editor, &revs[0], &a, &src)
        .await
        .unwrap();
    assert_shape("attach_rendered_pdf", &example_result(&rendered, 0), &r);
    let e = attach_rendered(&env, &env.editor, &revs[0], &a, &other)
        .await
        .unwrap_err();
    let want = refusal(&rendered, "declared source is another revision's");
    assert_shape("source_mismatch", &want, e.data.as_deref().unwrap());
    let p =
        json!({"kb_id": KB, "revision_id": revs[0], "kind": "rendered_pdf", "reason": "Wrong."});
    let r = call(&env, &env.editor, "kb/review.annul_attachment", p)
        .await
        .unwrap();
    assert_shape(
        "annul_attachment",
        &example_result(&fixture("annul_attachment"), 0),
        &r,
    );
}

/// E12: the signed-PDF attach and the revision read, as above.
#[tokio::test]
async fn the_signed_pdf_and_revision_read_answer_in_their_fixtures_shape() {
    let env = env().await;
    let _dir = with_blobs(&env);
    policy(&env, json!({})).await;
    let (rq, revs) = requested(&env, &["n1", "n2"]).await;
    let src = rev_source(&env, &revs[0]).await;
    let rendered = put(&env, &pdf("rendered")).await;
    attach_rendered(&env, &env.editor, &revs[0], &rendered, &src)
        .await
        .unwrap();
    for r in &revs {
        let p = json!({"kb_id": KB, "revision_id": r});
        as_(&env, &approver(&env), "kb/review.approve", p)
            .await
            .unwrap();
    }
    let f = json!({"kb_id": KB, "request_id": rq, "effective_at": NOW});
    as_(&env, &approver(&env), "kb/review.enter_into_force", f)
        .await
        .unwrap();
    let signed = fixture("attach_signed_pdf");
    let s1 = put(&env, &pdf("signed")).await;
    let r = attach_signed(&env, &approver(&env), &revs[0], &s1)
        .await
        .unwrap();
    assert_shape("attach_signed_pdf", &example_result(&signed, 0), &r);
    let s2 = put(&env, &pdf("another signed")).await;
    let e = attach_signed(&env, &approver(&env), &revs[0], &s2)
        .await
        .unwrap_err();
    let want = refusal(
        &signed,
        "a signed PDF is already attached (different bytes)",
    );
    assert_shape("attachment_exists", &want, e.data.as_deref().unwrap());
    let text = put(&env, b"not a pdf").await;
    let e = attach_signed(&env, &approver(&env), &revs[1], &text)
        .await
        .unwrap_err();
    assert_shape(
        "attachment_type",
        &refusal(&signed, "the blob is not a PDF"),
        e.data.as_deref().unwrap(),
    );

    let p = json!({"kb_id": KB, "revision": revs[0]});
    let read = call(&env, &env.owner, "kb/query.get", p).await.unwrap();
    let want = example_result(&fixture("query_get_revision"), 0);
    assert_shape("query_get_revision", &want["revision"], &read["revision"]);
    assert_eq!(read["revision"]["attachments"].as_array().unwrap().len(), 2);
}

/// E3 under a race: two approvers attach a set's last two signed PDFs at
/// once. The set must still enter into force, by exactly one of them. Kills a
/// completion check read before the attaches serialize (both see the other's
/// PDF missing, and the set is left waiting).
#[tokio::test]
async fn the_last_two_signed_pdfs_at_once_put_the_set_in_force() {
    let env = env().await;
    let _dir = with_blobs(&env);
    policy(
        &env,
        json!({"approval_record": "signed_pdf", "in_force_on_approval": true,
        "reason": "The signed original is the record."}),
    )
    .await;
    let (_, revs) = requested(&env, &["n1", "n2"]).await;
    for r in &revs {
        let p = json!({"kb_id": KB, "revision_id": r});
        as_(&env, &approver(&env), "kb/review.approve", p)
            .await
            .unwrap();
    }
    let (s1, s2) = (put(&env, &pdf("s1")).await, put(&env, &pdf("s2")).await);
    let (y, z) = (
        approver(&env),
        signed_in(&env.owner, &["approvers"], PrincipalKind::Person),
    );
    let (a, b) = tokio::join!(
        attach_signed(&env, &y, &revs[0], &s1),
        attach_signed(&env, &z, &revs[1], &s2)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    let entered = [&a, &b]
        .iter()
        .filter(|r| r["in_force_since"].is_string())
        .count();
    assert_eq!(entered, 1, "{a} {b}");
    for n in ["n1", "n2"] {
        assert!(
            status(&env, n).await["in_force"].is_object(),
            "{n} in force"
        );
    }
}
