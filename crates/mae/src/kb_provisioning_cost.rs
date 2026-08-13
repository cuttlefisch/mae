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
//!
//! # What this harness got wrong, and why it now measures the real function
//!
//! It measured a *proxy* for startup rather than startup, and the gap between
//! the two is how a wrong number came to be trusted (issue #713).
//!
//! [`measure_corpus_build_cost_per_engine`] times `build_org_kb` alone. That
//! produced the "0.021s (MaePractices) + 0.198s (DevPractices), ~0.22s
//! combined" figure which was then cited in `bootstrap` to justify doing this
//! work synchronously on every platform's startup path. Two things were missing
//! from it: the `kb_open_instance_store` open that follows every build, and any
//! platform other than the fast Linux box it was run on. Windows — which since
//! #706 builds these corpora on *every* first launch, having previously shipped
//! none — paid roughly double the GUI startup time, and the window-appears test
//! started failing intermittently against its 120s bound.
//!
//! So [`measure_real_guidance_provisioning`] now times
//! `bootstrap::provision_guidance_stores` — the actual function `init_kb_federation`
//! calls — instead of a stand-in for it. A harness whose numbers justify a
//! design decision has to measure the thing being decided about.

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Instant;

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

    /// **The decisive measurement, and the one that was missing.**
    ///
    /// Times `bootstrap::provision_guidance_stores` — the real function, doing
    /// locate-or-build *and* the engine-aware open that follows — on a clean
    /// cache, which is what a first launch after an upgrade actually faces.
    ///
    /// Compare against the ~0.22s that
    /// [`measure_corpus_build_cost_per_engine`] reported for the build half
    /// alone. If the two disagree materially on this machine, that gap is the
    /// one that put guidance provisioning on the startup critical path and kept
    /// it there (#713).
    ///
    /// Run this on every platform before moving work back onto startup.
    #[test]
    #[ignore = "measurement, not an assertion; run with --ignored --nocapture"]
    fn measure_real_guidance_provisioning() {
        println!("\n=== guidance provisioning, THE REAL PATH (locate-or-build + open) ===");

        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        // Cold cache, and no installed store to short-circuit to: the state a
        // fresh install or a post-upgrade first launch is in.
        let _lock = mae_effect_sandbox::lock_env();
        let prev = std::env::var("XDG_CACHE_HOME").ok();
        std::env::set_var("XDG_CACHE_HOME", cache.path());

        // Mirrors what `init_kb_federation` hands the thread: the env lookup
        // resolved on the caller's side.
        let located = || -> Vec<(&'static mae_kb::system_kb::SystemKb, Option<PathBuf>)> {
            mae_kb::system_kb::auto_enabled()
                .filter(|kb| kb.name != mae_kb::system_kb::MANUAL)
                .map(|kb| (kb, crate::guidance_kb_engine::locate(kb, data.path())))
                .collect()
        };

        let start = Instant::now();
        let built = crate::bootstrap::provision_guidance_stores(data.path(), "sqlite", located());
        let elapsed = start.elapsed();

        // Warm: the second launch on the same version, which should hit the
        // version-keyed cache and skip the build entirely.
        let warm_start = Instant::now();
        let warm = crate::bootstrap::provision_guidance_stores(data.path(), "sqlite", located());
        let warm_elapsed = warm_start.elapsed();

        match prev {
            Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
            None => std::env::remove_var("XDG_CACHE_HOME"),
        }

        println!(
            "cold cache: {:>7.3}s for {} store(s): {}",
            elapsed.as_secs_f64(),
            built.len(),
            built
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!(
            "warm cache: {:>7.3}s for {} store(s)",
            warm_elapsed.as_secs_f64(),
            warm.len()
        );
        println!(
            "\nThis is what `init_kb_federation` used to do BEFORE the window \
             appeared. It now runs on a background thread (#713). Compare the cold \
             figure against the ~0.22s the build-only measurement below reported \
             on one Linux machine — and against this platform's own numbers, not \
             another's, before ever moving it back."
        );
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
}
