//! ADR-092 D1: `kb_update_node_with` is the **sole node-content mutator**.
//!
//! It alone carries `kb_write_blocked`, thin-startup mirror hydration, owner
//! resolution across primary ∪ federated instances, the seed-node refusal, and
//! the `kb_sync_target` CRDT-vs-direct branch. The ADR states the rule plainly:
//! *"A new content-write path that does not [route through it] is a defect, not a
//! variant."*
//!
//! A rule stated in an ADR is a rule nobody enforces. This file enforces it.

/// Editor source files that may legitimately call `store.update_node` /
/// `store.insert_node` directly, with the reason.
///
/// **Every entry is a claim that this path is NOT a local content mutation.**
/// Adding one without that being true reintroduces exactly what D1 forbids.
const ALLOWED_DIRECT_WRITERS: &[(&str, &str)] = &[
    (
        "kb_ops/nodes.rs",
        "defines `kb_update_node_with` and `kb_create_node` — the sole mutator itself",
    ),
    (
        "kb_ops/sync.rs",
        "projects state RECEIVED from peers. Routing this through the mutator would \
         re-author CRDT ops for content that came FROM the CRDT — a loop, not a fix.",
    ),
    (
        "kb_ops/registry.rs",
        "bulk ingest/import of a whole instance, which is a store-level replace \
         rather than an edit to one node's content",
    ),
];

fn is_allowed(path: &str) -> Option<&'static str> {
    ALLOWED_DIRECT_WRITERS
        .iter()
        .find(|(p, _)| path.ends_with(p))
        .map(|(_, why)| *why)
}

/// **The guard.** No editor code may write node content directly except the
/// paths that have justified it above.
///
/// Scanned from source rather than asserted at runtime, because the property is
/// "this call does not appear", which no runtime test can observe. Same shape as
/// `query_plan_guard_tests` — the mechanism, not the sixteen instances.
/// Every `.rs` file under `src/editor`, as (relative path, contents).
fn editor_sources() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/editor");
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|x| x != "rs") {
                continue;
            }
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if let Ok(src) = std::fs::read_to_string(&path) {
                out.push((rel, src));
            }
        }
    }
    out
}

/// Direct node writes in `src`, as `line_no: text`, skipping `#[cfg(test)]`
/// modules.
///
/// Skipped by BRACE DEPTH, not by truncating at the first marker: three files in
/// this tree have production items AFTER an inline test module, so truncating
/// would have silently exempted them. See
/// `some_files_have_production_code_after_an_inline_test_module`.
fn direct_node_writes(src: &str) -> Vec<String> {
    let mut hits = Vec::new();
    let mut test_depth: Option<i32> = None;
    let mut pending_cfg_test = false;
    for (i, line) in src.lines().enumerate() {
        if let Some(d) = test_depth.as_mut() {
            *d += line.matches('{').count() as i32;
            *d -= line.matches('}').count() as i32;
            if *d <= 0 {
                test_depth = None;
            }
            continue;
        }
        if line.trim_start().starts_with("#[cfg(test)]") {
            pending_cfg_test = true;
            continue;
        }
        if pending_cfg_test {
            pending_cfg_test = false;
            let d = line.matches('{').count() as i32 - line.matches('}').count() as i32;
            if d > 0 {
                test_depth = Some(d);
                continue;
            }
        }
        let t = line.trim_start();
        if t.starts_with("//") {
            continue;
        }
        if line.contains(".update_node(") || line.contains(".insert_node(") {
            hits.push(format!("{}: {}", i + 1, t.trim()));
        }
    }
    hits
}

/// **The guard.** No editor code may write node content directly except the
/// paths that have justified it above.
///
/// Scanned from source rather than asserted at runtime, because the property is
/// "this call does not appear", which no runtime test can observe. Same shape as
/// `query_plan_guard_tests` — the mechanism, not the instances.
#[test]
fn no_editor_path_writes_node_content_outside_the_sole_mutator() {
    let mut offences = Vec::new();
    for (rel, src) in editor_sources() {
        if rel.contains("tests/") || rel.ends_with("_tests.rs") || is_allowed(&rel).is_some() {
            continue;
        }
        for hit in direct_node_writes(&src) {
            offences.push(format!("  {rel}:{hit}"));
        }
    }
    assert!(
        offences.is_empty(),
        "ADR-092 D1: node content must be written through `kb_update_node_with` \
         (or `kb_create_node`), which carries the write-policy check, seed-node \
         refusal, federated-owner resolution and the CRDT branch. These bypass \
         it:\n{}\n\nIf a path genuinely is not a local content mutation (e.g. it \
         projects peer state, or bulk-imports an instance), add it to \
         ALLOWED_DIRECT_WRITERS with the reason.",
        offences.join("\n")
    );
}

/// The allow-list must not rot into names nobody re-reads: every entry has to
/// name a file that still exists.
#[test]
fn every_allowed_direct_writer_still_exists() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/editor");
    for (rel, _why) in ALLOWED_DIRECT_WRITERS {
        assert!(
            root.join(rel).exists(),
            "ALLOWED_DIRECT_WRITERS names `{rel}`, which no longer exists — \
             delete the entry rather than leaving a stale exemption"
        );
    }
}

/// The scan skips `#[cfg(test)]` modules by BRACE DEPTH, and this pins why that
/// was necessary: three files in this tree have production items **after** an
/// inline test module (`file_ops.rs`'s `parse_file_link`, `mod.rs`'s
/// `rekey_after_remove`, `kb_ops/registry.rs`'s second `impl Editor`).
///
/// An earlier version truncated at the first `#[cfg(test)]` marker, which would
/// have silently exempted all of that production code from the guard. The hole
/// was found by asserting the assumption rather than relying on it — so this test
/// exists to stop anyone "simplifying" the scan back.
#[test]
fn some_files_have_production_code_after_an_inline_test_module() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/editor");
    let mut found = Vec::new();
    for rel in ["file_ops.rs", "mod.rs", "kb_ops/registry.rs"] {
        let Ok(src) = std::fs::read_to_string(root.join(rel)) else {
            continue;
        };
        let Some(at) = src.find("#[cfg(test)]") else {
            continue;
        };
        if src[at..].lines().skip(1).any(|l| {
            l.starts_with("pub fn ") || l.starts_with("pub(crate) fn ") || l.starts_with("impl ")
        }) {
            found.push(rel);
        }
    }
    assert!(
        !found.is_empty(),
        "no file has production code after an inline test module any more — the \
         brace-depth tracking in the scan above may look like over-engineering. \
         It was not: it is what stopped that code being exempt. Re-check before \
         simplifying."
    );
}
