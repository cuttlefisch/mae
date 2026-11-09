//! Planning over synthetic trees (no git): selection, refusals across files,
//! exclusion, the plan hash and the report.

use std::collections::HashMap;

use super::page;
use crate::seed::*;

const T0: i64 = 1_700_000_000;

fn input(files: &[(&str, Vec<u8>)]) -> PlanInput {
    let mut entries = Vec::new();
    let mut blobs = HashMap::new();
    let mut file_times = HashMap::new();
    for (i, (path, bytes)) in files.iter().enumerate() {
        let sha = format!("{:040x}", i + 1);
        entries.push(TreeEntry {
            mode: "100644".into(),
            kind: "blob".into(),
            sha: sha.clone(),
            path: Ok(path.to_string()),
        });
        blobs.insert(sha, bytes.clone());
        let t = T0 + 10 * i as i64;
        file_times.insert(
            path.to_string(),
            FileTimes {
                created: t,
                modified: t + 5,
            },
        );
    }
    PlanInput {
        source: Source {
            repo: "/synthetic".into(),
            rev: "v1".into(),
            commit: "c".repeat(40),
            tree: "t".repeat(40),
        },
        entries,
        blobs,
        attrs: Vec::new(),
        file_times,
        received_at: 42,
    }
}

fn opts() -> PlanOptions {
    PlanOptions {
        repo: "/synthetic".into(),
        rev: "v1".into(),
        include: Vec::new(),
        exclude: Vec::new(),
        max_body_bytes: DEFAULT_MAX_BODY_BYTES,
        require_canonical: false,
        code_property: None,
    }
}

fn ok_page(id: &str) -> Vec<u8> {
    page(&[("ID", id)], Some("T"), None, &format!("Texto de {id}.\n")).into_bytes()
}

fn refused(plan: &SeedPlan) -> Vec<(String, ProblemKind)> {
    plan.refusals
        .iter()
        .map(|r| (r.path.clone(), r.kind))
        .collect()
}

/// Test 4: one bad file of each kind among valid ones; every offender is
/// listed, the plan is not OK, and there is no plan hash. Kills: stopping at
/// the first problem, skipping a bad file, or tolerating a duplicate id.
#[test]
fn every_offender_is_listed_and_the_plan_is_refused() {
    let anchor_clash = page(
        &[("ID", "clash")],
        None,
        None,
        "* H\n:PROPERTIES:\n:ID: ok-1\n:END:\n",
    );
    let big = page(
        &[("ID", "big")],
        None,
        None,
        &"x".repeat(DEFAULT_MAX_BODY_BYTES + 1),
    );
    let files = vec![
        ("notes/ok1.org", ok_page("ok-1")),
        ("notes/ok2.org", ok_page("ok-2")),
        (
            "notes/noid.org",
            b":PROPERTIES:\n:ROLE: a\n:END:\nx\n".to_vec(),
        ),
        ("notes/dup-a.org", ok_page("dup")),
        ("notes/dup-b.org", ok_page("dup")),
        ("notes/anchor.org", anchor_clash.into_bytes()),
        (
            "notes/latin1.org",
            b":PROPERTIES:\n:ID: l\n:END:\ncaf\xe9\n".to_vec(),
        ),
        (
            "notes/crlf.org",
            b":PROPERTIES:\r\n:ID: c\r\n:END:\r\n".to_vec(),
        ),
        (
            "notes/decl.org",
            page(&[("ID", "d")], None, None, "#+TODO: A | B\n").into_bytes(),
        ),
        ("notes/big.org", big.into_bytes()),
    ];
    let plan = plan_entries(input(&files), &opts()).unwrap();
    let got = refused(&plan);
    use ProblemKind::*;
    for want in [
        ("notes/noid.org", MissingId),
        ("notes/dup-a.org", DuplicateId),
        ("notes/dup-b.org", DuplicateId),
        ("notes/anchor.org", DuplicateId),
        ("notes/ok1.org", DuplicateId),
        ("notes/latin1.org", NotUtf8),
        ("notes/crlf.org", CarriageReturn),
        ("notes/decl.org", Declaration),
        ("notes/big.org", BodyTooLarge),
    ] {
        assert!(
            got.contains(&(want.0.to_string(), want.1)),
            "{want:?} missing from {got:?}"
        );
    }
    assert!(
        !got.iter().any(|(p, _)| p == "notes/ok2.org"),
        "a valid file is not refused"
    );
    assert!(!plan.ok());
    assert_eq!(plan.plan_hash(), None, "a refused plan has no hash");
}

