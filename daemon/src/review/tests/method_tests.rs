//! PR-4's "Tests" list, method by method, plus the withdraw and
//! `approval_record` rules this PR adds. Each names the guard it kills.

use std::collections::BTreeSet;

use mae_sync::kb::history::VersionKind;
use mae_sync::kb::AttachmentRef;
use serde_json::json;

use super::*;

/// Rows in a review table, counted under one read.
async fn count(env: &Env, table: &'static str) -> i64 {
    env.store
        .review_read(move |c| {
            let mut s = c.prepare(format!("SELECT COUNT(*) FROM {table}"))?;
            s.next()?;
            Ok(s.read::<i64, _>(0)?)
        })
        .await
        .unwrap()
}

/// A request naming a superseded `version` is refused with the current one,
/// and writes nothing: no request, no revision, no source, no event. A request
/// naming the current version then succeeds. Kills the stale-version check.
#[tokio::test]
async fn a_request_naming_a_superseded_version_is_refused_with_the_current_one() {
    let env = env().await;
    enable(&env).await;
    let seen = version(&env, "n1").await;
    retitle(&env, "n1", "moved on").await;
    let current = version(&env, "n1").await;
    assert_ne!(seen, current);
    let events_before = count(&env, "kb_review_events").await;
    let e = call(
        &env,
        &env.editor,
        "kb/review.request",
        json!({"kb_id": KB, "nodes": [{"node_id": "n1", "version": seen}], "reason": "r"}),
    )
    .await
    .unwrap_err();
    assert_eq!(kind(&e), "stale_version");
    let data = e.data.as_deref().unwrap();
    assert_eq!(data["current"], json!(current));
    assert_eq!(data["node_id"], json!("n1"));
    for t in [
        "kb_review_requests",
        "kb_review_revisions",
        "kb_review_sources",
    ] {
        assert_eq!(count(&env, t).await, 0, "{t} written by a refused request");
    }
    assert_eq!(count(&env, "kb_review_events").await, events_before);
    request(&env, &env.editor, &["n1"]).await.unwrap();
}

/// A revision's body and `version` are unchanged after the working copy is
/// edited, while the working view follows the edit. Kills a revision read
/// from the working copy instead of its frozen source.
#[tokio::test]
async fn a_revisions_body_and_version_are_unchanged_after_the_working_copy_is_edited() {
    let env = env().await;
    enable(&env).await;
    let frozen_version = version(&env, "n2").await;
    let r = request(&env, &env.editor, &["n2"]).await.unwrap();
    let rev = r["revisions"][0]["revision_id"]
        .as_str()
        .unwrap()
        .to_string();
    let params = json!({"kb_id": KB, "node_id": "n2", "revision": rev});
    let get = || call(&env, &env.viewer, "kb/query.get", params.clone());
    let before = get().await.unwrap();
    let editor = env.editor.clone();
    edit_with(&env.store, KB, "n2", &editor, |d| {
        d.set_body("A different body.")
    })
    .await;
    let after = get().await.unwrap();
    assert_eq!(before, after);
    assert_eq!(after["body"], json!("Body of n2."));
    assert_eq!(after["version"], json!(frozen_version));
    assert_eq!(after["revision"]["state"], json!("in_review"));
    let working = call(
        &env,
        &env.viewer,
        "kb/query.get",
        json!({"kb_id": KB, "node_id": "n2"}),
    )
    .await
    .unwrap();
    assert_eq!(working["body"], json!("A different body."));
    assert_ne!(working["version"], after["version"]);
}

/// The freeze cuts a `review_freeze` history version holding exactly the
/// frozen node, at the WAL cutoff the revision records (O8, A2).
#[tokio::test]
async fn the_freeze_cuts_a_review_freeze_version_the_revision_references() {
    let env = env().await;
    enable(&env).await;
    retitle(&env, "n1", "edited").await;
    request(&env, &env.editor, &["n1"]).await.unwrap();
    let rev = env
        .store
        .review_read(|c| crate::review::store::node_revisions(c, KB, "n1"))
        .await
        .unwrap()
        .remove(0);
    let h = env.store.history_list(KB, "n1").await.unwrap();
    let v = h.iter().find(|e| e.seq == rev.history_seq).unwrap();
    assert_eq!(v.kind, VersionKind::ReviewFreeze);
    assert_eq!(v.wal_to, rev.cutoff_wal);
    let node = v.node().unwrap().unwrap();
    assert_eq!(hex::encode(&node.digest()[..16]), rev.version);
    assert_eq!(node.title, "edited");
}

