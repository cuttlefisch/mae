//! Real subprocess e2e for `build-manual-kb`/`build-practices-kb`'s
//! `NodeSource::Seed` stamping (found via live QA testing, K1 of the
//! post-ship quality pass).
//!
//! Every existing AI-residency test (`ai_residency.rs`'s own suite) exercises
//! the exemption logic against synthetic fixtures pre-stamped with
//! `.with_source(Seed, 1)` -- none of them exercise the real
//! `build-manual-kb`/`build-practices-kb` binaries' actual output. That's
//! exactly the gap that let a real bug ship: both binaries stamped
//! code-generated nodes via `kb_seed::seed_kb` but never stamped the
//! org-file-parsed nodes before `insert_node`, silently breaking the
//! ADR-048 residency exemption for `assets/manual/index.org` (which shares
//! its `:ID:` with a code-generated Seed node and so got silently
//! overwritten unstamped) and any manual page with no code-generated
//! counterpart at all (e.g. `assets/manual/concept-scheme-api.org`).
//!
//! This test spawns the real compiled `build-manual-kb`/`build-practices-kb`
//! binaries (`env!("CARGO_BIN_EXE_...")`, sibling bin targets in this same
//! crate) against a small real fixture `assets/manual/`/`assets/practices/`
//! directory -- one file whose `:ID:` collides with a real code-generated
//! seed node id (`index`), one with a fresh id that has no code-generated
//! counterpart -- then opens the resulting real CozoDB store and asserts
//! every persisted node carries `source == Some(NodeSource::Seed)`.

use std::process::Command;

use mae_kb::{CozoKbStore, KbStore, NodeSource};

fn run_builder(bin: &str, cwd: &std::path::Path, output_rel: &str) {
    let output = Command::new(bin)
        .arg(output_rel)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|e| panic!("failed to run {bin}: {e}"));
    assert!(
        output.status.success(),
        "{bin} failed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn build_manual_kb_stamps_seed_source_on_every_org_parsed_node() {
    let tmp = tempfile::tempdir().unwrap();
    let manual_dir = tmp.path().join("assets/manual");
    std::fs::create_dir_all(&manual_dir).unwrap();

    // Collides with the real code-generated `index` node
    // (crates/core/src/kb_seed/mod.rs's `Node::new("index", ...)`) --
    // the exact overwrite scenario that shipped unstamped.
    std::fs::write(
        manual_dir.join("index.org"),
        ":PROPERTIES:\n:ID: index\n:KIND: index\n:END:\n#+title: Test Index\n\nBody.\n",
    )
    .unwrap();

    // A fresh id with no code-generated counterpart at all -- the
    // concept-scheme-api.org scenario.
    std::fs::write(
        manual_dir.join("fresh-page.org"),
        ":PROPERTIES:\n:ID: concept:fresh-fixture-page\n:KIND: concept\n:END:\n#+title: Fresh Fixture Page\n\nBody.\n",
    )
    .unwrap();

    let output_rel = "test-manual.cozo";
    run_builder(
        env!("CARGO_BIN_EXE_build-manual-kb"),
        tmp.path(),
        output_rel,
    );

    let store = CozoKbStore::open(tmp.path().join(output_rel)).expect("failed to open built KB");

    let index_node = store
        .get_node("index")
        .expect("get_node failed")
        .expect("expected `index` node to exist after build");
    assert_eq!(
        index_node.source,
        Some(NodeSource::Seed),
        "the org-parsed `index.org` must stamp NodeSource::Seed, matching the \
         code-generated node it overwrites -- not silently lose provenance"
    );

    let fresh_node = store
        .get_node("concept:fresh-fixture-page")
        .expect("get_node failed")
        .expect("expected fresh org-only node to exist after build");
    assert_eq!(
        fresh_node.source,
        Some(NodeSource::Seed),
        "an org-parsed node with no code-generated counterpart must still be \
         stamped NodeSource::Seed, or it can never pass the ADR-048 residency \
         exemption for an unauthenticated MCP requester"
    );
}

#[test]
fn build_practices_kb_stamps_seed_source_on_every_org_parsed_node() {
    let tmp = tempfile::tempdir().unwrap();
    let practices_dir = tmp.path().join("assets/practices");
    std::fs::create_dir_all(&practices_dir).unwrap();

    // build-practices-kb requires a literal `index` node id (panics otherwise
    // -- guidance.rs::read_guidance_kb_context looks up exactly that id).
    std::fs::write(
        practices_dir.join("index.org"),
        ":PROPERTIES:\n:ID: index\n:KIND: index\n:END:\n#+title: Test Practices Index\n\nBody.\n",
    )
    .unwrap();
    std::fs::write(
        practices_dir.join("fresh-page.org"),
        ":PROPERTIES:\n:ID: practice:fresh-fixture-page\n:KIND: concept\n:END:\n#+title: Fresh Fixture Practice\n\nBody.\n",
    )
    .unwrap();

    let output_rel = "test-practices.cozo";
    run_builder(
        env!("CARGO_BIN_EXE_build-practices-kb"),
        tmp.path(),
        output_rel,
    );

    let store = CozoKbStore::open(tmp.path().join(output_rel)).expect("failed to open built KB");

    for id in ["index", "practice:fresh-fixture-page"] {
        let node = store
            .get_node(id)
            .expect("get_node failed")
            .unwrap_or_else(|| panic!("expected `{id}` node to exist after build"));
        assert_eq!(
            node.source,
            Some(NodeSource::Seed),
            "org-parsed node `{id}` must stamp NodeSource::Seed or it can never \
             pass the ADR-048 residency exemption"
        );
    }
}