/// The cap is inclusive: exactly the limit is seeded, one byte more is not.
/// Kills: `>=` for `>`, and any truncation to fit.
#[test]
fn a_body_of_exactly_the_limit_is_accepted() {
    let mut o = opts();
    o.max_body_bytes = 64;
    let at = page(&[("ID", "a")], None, None, &"y".repeat(64));
    let over = page(&[("ID", "b")], None, None, &"y".repeat(65));
    let plan = plan_entries(input(&[("a.org", at.into_bytes())]), &o).unwrap();
    assert!(plan.ok(), "{:?}", plan.refusals);
    assert_eq!(plan.nodes[0].file.body.len(), 64);
    let plan = plan_entries(input(&[("b.org", over.into_bytes())]), &o).unwrap();
    assert_eq!(
        refused(&plan),
        vec![("b.org".to_string(), ProblemKind::BodyTooLarge)]
    );
}

/// An id-less file is a refusal by default, and leaves only by name, and the
/// report says so. Kills: silent skipping, an unlisted exclusion.
#[test]
fn an_id_less_file_leaves_only_by_exclude_and_is_listed() {
    let files = [
        ("notes/a.org", ok_page("a")),
        ("notes/noid.org", b":PROPERTIES:\n:X: y\n:END:\n".to_vec()),
    ];
    let plan = plan_entries(input(&files), &opts()).unwrap();
    assert_eq!(
        refused(&plan),
        vec![("notes/noid.org".to_string(), ProblemKind::MissingId)]
    );

    let mut o = opts();
    o.exclude = vec!["notes/noid.org".into()];
    let plan = plan_entries(input(&files), &o).unwrap();
    assert!(plan.ok(), "{:?}", plan.refusals);
    assert_eq!(plan.excluded, vec!["notes/noid.org"]);
    assert_eq!(plan.nodes.len(), 1);
    let text = render_text(&plan);
    assert!(
        text.contains("== excluded by --exclude (1) ==\nnotes/noid.org\n"),
        "{text}"
    );
    let json = render_json(&plan, &o.exclude);
    assert_eq!(json["excluded"], serde_json::json!(["notes/noid.org"]));
    assert_eq!(
        json["options"]["exclude"],
        serde_json::json!(["notes/noid.org"])
    );
}

#[test]
fn an_exclude_outside_the_selection_is_refused() {
    let mut o = opts();
    o.include = vec!["notes/**/*.org".into()];
    o.exclude = vec!["notes/typo.org".into(), "README.org".into()];
    let files = [
        ("notes/a.org", ok_page("a")),
        ("README.org", b"readme\n".to_vec()),
    ];
    let plan = plan_entries(input(&files), &o).unwrap();
    let got = refused(&plan);
    assert!(got.contains(&("notes/typo.org".into(), ProblemKind::ExcludeNotSelected)));
    assert!(got.contains(&("README.org".into(), ProblemKind::ExcludeNotSelected)));
}

/// Paths outside `--include` are listed, with a count, and are not refused,
/// whatever they contain. Kills: a default that seeds the canary pages.
#[test]
fn files_outside_the_include_set_are_listed_as_not_selected() {
    let mut o = opts();
    o.include = vec!["notes/**/*.org".into()];
    let files = [
        ("notes/x/a.org", ok_page("a")),
        ("notes/b.org", ok_page("b")),
        ("canary/leak.org", ok_page("canary:leak")),
        ("README.org", b"no drawer\n".to_vec()),
        ("tools/gate.sh", b"#!/bin/sh\n".to_vec()),
    ];
    let plan = plan_entries(input(&files), &o).unwrap();
    assert!(plan.ok(), "{:?}", plan.refusals);
    let paths: Vec<&str> = plan.nodes.iter().map(|n| n.path.as_str()).collect();
    assert_eq!(paths, vec!["notes/b.org", "notes/x/a.org"]);
    assert_eq!(
        plan.not_selected,
        vec!["README.org", "canary/leak.org", "tools/gate.sh"]
    );
    let text = render_text(&plan);
    assert!(
        text.contains(
            "== not selected by --include (3) ==\nREADME.org\ncanary/leak.org\ntools/gate.sh\n"
        ),
        "{text}"
    );
}

