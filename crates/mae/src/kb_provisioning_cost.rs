//! Phase 0 evidence gate: what does building MAE's system KBs at runtime cost,
//! and what does the mechanism it would replace cost?
//!
//! The decision this informs — whether MAE can build its own bundled corpora on
//! the user's machine instead of shipping pre-built stores — should be settled
//! by measurement rather than taste, following ADR-102's evidence-gated
//! precedent.
//!
//! # Why the question is worth measuring
//!
//! Shipping pre-built stores carries real costs: they are 53–159x their source
//! text; they are sled, which the sqlite-only daemon cannot open at all and
//! which `kb_storage_engine`'s sqlite default migrates in place on first
//! registration; they are rewritten on first open, so an installed store can
//! never be checksum-verified; and they are absent entirely on Windows, in the
//! Docker image, and under `cargo install`.
//!
//! Building from source at first run removes all of that — **if** it is fast
//! enough. The bar is the startup watchdog (`crate::watchdog`), which trips at
//! ~10s.
//!
//! # Running it
//!
//! ```text
//! cargo test -p mae --bin mae -- --ignored --nocapture kb_provisioning_cost
//! ```
//!
//! `#[ignore]`d because these are measurements, not assertions: they take
//! seconds and their numbers are machine-specific. They live here as tests
//! rather than as a throwaway script so the same numbers can be taken on macOS
//! and on a 2-core runner through the existing CI, instead of being claimed
//! from one developer's Linux box (principle #13 — cross-platform parity is a
//! development constraint, not an afterthought).
//!
//! **No figure from these runs belongs in prose.** Record them in the ADR that
//! consumes them, dated, with the machine named.

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn dir_size(path: &Path) -> u64 {
        if path.is_file() {
            return std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        }
        let Ok(entries) = std::fs::read_dir(path) else {
            return 0;
        };
        entries
            .filter_map(|e| e.ok())
            .map(|e| dir_size(&e.path()))
            .sum()
    }

    /// Measurements 1 and 3: wall-clock and on-disk size, per corpus per engine.
    ///
    /// Driven off the real catalog rather than a hand-listed set, so a corpus
    /// added later is measured without anyone remembering to add it here.
    /// Entries without an org corpus are reported as skipped rather than
    /// silently omitted — the ADR corpus is `.md`, built by a different
    /// generator, and is `auto_enable: false`, so it never bears on first-run
    /// cost anyway.
    #[test]
    #[ignore = "measurement, not an assertion; run with --ignored --nocapture"]
    fn measure_corpus_build_cost_per_engine() {
        println!("\n=== KB build cost (org corpus -> store) ===");
        println!(
            "{:<16} {:<8} {:>9} {:>10} {:>8}  detail",
            "corpus", "engine", "build", "store", "source"
        );

        for kb in mae_kb::system_kb::SYSTEM_KBS {
            let src = workspace_root().join(kb.corpus_dir);
            if !src.join("index.org").exists() {
                println!("{:<16} (skipped: no org corpus)", kb.name);
                continue;
            }
            let src_bytes = dir_size(&src);

            for engine in ["sqlite", "sled"] {
                let out = tempfile::tempdir().unwrap();
                let db = out.path().join("measured.cozo");

                let start = Instant::now();
                let result = mae_kb::kb_build::build_org_kb(
                    &src,
                    &db,
                    &mae_kb::kb_build::OrgKbBuildOptions {
                        engine,
                        ..Default::default()
                    },
                );
                let elapsed = start.elapsed();

                match result {
                    Ok(stats) => println!(
                        "{:<16} {engine:<8} {:>8.3}s {:>9}K {:>7}K  {} nodes, {} links",
                        kb.name,
                        elapsed.as_secs_f64(),
                        dir_size(&db) / 1024,
                        src_bytes / 1024,
                        stats.nodes,
                        stats.typed_links_stored,
                    ),
                    Err(e) => println!("{:<16} {engine:<8} FAILED: {e}", kb.name),
                }
            }
        }
    }

    /// The decisive measurement: the manual KB's **full** pipeline, per engine.
    ///
    /// [`measure_corpus_build_cost_per_engine`] above measures only the org
    /// half. The shipped manual store is roughly 1200 nodes, not the 237 in
    /// `assets/manual`, because `build-manual-kb` first persists the
    /// code-generated corpus (`kb_seed`, derived from the live command / option
    /// / keymap / hook registries) and *then* upserts the hand-written org
    /// prose over it. Timing only the org half understates the real cost by
    /// most of it — and the manual is the largest corpus, so it is the one that
    /// decides whether first-run provisioning fits under the watchdog.
    #[test]
    #[ignore = "measurement, not an assertion; run with --ignored --nocapture"]
    fn measure_full_manual_pipeline_per_engine() {
        println!("\n=== manual KB, FULL pipeline (code-gen seed + org upsert) ===");

        let commands = mae_core::commands::CommandRegistry::with_builtins();
        let keymaps = mae_core::Editor::default_keymaps();
        let hooks = mae_core::hooks::HookRegistry::new();

        let src = workspace_root().join("assets/manual");
        // "mem" is the one that actually matters for the manual KB: it is
        // loaded into an in-memory store today (bootstrap opens `open_mem()`),
        // and keeping it that way is the design. sqlite/sled are measured for
        // comparison, not because the manual needs a durable store.
        for engine in ["mem", "sqlite", "sled"] {
            let out = tempfile::tempdir().unwrap();
            let db = out.path().join("manual.cozo");

            let start = Instant::now();
            let kb = mae_core::kb_seed::seed_kb(&commands, &keymaps, &hooks);
            let seeded = kb.len();
            let after_seed = start.elapsed();

            let store = mae_kb::kb_build::open_fresh_store(&db, engine).expect("open");
            store.persist_nodes(&kb).expect("persist");
            let after_persist = start.elapsed();

            let org = mae_kb::kb_build::ingest_org_dir(&store, &src, mae_kb::NodeSource::Seed)
                .expect("ingest");
            let total = start.elapsed();

            println!(
                "{engine:<8} total {:>7.3}s  (seed {:.3}s + persist {:.3}s + org {:.3}s)  \
                 store {:>7}K  {seeded} seed + {} org nodes",
                total.as_secs_f64(),
                after_seed.as_secs_f64(),
                (after_persist - after_seed).as_secs_f64(),
                (total - after_persist).as_secs_f64(),
                dir_size(&db) / 1024,
                org.nodes,
            );
        }
        println!(
            "\nThe watchdog (crate::watchdog) trips at ~10s. Provisioning happens once per \
             version, off the main thread; the figure to compare is the slowest of these \
             against that bar, with headroom for slower hardware."
        );
    }

    /// Measurement 4 — the one nobody has taken: what today's mechanism costs.
    ///
    /// [`crate::manual_kb::load_nodes_readonly`] recursively copies the entire
    /// pre-built manual store into `$TMPDIR`, opens it, `load_all()`s, and
    /// deletes the copy — **on every startup**. If building the corpus from
    /// ~240K of org text is cheaper than that, the "is provisioning fast
    /// enough?" question is moot: the change is a strict improvement on its own
    /// hot path rather than a cost to be justified.
    ///
    /// Reports the minimum of several runs. This is I/O-bound, so the fastest
    /// observed run is the one least contaminated by unrelated load — and a
    /// minimum is the conservative choice here, because it flatters the
    /// mechanism being argued *against*.
    ///
    /// Skips rather than fails when no pre-built store is present: the stores
    /// are gitignored build artifacts, so a clean checkout legitimately has
    /// none.
    #[test]
    #[ignore = "measurement, not an assertion; run with --ignored --nocapture"]
    fn measure_the_cost_the_change_would_replace() {
        println!("\n=== today's per-startup manual-KB load ===");

        let store = workspace_root().join("assets/mae-manual.cozo");
        // `exists()` is not enough, and assuming it was cost a measurement:
        // switching between a branch where the store is tracked and one where
        // it is deleted leaves the directory behind with only an empty `blobs/`
        // inside. That reported "0K, 0 nodes, 0.016s" as though it were a real
        // result — a timing that would have flattered the very mechanism this
        // test exists to weigh against. Require actual content.
        if dir_size(&store) < 1024 {
            println!(
                "(skipped: no usable pre-built manual store at {} — `make manual-kb` to measure)",
                store.display()
            );
            return;
        }
        println!("store on disk: {}K", dir_size(&store) / 1024);

        let mut best = Duration::MAX;
        let mut count = 0usize;
        for _ in 0..3 {
            let start = Instant::now();
            match crate::manual_kb::load_nodes_readonly(&store) {
                Ok(nodes) => {
                    best = best.min(start.elapsed());
                    count = nodes.len();
                }
                Err(e) => {
                    println!("FAILED: {e}");
                    return;
                }
            }
        }
        assert!(
            count > 0,
            "loaded 0 nodes from a non-empty store — the measurement is invalid, not fast"
        );
        println!(
            "load_nodes_readonly: {:.3}s (best of 3), {count} nodes",
            best.as_secs_f64()
        );
        println!(
            "\nCompare against the manual row above: if building from org source is cheaper \
             than this, runtime provisioning is faster than what it replaces."
        );
    }
}
