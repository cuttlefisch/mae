//! Phase 0 proving spike for Browser MAE (ADR-097): **can a browser Yjs client
//! bind to a real `KbNodeDoc` and converge with native MAE writers?**
//!
//! Everything in the browser-KB design assumes this. If UTF-16 offsets, the
//! yrs v1 update format, or the nested shared-type layout do not survive the
//! crossing into stock `yjs`, the design changes shape — so this is
//! deliberately the first thing built and it is built to *fail*, not to
//! reassure.
//!
//! Shape borrowed from `crates/export/tests/browser/`, the repo's established
//! Layer-2 pattern: Rust owns the fixtures and every assertion, a Node process
//! plays the untrusted other runtime. The Node half (`tests/browser/driver.mjs`)
//! uses the stock `yjs` package with no MAE code and asserts nothing itself, so
//! it cannot launder a Rust-side bug into a pass.
//!
//! Skipped (not failed) when the Node harness is absent, matching how the repo
//! treats optional external tooling — run `npm install` in
//! `shared/sync/tests/browser/` to enable. `interop_harness_is_present` reports
//! which mode the suite ran in so a silent all-skip cannot masquerade as green.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use mae_sync::kb::KbNodeDoc;

/// A node body with content chosen to break naive implementations rather than
/// flatter them: a non-BMP emoji (2 UTF-16 code units, 4 UTF-8 bytes), CJK
/// (1 UTF-16 unit, 3 UTF-8 bytes), an org typed link, and a `:PROPERTIES:`
/// drawer of the kind `shared/kb/src/org.rs` really stores inside bodies.
const BODY: &str = "\
:PROPERTIES:
:ID: spike-node-1
:END:

First paragraph with an emoji 🎉 and CJK 日本語 inline.
A typed link: [[id:other-node][another note]].

Second paragraph, untouched by every writer in these tests.";

const TITLE: &str = "Spike node — 🎉 unicode title";

fn harness_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("browser")
}

/// `Some(dir)` when the Node harness can actually run. Deliberately checks for
/// the installed dependency, not just the script — a present `driver.mjs` with
/// no `node_modules` would otherwise fail with an import error that reads like
/// a real interop failure.
fn harness() -> Option<PathBuf> {
    let dir = harness_dir();
    if !dir.join("driver.mjs").is_file() || !dir.join("node_modules/yjs").is_dir() {
        return None;
    }
    Command::new("node").arg("--version").output().ok()?;
    Some(dir)
}

/// What the Node side reports it can see. Mirrors `driver.mjs::observe`.
#[derive(Debug, serde::Deserialize)]
struct Observed {
    id: Option<String>,
    schema_v: Option<serde_json::Value>,
    kind: Option<String>,
    title: Option<String>,
    body: Option<String>,
    tags: Option<Vec<String>>,
    links: Option<Vec<String>>,
    aliases: Option<Vec<String>>,
    props: Option<HashMap<String, String>>,
    types: HashMap<String, Option<String>>,
}

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    /// Write a real `KbNodeDoc`'s full v1 state where the Node side can read it.
    fn new(doc: &KbNodeDoc) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("base.bin"), doc.encode()).expect("write base.bin");
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn run(&self, harness: &Path, command: &str) {
        let out = Command::new("node")
            .arg(harness.join("driver.mjs"))
            .arg(command)
            .arg(self.path())
            .output()
            .expect("spawn node");
        assert!(
            out.status.success(),
            "node driver `{command}` failed: {}\n{}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout),
        );
    }

    fn read_json<T: serde::de::DeserializeOwned>(&self, name: &str) -> T {
        let raw = std::fs::read_to_string(self.path().join(name))
            .unwrap_or_else(|e| panic!("read {name}: {e}"));
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {name}: {e}"))
    }

    /// Ask the browser side to insert `insert` at UTF-16 offset `at` in the
    /// body, and return the diff it emits — exactly what a real client would
    /// send upstream.
    fn browser_edit(&self, harness: &Path, at: usize, insert: &str) -> Vec<u8> {
        std::fs::write(
            self.path().join("edit-spec.json"),
            serde_json::json!({ "at": at, "insert": insert }).to_string(),
        )
        .expect("write edit-spec.json");
        self.run(harness, "edit");
        std::fs::read(self.path().join("browser-update.bin")).expect("read browser-update.bin")
    }
}