/// D3: a requester may only tighten `in_force_on_approval`. An editor asking
/// for `true` against a KB default of `false` is refused `role_forbidden`;
/// asking for `false` is fine; the owner may loosen. Once the KB default is
/// `true`, an editor may ask for either. Kills the tighten-only check
/// reversed or removed.
#[tokio::test]
async fn a_requester_cannot_loosen_in_force_on_approval() {
    let env = env().await;
    enable(&env).await;
    let ask = |who: String, node: &'static str, iofa: bool| {
        let env = &env;
        async move {
            let v = version(env, node).await;
            call(
                env,
                &who,
                "kb/review.request",
                json!({"kb_id": KB, "nodes": [{"node_id": node, "version": v}],
                       "in_force_on_approval": iofa, "reason": "r"}),
            )
            .await
        }
    };
    assert_eq!(
        err_kind(ask(env.editor.clone(), "n1", true).await),
        "role_forbidden"
    );
    assert_eq!(count(&env, "kb_review_requests").await, 0);
    ask(env.editor.clone(), "n1", false).await.unwrap();
    ask(env.owner.clone(), "n2", true).await.unwrap();
    call(
        &env,
        &env.owner,
        "kb/review.set_policy",
        json!({"kb_id": KB, "in_force_on_approval": true}),
    )
    .await
    .unwrap();
    ask(env.editor.clone(), "n3", true).await.unwrap();
    let modes: Vec<bool> = env
        .store
        .review_read(|c| {
            let mut s =
                c.prepare("SELECT in_force_on_approval FROM kb_review_requests ORDER BY ord")?;
            let mut out = Vec::new();
            while let sqlite::State::Row = s.next()? {
                out.push(s.read::<i64, _>(0)? != 0);
            }
            Ok(out)
        })
        .await
        .unwrap();
    assert_eq!(modes, [false, true, true]);
}

/// D3: the switch is the owner's. An editor and a viewer turning review on or
/// off are refused and change nothing; the owner's change is a
/// `policy_changed` event in a chain that verifies. Kills the owner check.
#[tokio::test]
async fn only_the_owner_turns_review_on_or_off_and_each_change_is_an_event() {
    let env = env().await;
    for who in [&env.editor, &env.viewer] {
        for on in [true, false] {
            let r = call(
                &env,
                who,
                "kb/review.set_policy",
                json!({"kb_id": KB, "review_enabled": on}),
            )
            .await;
            assert_eq!(err_kind(r), "role_forbidden");
        }
    }
    assert_eq!(count(&env, "kb_review_policy").await, 0);
    assert_eq!(count(&env, "kb_review_events").await, 0);
    let on = call(
        &env,
        &env.owner,
        "kb/review.set_policy",
        json!({"kb_id": KB, "review_enabled": true}),
    )
    .await
    .unwrap();
    assert_eq!(on["policy"]["review_enabled"], json!(true));
    let off = call(
        &env,
        &env.owner,
        "kb/review.set_policy",
        json!({"kb_id": KB, "review_enabled": false}),
    )
    .await
    .unwrap();
    assert_ne!(on["event_id"], off["event_id"]);
    let same = call(
        &env,
        &env.owner,
        "kb/review.set_policy",
        json!({"kb_id": KB, "review_enabled": false}),
    )
    .await
    .unwrap();
    assert!(same["event_id"].is_null(), "no change, no event");
    let events = event_objects(&env).await;
    let kinds: Vec<_> = events.iter().map(|e| e["kind"].clone()).collect();
    assert_eq!(kinds, [json!("policy_changed"), json!("policy_changed")]);
    assert_eq!(events[0]["payload"]["review_enabled"], json!(true));
    assert_eq!(events[1]["payload"]["review_enabled"], json!(false));
    assert_eq!(events[0]["actor"]["principal"], json!(env.owner));
    assert_eq!(verified_events(&env).await, 2);
}