#[test]
fn selected_entries_that_are_not_plain_org_files_are_refused() {
    let mut inp = input(&[("a.org", ok_page("a")), ("notes/x.txt", b"x".to_vec())]);
    let mut o = opts();
    o.include = vec!["**".into()];
    let mut entry = |mode: &str, kind: &str, path: Result<String, Vec<u8>>| {
        inp.entries.push(TreeEntry {
            mode: mode.into(),
            kind: kind.into(),
            sha: "f".repeat(40),
            path,
        });
    };
    entry("120000", "blob", Ok("link.org".into()));
    entry("160000", "commit", Ok("sub.org".into()));
    entry("100644", "blob", Err(b"bad\xff.org".to_vec()));
    let plan = plan_entries(inp, &o).unwrap();
    let got = refused(&plan);
    use ProblemKind::*;
    for want in [
        ("link.org", Symlink),
        ("sub.org", Submodule),
        ("notes/x.txt", NotOrgFile),
    ] {
        assert!(
            got.contains(&(want.0.to_string(), want.1)),
            "{want:?} missing from {got:?}"
        );
    }
    assert!(got.iter().any(|(_, k)| *k == NonUtf8Path), "{got:?}");
}

#[test]
fn checkout_attributes_on_selected_files_are_refused() {
    let mut inp = input(&[("a.org", ok_page("a"))]);
    inp.attrs.push(CheckoutAttr {
        path: "a.org".into(),
        attr: "eol".into(),
        value: "crlf".into(),
    });
    let plan = plan_entries(inp, &opts()).unwrap();
    assert_eq!(
        refused(&plan),
        vec![("a.org".to_string(), ProblemKind::CheckoutAttribute)]
    );
}

/// Test 2 at plan level: a non-canonical header is listed with its differing
/// lines, accepted by default, refused under `--require-canonical`.
#[test]
fn non_canonical_headers_are_listed_and_fail_under_require_canonical() {
    let canonical = ok_page("c");
    let loose = b":PROPERTIES:\n:ID:    n\n:END:\n#+TITLE: T\nTexto.\n".to_vec();
    let files = [("c.org", canonical), ("n.org", loose)];
    let plan = plan_entries(input(&files), &opts()).unwrap();
    assert!(plan.ok());
    let n = plan.nodes.iter().find(|n| n.path == "n.org").unwrap();
    assert!(!n.canonical);
    let (removed, added) = n.header_diff.clone().unwrap();
    assert_eq!(removed, vec![":ID:    n\n", ":END:\n", "#+TITLE: T\n"]);
    assert_eq!(added, vec![":ID: n\n", ":END:\n", "#+title: T\n"]);
    assert!(
        plan.nodes
            .iter()
            .find(|n| n.path == "c.org")
            .unwrap()
            .canonical
    );
    let text = render_text(&plan);
    assert!(text.contains("n.org\n  - :ID:    n\n"), "{text}");

    let mut o = opts();
    o.require_canonical = true;
    let plan = plan_entries(input(&files), &o).unwrap();
    assert_eq!(
        refused(&plan),
        vec![("n.org".to_string(), ProblemKind::NonCanonical)]
    );
}

/// Same content gives the same hash, in any tree order; any change to a body
/// byte or a creation time gives another. Kills: hashing in input order,
/// leaving a field out of the hash.
#[test]
fn the_plan_hash_is_deterministic_and_covers_content_and_time() {
    let files = vec![("a.org", ok_page("a")), ("b/b.org", ok_page("b"))];
    let h = |inp: PlanInput| plan_entries(inp, &opts()).unwrap().plan_hash().unwrap();
    let base = h(input(&files));
    assert_eq!(base, h(input(&files)));
    let mut reordered = input(&files);
    reordered.entries.reverse();
    assert_eq!(base, h(reordered), "tree order does not matter");
    let mut edited = files.clone();
    edited[1].1 = page(&[("ID", "b")], Some("T"), None, "Texto de b!\n").into_bytes();
    assert_ne!(base, h(input(&edited)), "a body byte changes the hash");
    let mut later = input(&files);
    later.file_times.get_mut("a.org").unwrap().created += 1;
    assert_ne!(base, h(later), "created_at is in the hash");
    let mut later = input(&files);
    later.file_times.get_mut("a.org").unwrap().modified += 1;
    assert_ne!(base, h(later), "modified_at is in the hash");
}