fn base_doc() -> KbNodeDoc {
    KbNodeDoc::new_with_client_id(
        "spike-node-1",
        TITLE,
        BODY,
        &["spike".to_string(), "browser".to_string()],
        // A realistic derived client id, not `1` — `crates/core/tests/kb_sync_n_peer_e2e.rs`
        // documents `client_id = 1` stand-ins as the anti-pattern that let a real
        // convergence bug hide.
        0x0011_2233_4455,
    )
}

/// Full materialized state, not just `content_hash` (which covers only
/// title+body+tags). Convergence must hold for links/aliases/props too.
fn snapshot(doc: &KbNodeDoc) -> String {
    let m = doc.materialize();
    let mut props: Vec<_> = m.properties.into_iter().collect();
    props.sort();
    format!(
        "id={}|title={}|body={}|tags={:?}|links={:?}|aliases={:?}|props={:?}|kind={:?}|todo={:?}|prio={:?}",
        m.id, m.title, m.body, m.tags, m.links, m.aliases, props, m.kind, m.todo_state, m.priority
    )
}

#[test]
fn interop_harness_is_present() {
    // Not an assertion — a report. A suite where every real test silently
    // skipped is indistinguishable from a passing one otherwise.
    match harness() {
        Some(dir) => println!("browser interop harness active at {}", dir.display()),
        None => println!(
            "browser interop harness ABSENT — every interop test below skipped. \
             Run `npm install` in shared/sync/tests/browser/ to enable."
        ),
    }
}

/// The base falsification: stock `yjs`, given a real `KbNodeDoc`'s v1 state,
/// must see the *same content* AND the *same shared types*. Checking only the
/// stringified content would pass on a degraded decode that flattened Y.Text to
/// a plain string — which would be unusable for collaborative editing and is
/// precisely the failure this spike exists to catch.
#[test]
fn browser_reads_a_real_kb_node_doc_as_live_shared_types() {
    let Some(h) = harness() else { return };

    let doc = base_doc();
    let fx = Fixture::new(&doc);
    fx.run(&h, "read");
    let seen: Observed = fx.read_json("observed.json");

    assert_eq!(seen.id.as_deref(), Some("spike-node-1"));
    assert_eq!(seen.title.as_deref(), Some(TITLE), "title must round-trip");
    assert_eq!(seen.body.as_deref(), Some(BODY), "body must round-trip");
    assert_eq!(
        seen.tags.as_deref(),
        Some(&["spike".to_string(), "browser".to_string()][..])
    );
    assert_eq!(seen.links.as_deref(), Some(&[][..]));
    assert_eq!(seen.aliases.as_deref(), Some(&[][..]));
    assert_eq!(seen.props, Some(HashMap::new()));

    // The load-bearing part: these must be live CRDT types.
    assert_eq!(seen.types.get("title"), Some(&Some("YText".to_string())));
    assert_eq!(seen.types.get("body"), Some(&Some("YText".to_string())));
    assert_eq!(seen.types.get("tags"), Some(&Some("YArray".to_string())));
    assert_eq!(seen.types.get("props"), Some(&Some("YMap".to_string())));
}

/// **Finding from this spike.** A browser client must NOT branch on `schema_v`
/// to decide whether a node has ADR-093 v2 semantics.
///
/// `schema_v` is stamped *lazily* — only by a v2 setter (`set_kind`,
/// `set_aliases`, `set_properties`, … via the `scalar`/array/map setters in
/// `shared/sync/src/kb/node.rs`). `KbNodeDoc::new` eagerly seeds the v2
/// *containers* (`aliases`, `props`) per ADR-093 D4 but never stamps the
/// version, so a freshly created node is structurally v2 and reports v1.
///
/// That is defensible on the Rust side, where every reader already tolerates an
/// absent key and `schema_version()` returns 1 by design. It is a trap for a
/// second runtime writing a reader from scratch, which is exactly what the
/// browser is. The rule this test pins: **probe the container, never the
/// marker** — and the marker, once present, is trustworthy.
#[test]
fn a_browser_must_probe_containers_not_the_schema_marker() {
    let Some(h) = harness() else { return };

    // A node created the ordinary way: v2 containers present, marker absent.
    let fresh = base_doc();
    let fx = Fixture::new(&fresh);
    fx.run(&h, "read");
    let seen: Observed = fx.read_json("observed.json");

    assert_eq!(
        seen.schema_v, None,
        "a freshly created node is expected to carry no schema marker — if this now fails, \
         the lazy-stamping behavior changed and the browser reader's contract can be simplified"
    );
    assert_eq!(fresh.schema_version(), 1, "Rust agrees it reads as v1");
    // …yet the v2 containers a browser needs are already there and usable.
    assert_eq!(seen.aliases.as_deref(), Some(&[][..]));
    assert_eq!(seen.props, Some(HashMap::new()));

    // Once any v2 field is actually set, the marker appears and IS reliable.
    let mut stamped = base_doc();
    let _ = stamped.set_kind(Some("note"));
    let fx2 = Fixture::new(&stamped);
    fx2.run(&h, "read");
    let seen2: Observed = fx2.read_json("observed.json");

    assert_eq!(
        seen2.schema_v,
        Some(serde_json::json!("2")),
        "a node carrying a real v2 field must expose the marker to the browser"
    );
    assert_eq!(seen2.kind.as_deref(), Some("note"));
}