/// E3: changing `approval_record` needs a reason, recorded in its event; a
/// change of anything else does not. Kills the reason requirement.
#[tokio::test]
async fn changing_the_record_of_approval_needs_a_reason_and_records_it() {
    let env = env().await;
    let p = |extra: Value| {
        let mut v = json!({"kb_id": KB});
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        v
    };
    for blank in [json!({}), json!({"reason": "  "}), json!({"reason": null})] {
        let mut params = p(json!({"approval_record": "signed_pdf"}));
        params
            .as_object_mut()
            .unwrap()
            .extend(blank.as_object().unwrap().clone());
        let e = call(&env, &env.owner, "kb/review.set_policy", params)
            .await
            .unwrap_err();
        assert_eq!(kind(&e), "invalid_params");
        assert_eq!(e.data.as_deref().unwrap()["param"], json!("reason"));
    }
    assert_eq!(count(&env, "kb_review_events").await, 0);
    let ok = call(
        &env,
        &env.owner,
        "kb/review.set_policy",
        p(json!({"approval_record": "signed_pdf", "reason": "The assessor has not accepted in-system approval."})),
    )
    .await
    .unwrap();
    assert_eq!(ok["policy"]["approval_record"], json!("signed_pdf"));
    call(
        &env,
        &env.owner,
        "kb/review.set_policy",
        p(json!({"review_enabled": true})),
    )
    .await
    .unwrap();
    let ev = event_objects(&env).await;
    assert_eq!(ev[0]["payload"]["approval_record"], json!("signed_pdf"));
    assert_eq!(
        ev[0]["payload"]["reason"],
        json!("The assessor has not accepted in-system approval.")
    );
    // Setting the value it already has is no change, and needs no reason.
    call(
        &env,
        &env.owner,
        "kb/review.set_policy",
        p(json!({"approval_record": "signed_pdf"})),
    )
    .await
    .unwrap();
    for bad in [
        json!({"approval_record": "pdf"}),
        json!({"required_acts": ["reviewed"]}),
        json!({"required_acts": ["approved", "approved"]}),
        json!({"approvers": [{"role": "quality"}]}),
        json!({"approvers": [{"group": "q", "member": "SHA256:x"}]}),
        json!({"reviewers": [{"member": "not-a-fingerprint"}]}),
        json!({"approvers": [{"group": "q"}, {"group": "q"}]}),
        json!({"code_prefixes": ["pro"]}),
    ] {
        let r = call(&env, &env.owner, "kb/review.set_policy", p(bad.clone())).await;
        assert_eq!(err_kind(r), "invalid_params", "{bad}");
    }
}

/// With review off, `kb/review.request` is refused `review_not_enabled`, and
/// `kb/query.get` answers exactly as it did, before and after review is turned
/// on and a revision frozen. Kills the switch check, and a freeze that
/// changes what a reader of the working copy sees.
#[tokio::test]
async fn with_review_off_a_request_is_refused_and_readers_see_what_they_saw() {
    let env = env().await;
    let get = || {
        call(
            &env,
            &env.viewer,
            "kb/query.get",
            json!({"kb_id": KB, "node_id": "n3"}),
        )
    };
    let before = get().await.unwrap();
    assert_eq!(
        err_kind(request(&env, &env.editor, &["n3"]).await),
        "review_not_enabled"
    );
    assert_eq!(count(&env, "kb_review_revisions").await, 0);
    assert_eq!(get().await.unwrap(), before);
    enable(&env).await;
    request(&env, &env.editor, &["n3"]).await.unwrap();
    assert_eq!(get().await.unwrap(), before);
}