#[test]
fn the_times_come_from_git_or_from_the_receipt_time() {
    let mut inp = input(&[("a.org", ok_page("a")), ("b.org", ok_page("b"))]);
    inp.file_times.remove("b.org");
    let plan = plan_entries(inp, &opts()).unwrap();
    let (a, b) = (&plan.nodes[0], &plan.nodes[1]);
    assert_eq!(
        (a.created_at, a.modified_at, a.times_from_git),
        (T0, T0 + 5, true)
    );
    assert_eq!(
        (b.created_at, b.modified_at, b.times_from_git),
        (42, 42, false)
    );
    let json = render_json(&plan, &[]);
    assert_eq!(json["nodes"][0]["created_at"], T0);
    assert_eq!(json["nodes"][0]["modified_at"], T0 + 5);
    assert_eq!(json["nodes"][0]["created_at_source"], "git");
    assert_eq!(json["nodes"][1]["created_at_source"], "received");
    assert_eq!(json["nodes"][1]["modified_at_source"], "received");
    assert_eq!(json["counts"]["times_received"], 1);
    assert!(render_text(&plan).contains("from git for 1, receipt time for 1 (not reproducible)"));
}

/// `--code-property`: the code is reported per node (case-insensitive key);
/// a page without one is listed, not refused.
#[test]
fn document_codes_are_reported_and_missing_ones_listed() {
    let coded = page(&[("ID", "a"), ("Codigo", "PRO-1")], Some("T"), None, "x\n");
    let empty = page(&[("ID", "b"), ("CODIGO", "")], Some("T"), None, "x\n");
    let files = [
        ("a.org", coded.into_bytes()),
        ("b.org", empty.into_bytes()),
        ("c.org", ok_page("c")),
    ];
    let mut o = opts();
    o.code_property = Some("CODIGO".into());
    let plan = plan_entries(input(&files), &o).unwrap();
    assert!(plan.ok());
    assert_eq!(plan.nodes[0].code.as_deref(), Some("PRO-1"));
    assert_eq!(plan.missing_code, vec!["b.org", "c.org"]);
    assert!(render_text(&plan)
        .contains("== missing document code (--code-property CODIGO) (2) ==\nb.org\nc.org\n"));
    let without = plan_entries(input(&files), &opts()).unwrap();
    assert!(without.missing_code.is_empty() && without.nodes[0].code.is_none());
}

#[test]
fn an_empty_selection_is_refused() {
    let mut o = opts();
    o.include = vec!["notes/**/*.org".into()];
    let plan = plan_entries(input(&[("a.org", ok_page("a"))]), &o).unwrap();
    assert_eq!(
        refused(&plan),
        vec![(String::new(), ProblemKind::NothingSelected)]
    );
}

#[test]
fn the_json_report_carries_every_section() {
    let plan = plan_entries(input(&[("a.org", ok_page("ens:a"))]), &opts()).unwrap();
    let j = render_json(&plan, &[]);
    for key in [
        "contract",
        "dry_run",
        "ok",
        "plan_hash",
        "source",
        "options",
        "counts",
        "nodes",
        "refusals",
        "excluded",
        "non_canonical",
        "not_selected",
        "missing_code",
    ] {
        assert!(j.get(key).is_some(), "missing {key}");
    }
    assert_eq!(j["contract"], CONTRACT);
    assert_eq!(j["nodes"][0]["id"], "ens:a");
    assert_eq!(
        j["nodes"][0]["properties"],
        serde_json::json!([["ID", "ens:a"]])
    );
    assert_eq!(j["plan_hash"].as_str().unwrap().len(), 64);
}