/// A browser edit expressed at a **UTF-16** offset must land at the same place
/// Rust sees it. The insertion point is chosen to sit immediately after the
/// non-BMP emoji, so a byte- or char-offset implementation lands somewhere else
/// and this fails loudly.
#[test]
fn a_browser_edit_at_a_utf16_offset_lands_where_rust_expects_it() {
    let Some(h) = harness() else { return };

    let marker = "🎉";
    let at_utf16 = BODY
        .split(marker)
        .next()
        .map(|prefix| prefix.encode_utf16().count() + marker.encode_utf16().count())
        .expect("marker present in body");

    let mut doc = base_doc();
    let fx = Fixture::new(&doc);
    let update = fx.browser_edit(&h, at_utf16, "<<INSERTED>>");

    doc.apply_update(&update).expect("apply browser update");

    let expected = BODY.replacen(marker, &format!("{marker}<<INSERTED>>"), 1);
    assert_eq!(
        doc.body(),
        expected,
        "browser insert must land immediately after the emoji, not mid-codepoint or offset by \
         the emoji's extra UTF-8 bytes"
    );
}

/// N-way convergence with N=3 and **every apply order**, per principle #14 —
/// not a 2-writer happy path, and not one fixed order.
///
/// Writers: the browser, a native MAE editor, and an MCP client — three
/// distinct client ids editing the same node in one window.
#[test]
fn three_writers_converge_identically_under_every_apply_order() {
    let Some(h) = harness() else { return };

    let base = base_doc();
    let base_state = base.encode();

    // The browser's edit, produced by the real Node runtime.
    let fx = Fixture::new(&base);
    let browser_update = fx.browser_edit(&h, 0, "[browser] ");

    // Two native writers, each from its own client id, editing concurrently
    // (both branch from the same base state — they never see each other first).
    let native_update = {
        let mut d = KbNodeDoc::from_bytes_with_client_id(&base_state, 0x00AA_BBCC_DDEE)
            .expect("native doc");
        let sv = d.state_vector();
        let _ = d.set_body(&format!("{BODY}\n\n[native] appended by the GUI."));
        d.encode_diff(&sv).expect("native diff")
    };
    let mcp_update = {
        let mut d =
            KbNodeDoc::from_bytes_with_client_id(&base_state, 0x0099_8877_6655).expect("mcp doc");
        let sv = d.state_vector();
        let _ = d.add_tag("from-mcp");
        d.encode_diff(&sv).expect("mcp diff")
    };

    let updates = [
        ("browser", browser_update.as_slice()),
        ("native", native_update.as_slice()),
        ("mcp", mcp_update.as_slice()),
    ];

    // All 6 permutations of 3 updates.
    let orders = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];

    let mut converged: Option<String> = None;
    for order in orders {
        let mut doc = KbNodeDoc::from_bytes(&base_state).expect("fresh base");
        for i in order {
            doc.apply_update(updates[i].1).expect("apply");
        }
        let snap = snapshot(&doc);

        // Every writer's intent must survive — no silent loss.
        assert!(
            doc.body().contains("[browser] "),
            "browser edit lost in order {order:?}"
        );
        assert!(
            doc.body().contains("[native] appended by the GUI."),
            "native edit lost in order {order:?}"
        );
        assert!(
            doc.tags().contains(&"from-mcp".to_string()),
            "mcp edit lost in order {order:?}"
        );

        match &converged {
            None => converged = Some(snap),
            Some(first) => assert_eq!(
                first, &snap,
                "apply order {order:?} converged to a different state — CRDT convergence violated"
            ),
        }
    }
}