/// D2: codes are unique per (KB, prefix, number). Two concurrent `set_code`
/// for one number on two nodes: exactly one wins, the other is refused
/// `doc_code_taken` naming the winner. An unlisted prefix is refused. A code
/// once given is never given to another node, even after its holder moves
/// on. Kills a check-then-insert outside the transaction.
#[tokio::test]
async fn two_concurrent_set_codes_for_one_number_one_wins() {
    let env = env().await;
    enable(&env).await;
    let set = |node: &'static str, code: &'static str| {
        let store = Arc::clone(&env.store);
        let owner = env.owner.clone();
        async move {
            call_on(
                &store,
                &owner,
                "kb/review.set_code",
                json!({"kb_id": KB, "node_id": node, "doc_code": code}),
            )
            .await
        }
    };
    let (a, b) = tokio::join!(
        tokio::spawn(set("n1", "PRO-7")),
        tokio::spawn(set("n2", "PRO-7"))
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    let (winner, loser) = match (&a, &b) {
        (Ok(_), Err(e)) => ("n1", e),
        (Err(e), Ok(_)) => ("n2", e),
        other => panic!("exactly one must win: {other:?}"),
    };
    assert_eq!(kind(loser), "doc_code_taken");
    assert_eq!(loser.data.as_deref().unwrap()["holder"], json!(winner));
    assert_eq!(err_kind(set("n3", "IT-1").await), "invalid_params");
    assert_eq!(err_kind(set("n3", "PRO-07").await), "invalid_params");
    set(winner, "PRO-8").await.unwrap();
    let other = if winner == "n1" { "n2" } else { "n1" };
    assert_eq!(err_kind(set(other, "PRO-7").await), "doc_code_taken");
    let st = call(
        &env,
        &env.viewer,
        "kb/review.status",
        json!({"kb_id": KB, "node_id": winner}),
    )
    .await
    .unwrap();
    assert_eq!(st["doc_code"], json!("PRO-8"));
    assert_eq!(verified_events(&env).await, 3, "policy, two codes");
}

/// E2: a withdrawal by the requester ends the whole request, every member
/// `withdrawn`, as one event; another member is refused; a second withdrawal
/// is an invalid transition; an unknown request is `unknown_request`. A node
/// already in review cannot be requested again until it is withdrawn.
#[tokio::test]
async fn a_withdrawal_by_the_requester_ends_the_whole_request() {
    let env = env().await;
    enable(&env).await;
    let r = request(&env, &env.editor, &NODES).await.unwrap();
    let rq = r["request_id"].clone();
    let again = request(&env, &env.owner, &["n2"]).await.unwrap_err();
    assert_eq!(kind(&again), "invalid_transition");
    assert_eq!(again.data.as_deref().unwrap()["from"], json!("in_review"));
    let w = |who: String, id: Value| {
        let env = &env;
        async move {
            call(
                env,
                &who,
                "kb/review.withdraw",
                json!({"kb_id": KB, "request_id": id, "reason": "wider revision"}),
            )
            .await
        }
    };
    assert_eq!(
        err_kind(w(env.owner.clone(), rq.clone()).await),
        "not_approver"
    );
    assert_eq!(
        err_kind(w(env.editor.clone(), json!("rq-unknown")).await),
        "unknown_request"
    );
    w(env.editor.clone(), rq.clone()).await.unwrap();
    let states: BTreeSet<String> = env
        .store
        .review_read(|c| {
            let mut s = c.prepare("SELECT DISTINCT state FROM kb_review_revisions")?;
            let mut out = BTreeSet::new();
            while let sqlite::State::Row = s.next()? {
                out.insert(s.read::<String, _>(0)?);
            }
            Ok(out)
        })
        .await
        .unwrap();
    assert_eq!(states, BTreeSet::from(["withdrawn".to_string()]));
    let e = w(env.editor.clone(), rq).await.unwrap_err();
    assert_eq!(kind(&e), "invalid_transition");
    assert_eq!(e.data.as_deref().unwrap()["from"], json!("withdrawn"));
    let pending = call(&env, &env.viewer, "kb/review.pending", json!({"kb_id": KB}))
        .await
        .unwrap();
    assert_eq!(pending["requests"], json!([]));
    request(&env, &env.owner, &["n2"]).await.unwrap();
    let kinds: Vec<_> = event_objects(&env)
        .await
        .iter()
        .map(|e| e["kind"].clone())
        .collect();
    assert_eq!(
        kinds,
        [
            json!("policy_changed"),
            json!("request"),
            json!("withdraw"),
            json!("request")
        ]
    );
}

/// D8: `status` turns to draft when the working copy moves past the latest
/// revision, and is not a draft while it stands at it.
#[tokio::test]
async fn status_is_draft_exactly_when_the_working_copy_differs_from_the_latest_revision() {
    let env = env().await;
    enable(&env).await;
    let st = || {
        call(
            &env,
            &env.viewer,
            "kb/review.status",
            json!({"kb_id": KB, "node_id": "n1"}),
        )
    };
    assert_eq!(st().await.unwrap()["draft"], json!(true), "never requested");
    request(&env, &env.editor, &["n1"]).await.unwrap();
    let s = st().await.unwrap();
    assert_eq!(s["draft"], json!(false));
    assert_eq!(
        s["in_review"]["waiting_for"][0]["capacity"],
        json!("approve")
    );
    retitle(&env, "n1", "next draft").await;
    assert_eq!(st().await.unwrap()["draft"], json!(true));
}

/// ADR-115 D5 with PR-4's holder: a blob held only by a withdrawn revision's
/// frozen map stays in the live set after the node drops it and its history
/// versions have no content left. Kills the revision holder left out of the
/// sweep's live set.
#[tokio::test]
async fn a_blob_held_only_by_a_withdrawn_revisions_frozen_map_stays_live() {
    let env = env().await;
    enable(&env).await;
    let sha = hex::encode([7u8; 32]);
    let pic = AttachmentRef::new(sha.clone(), 10, "image/png").unwrap();
    let editor = env.editor.clone();
    edit_with(&env.store, KB, "n1", &editor, |d| {
        d.set_attachment("fig.png", &pic).unwrap()
    })
    .await;
    let r = request(&env, &env.editor, &["n1"]).await.unwrap();
    call(
        &env,
        &env.editor,
        "kb/review.withdraw",
        json!({"kb_id": KB, "request_id": r["request_id"], "reason": "r"}),
    )
    .await
    .unwrap();
    edit_with(&env.store, KB, "n1", &editor, |d| {
        d.remove_attachment("fig.png")
    })
    .await;
    // History versions drop their content when pruned; play every one pruned.
    env.backend
        .execute_for_test("UPDATE kb_node_history SET content = NULL");
    let live = env.store.referenced_blobs(KB).await.unwrap();
    assert!(live.complete(), "{:?}", live.unreadable);
    assert!(
        live.hashes.contains(&sha),
        "the revision's blob left the live set"
    );
}

/// E1/E2: a withdrawal by anyone but the requester is refused identically,
/// kind and message, whether the request is open, has ended, or is in force,
/// so the refusal tells a non-requester nothing of where it stands. Kills a
/// requester check placed after the state check.
#[tokio::test]
async fn a_non_requester_gets_one_refusal_whatever_the_request_state() {
    let env = env().await;
    enable(&env).await;
    let mut ids = Vec::new();
    for n in NODES {
        ids.push(request(&env, &env.editor, &[n]).await.unwrap()["request_id"].clone());
    }
    let w = |who: String, id: Value| {
        let env = &env;
        async move {
            let params = json!({"kb_id": KB, "request_id": id, "reason": "r"});
            call(env, &who, "kb/review.withdraw", params).await
        }
    };
    w(env.editor.clone(), ids[1].clone()).await.unwrap();
    let in_force = ids[2].as_str().unwrap().to_string();
    env.store
        .review_write(move |c| {
            let mut s = c.prepare(
                "UPDATE kb_review_revisions SET state = 'in_force' WHERE request_id = ?1",
            )?;
            s.bind((1, in_force.as_str()))?;
            s.next()?;
            Ok(())
        })
        .await
        .unwrap();
    let mut seen = BTreeSet::new();
    for id in &ids {
        let e = w(env.owner.clone(), id.clone()).await.unwrap_err();
        seen.insert((kind(&e), e.message));
    }
    let only = BTreeSet::from([(
        "not_approver".to_string(),
        "only the requester or a listed approver may withdraw this request".to_string(),
    )]);
    assert_eq!(seen, only);
}

/// E3: a revision keeps the record of approval in force when it was
/// requested; changing the policy later changes the KB's answer, not the
/// revision's. Kills a reader that falls back to the live policy.
#[tokio::test]
async fn a_revisions_approval_record_is_the_one_in_force_when_requested() {
    let env = env().await;
    enable(&env).await;
    let r = request(&env, &env.editor, &["n1"]).await.unwrap();
    call(
        &env,
        &env.owner,
        "kb/review.set_policy",
        json!({"kb_id": KB, "approval_record": "signed_pdf", "reason": "paper record"}),
    )
    .await
    .unwrap();
    let s = call(
        &env,
        &env.viewer,
        "kb/review.status",
        json!({"kb_id": KB, "node_id": "n1"}),
    )
    .await
    .unwrap();
    assert_eq!(s["approval_record"], json!("signed_pdf"), "the KB's answer");
    assert_eq!(
        s["revisions"][0]["revision_id"],
        r["revisions"][0]["revision_id"]
    );
    assert_eq!(s["revisions"][0]["approval_record"], json!("act"));
    let q = call(
        &env,
        &env.viewer,
        "kb/query.revisions",
        json!({"kb_id": KB, "node_id": "n1"}),
    )
    .await
    .unwrap();
    assert_eq!(q["revisions"][0]["approval_record"], json!("act"));
}

/// A storage failure reaches the client as `internal` with a fixed message;
/// the database's own text (table names, SQL) stays in the daemon's log.
/// Kills a fault that formats the storage error into the reply.
#[tokio::test]
async fn a_storage_fault_tells_the_client_nothing_of_the_database() {
    let env = env().await;
    enable(&env).await;
    env.store
        .review_write(|c| Ok(c.execute("DROP TABLE kb_review_requests")?))
        .await
        .unwrap();
    let e = call(&env, &env.viewer, "kb/review.pending", json!({"kb_id": KB}))
        .await
        .unwrap_err();
    assert_eq!(kind(&e), "internal");
    assert_eq!(
        e.message,
        "publication review could not read or write its record"
    );
}

/// The performer lists (D3): stored, returned, and their changes recorded; a
/// named member must be bound as a person (ADR-117 D8). Kills dropping the
/// lists on write and the person check.
#[tokio::test]
async fn the_performer_lists_are_stored_and_a_named_member_must_be_a_person() {
    let env = env().await;
    let groups = json!({"kb_id": KB,
        "reviewers": [{"group": "quality-reviewers"}],
        "approvers": [{"group": "quality-approvers"}]});
    let ok = call(&env, &env.owner, "kb/review.set_policy", groups)
        .await
        .unwrap();
    assert_eq!(
        ok["policy"]["approvers"],
        json!([{"group": "quality-approvers"}])
    );
    assert_eq!(
        ok["policy"]["reviewers"],
        json!([{"group": "quality-reviewers"}])
    );
    let ev = event_objects(&env).await;
    assert_eq!(
        ev[0]["payload"]["approvers"],
        json!([{"group": "quality-approvers"}])
    );
    // No binding table on this daemon: nobody is a person, so a named member
    // is refused, and the refusal names them.
    let fp = mae_mcp::identity::Identity::generate("someone").fingerprint();
    let e = call(
        &env,
        &env.owner,
        "kb/review.set_policy",
        json!({"kb_id": KB, "approvers": [{"member": fp}]}),
    )
    .await
    .unwrap_err();
    assert_eq!(kind(&e), "performer_not_person");
    assert_eq!(e.data.as_deref().unwrap()["member"], json!(fp));
    assert_eq!(
        event_objects(&env).await.len(),
        1,
        "a refused change records nothing"
    );
}