/// The #625 oracle: a concurrent merge must not duplicate the untouched base.
/// A whole-document-replace implementation passes a naive "both edits present"
/// check while silently doubling everything around them.
#[test]
fn merging_a_browser_edit_does_not_duplicate_the_untouched_base() {
    let Some(h) = harness() else { return };

    let base = base_doc();
    let base_state = base.encode();
    let fx = Fixture::new(&base);
    let browser_update = fx.browser_edit(&h, 0, "[browser] ");

    let mut doc = KbNodeDoc::from_bytes(&base_state).expect("fresh base");
    doc.apply_update(&browser_update).expect("apply");

    let sentinel = "Second paragraph, untouched by every writer in these tests.";
    assert_eq!(
        doc.body().matches(sentinel).count(),
        1,
        "untouched base paragraph was duplicated by the merge"
    );
    assert_eq!(
        doc.body().matches(":PROPERTIES:").count(),
        1,
        "the properties drawer was duplicated by the merge"
    );
}

/// Offline-then-reconnect: the browser edits from a *stale* base while a native
/// writer advances the document. Reconnecting must merge, never clobber.
#[test]
fn an_offline_browser_edit_does_not_clobber_a_concurrent_native_edit() {
    let Some(h) = harness() else { return };

    let base = base_doc();
    let base_state = base.encode();

    // Browser goes offline holding `base_state` and edits there.
    let fx = Fixture::new(&base);
    let offline_update = fx.browser_edit(&h, 0, "[offline-browser] ");

    // Meanwhile the native side advances, never having seen the browser.
    let mut server =
        KbNodeDoc::from_bytes_with_client_id(&base_state, 0x00AA_BBCC_DDEE).expect("server doc");
    let _ = server.set_body(&format!("{BODY}\n\n[native-while-offline] more text."));

    // Browser reconnects and its queued update is applied.
    server.apply_update(&offline_update).expect("apply offline");

    assert!(
        server.body().contains("[offline-browser] "),
        "the offline browser edit was dropped on reconnect"
    );
    assert!(
        server.body().contains("[native-while-offline] more text."),
        "the offline browser edit clobbered the concurrent native edit"
    );
}

/// Adversarial input: a hostile/corrupt update from a browser must be rejected
/// at the boundary as a structured error, never panic and never leave the
/// document half-mutated for a subsequent TUI reader.
#[test]
fn a_hostile_browser_update_is_rejected_without_corrupting_the_document() {
    let base = base_doc();
    let base_state = base.encode();
    let good = snapshot(&base);

    let hostile: &[&[u8]] = &[
        b"",
        b"not a yrs update at all",
        &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
        &[0x00, 0x00],
    ];

    for (i, bytes) in hostile.iter().enumerate() {
        let mut doc = KbNodeDoc::from_bytes(&base_state).expect("fresh base");
        // Must not panic. Whether a given malformed input errors or is a no-op
        // is the decoder's business; what must hold is that the document is
        // never left in a state a reader can't handle.
        let _ = doc.apply_update(bytes);
        assert_eq!(
            snapshot(&doc),
            good,
            "hostile update #{i} mutated the document"
        );
    }
}

/// **Negative control.** The convergence oracle above must be capable of
/// failing. Here the browser's update is deliberately withheld — the same
/// "prove the oracle has teeth" discipline `scripts/collab-encrypted-e2e.sh`
/// applies via `MAE_E2E_NEGATIVE=1`. If this test's assertion ever stops
/// holding, the positive tests above are worthless.
#[test]
fn the_convergence_oracle_detects_a_dropped_browser_update() {
    let Some(h) = harness() else { return };

    let base = base_doc();
    let base_state = base.encode();
    let fx = Fixture::new(&base);
    let browser_update = fx.browser_edit(&h, 0, "[browser] ");

    let applied = {
        let mut d = KbNodeDoc::from_bytes(&base_state).expect("fresh");
        d.apply_update(&browser_update).expect("apply");
        snapshot(&d)
    };
    let dropped = {
        let d = KbNodeDoc::from_bytes(&base_state).expect("fresh");
        snapshot(&d)
    };

    assert_ne!(
        applied, dropped,
        "the oracle cannot distinguish an applied browser update from a dropped one — \
         every convergence assertion in this file is vacuous"
    );
}
