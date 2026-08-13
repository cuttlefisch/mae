//! Tests for [`super`] — the shared build-time KB-asset pipeline.
//!
//! Extracted under CLAUDE.md's file-ceiling remedy when the staging/rename
//! rework pushed `kb_build.rs` past 800 lines. Follows the `watch.rs` /
//! `watch_tests.rs` precedent: `#[path]` adds a module level, so the inner
//! `mod tests` uses `use super::super::*`.

#[cfg(test)]
mod tests {
    use super::super::*;
    use tempfile::TempDir;

    fn write_org(dir: &Path, filename: &str, content: &str) {
        std::fs::write(dir.join(filename), content).expect("failed to write fixture org file");
    }

    // --- compute_db_checksum ---

    #[test]
    fn checksum_is_deterministic_across_repeated_calls() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"hello").unwrap();
        std::fs::write(tmp.path().join("b.txt"), b"world").unwrap();

        let first = compute_db_checksum(tmp.path());
        let second = compute_db_checksum(tmp.path());
        assert_eq!(
            first, second,
            "checksum of an unchanged directory must be stable across calls"
        );
    }

    /// Adversarial half of the determinism test: a real content change MUST
    /// change the checksum. A test that only asserts "it doesn't crash" or
    /// "two computations of the same input match" would pass even if the
    /// hasher silently ignored file contents.
    #[test]
    fn checksum_changes_when_file_content_changes() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"hello").unwrap();
        let before = compute_db_checksum(tmp.path());

        std::fs::write(tmp.path().join("a.txt"), b"hello, world").unwrap();
        let after = compute_db_checksum(tmp.path());

        assert_ne!(
            before, after,
            "changing a tracked file's bytes must change the checksum"
        );
    }

    #[test]
    fn checksum_of_single_file_matches_direct_read() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("single.cozo");
        std::fs::write(&file, b"some db bytes").unwrap();

        let mut hasher = Sha256::new();
        hasher.update(b"some db bytes");
        let expected = hex::encode(hasher.finalize());

        assert_eq!(compute_db_checksum(&file), expected);
    }

    #[test]
    fn write_checksum_sidecar_writes_expected_format() {
        let tmp = TempDir::new().unwrap();
        let output = tmp.path().join("mae-test.cozo");
        write_checksum_sidecar(&output, "deadbeef");

        let sha_path = output.with_extension("cozo.sha256");
        let contents = std::fs::read_to_string(&sha_path).unwrap();
        assert_eq!(contents, format!("deadbeef  {}\n", output.display()));
    }

    // --- ingest_org_dir ---

    const NODE_A: &str =
        ":PROPERTIES:\n:ID: test:a\n:END:\n#+title: Node A\n\nBody of A, links to [[test:b][B]].\n";
    const NODE_B: &str =
        ":PROPERTIES:\n:ID: test:b\n:END:\n#+title: Node B\n\nBody of B, no outgoing links.\n";
    const NODE_C: &str = ":PROPERTIES:\n:ID: test:c\n:END:\n#+title: Node C\n\nBody of C, links to [[test:a][A]] and [[test:b][B]].\n";

    #[test]
    fn ingest_org_dir_counts_nodes_and_links_exactly() {
        let src_dir = TempDir::new().unwrap();
        write_org(src_dir.path(), "a.org", NODE_A);
        write_org(src_dir.path(), "b.org", NODE_B);
        write_org(src_dir.path(), "c.org", NODE_C);

        let store = CozoKbStore::open_mem().expect("failed to open in-memory store");
        store.seed_type_system().expect("seed type system");

        let stats = ingest_org_dir(&store, src_dir.path(), NodeSource::Seed).expect("ingest ok");

        assert_eq!(stats.org_files, 3);
        assert_eq!(stats.nodes, 3, "exactly 3 nodes across the 3 fixture files");
        // a -> b, c -> a, c -> b = 3 typed links total, all should store cleanly.
        assert_eq!(stats.typed_links_parsed, 3);
        assert_eq!(stats.typed_links_stored, 3);
        assert_eq!(stats.transclusions_parsed, 0);
        assert_eq!(stats.transclusions_stored, 0);

        // Selective oracle: not just "> 0", confirm the actual node made it
        // into the store with the right id and content survived the round
        // trip (principle #14 — pin the meaningful outcome).
        let node_a = store
            .get_node("test:a")
            .expect("query ok")
            .expect("node test:a must exist after ingest");
        assert_eq!(node_a.title, "Node A");
    }

    #[test]
    fn ingest_org_dir_ignores_non_org_files() {
        let src_dir = TempDir::new().unwrap();
        write_org(src_dir.path(), "a.org", NODE_A);
        std::fs::write(src_dir.path().join("README.md"), "not an org file").unwrap();
        std::fs::write(src_dir.path().join("notes.txt"), "also not org").unwrap();

        let store = CozoKbStore::open_mem().expect("failed to open in-memory store");
        store.seed_type_system().expect("seed type system");

        let stats = ingest_org_dir(&store, src_dir.path(), NodeSource::Seed).expect("ingest ok");
        assert_eq!(stats.org_files, 1);
        assert_eq!(stats.nodes, 1);
    }

    /// The provenance stamp is the thing that makes shipped content read-only:
    /// `kb_update_node_with` refuses `Some(NodeSource::Seed)` and nothing else.
    /// Assert it on a node read back out of the store, not on the input.
    #[test]
    fn ingested_nodes_carry_the_requested_provenance() {
        let src_dir = TempDir::new().unwrap();
        write_org(src_dir.path(), "a.org", NODE_A);

        let store = CozoKbStore::open_mem().expect("failed to open in-memory store");
        store.seed_type_system().expect("seed type system");
        ingest_org_dir(&store, src_dir.path(), NodeSource::Seed).expect("ingest ok");

        let node = store
            .get_node("test:a")
            .expect("query ok")
            .expect("node must exist");
        assert_eq!(
            node.source,
            Some(NodeSource::Seed),
            "an unstamped node is editable, and the next rebuild silently destroys the edit"
        );
    }

    /// The same guarantee for the generator that does *not* go through
    /// `ingest_org_dir` — the ADR KB builds `.md`-sourced nodes and inserts
    /// them directly, which is exactly how it ended up with `source == None`.
    #[test]
    fn insert_nodes_stamps_provenance_on_nodes_that_never_saw_an_org_file() {
        let store = CozoKbStore::open_mem().expect("failed to open in-memory store");
        store.seed_type_system().expect("seed type system");

        let unstamped = Node::new(
            "concept:adr-999-fixture",
            "ADR-999",
            crate::NodeKind::Concept,
            "Body.",
        );
        assert_eq!(unstamped.source, None, "fixture must start unstamped");

        let inserted = insert_nodes(&store, std::slice::from_ref(&unstamped), NodeSource::Seed);
        assert_eq!(inserted, 1);

        let read_back = store
            .get_node("concept:adr-999-fixture")
            .expect("query ok")
            .expect("node must exist");
        assert_eq!(read_back.source, Some(NodeSource::Seed));
    }

    #[test]
    fn ingest_org_dir_errors_on_missing_directory() {
        let store = CozoKbStore::open_mem().expect("failed to open in-memory store");
        store.seed_type_system().expect("seed type system");
        let err = ingest_org_dir(
            &store,
            Path::new("/nonexistent/definitely-not-a-real-dir-9f3c"),
            NodeSource::Seed,
        )
        .expect_err("a missing corpus must not silently produce an empty KB");
        assert!(matches!(err, KbBuildError::MissingSourceDir(_)), "{err:?}");
    }

    #[test]
    fn ingest_org_dir_errors_on_zero_nodes() {
        let src_dir = TempDir::new().unwrap();
        // Directory exists but has no .org files.
        std::fs::write(src_dir.path().join("README.md"), "nothing to ingest").unwrap();

        let store = CozoKbStore::open_mem().expect("failed to open in-memory store");
        store.seed_type_system().expect("seed type system");
        let err = ingest_org_dir(&store, src_dir.path(), NodeSource::Seed)
            .expect_err("an empty corpus must not produce a shippable KB");
        assert!(matches!(err, KbBuildError::EmptyCorpus(_)), "{err:?}");
    }

    // --- require_index_node ---

    #[test]
    fn require_index_node_errors_on_fresh_store_with_no_index() {
        let store = CozoKbStore::open_mem().expect("failed to open in-memory store");
        store.seed_type_system().expect("seed type system");
        let err = require_index_node(&store, Path::new("assets/fixture-kb"))
            .expect_err("a guidance KB without an index node surfaces nothing at read time");
        assert!(matches!(err, KbBuildError::MissingIndexNode(_)), "{err:?}");
    }

    #[test]
    fn require_index_node_passes_when_index_node_present() {
        let store = CozoKbStore::open_mem().expect("failed to open in-memory store");
        store.seed_type_system().expect("seed type system");

        let src_dir = TempDir::new().unwrap();
        write_org(
            src_dir.path(),
            "index.org",
            ":PROPERTIES:\n:ID: index\n:END:\n#+title: Index\n\nEntry point.\n",
        );
        ingest_org_dir(&store, src_dir.path(), NodeSource::Seed).expect("ingest ok");

        require_index_node(&store, Path::new("assets/fixture-kb")).expect("index node present");
    }

    // --- build_org_kb ---

    /// The disk engine available in *this* build.
    ///
    /// `mae-kb`'s own default features are sled-only (`Cargo.toml`), so a bare
    /// `cargo test -p mae-kb` has no sqlite; the editor and daemon builds unify
    /// `storage-sqlite` in via `mae-core`/`daemon`. Selecting here rather than
    /// `#[cfg]`-ing the test away keeps the round-trip covered in both
    /// configurations — and the reason this matters at all is that the engine
    /// is a *runtime* string dispatch inside cozo: asking for an
    /// uncompiled engine fails with a `bail!` at open time, not at compile
    /// time, which is precisely why `open_fresh_store` takes it as a parameter.
    #[cfg(feature = "storage-sqlite")]
    const TEST_DISK_ENGINE: &str = "sqlite";
    #[cfg(not(feature = "storage-sqlite"))]
    const TEST_DISK_ENGINE: &str = "sled";

    /// The composed pipeline, built to disk and reopened independently.
    #[test]
    fn build_org_kb_produces_a_queryable_store_on_disk() {
        let src_dir = TempDir::new().unwrap();
        write_org(
            src_dir.path(),
            "index.org",
            ":PROPERTIES:\n:ID: index\n:END:\n#+title: Index\n\nEntry point.\n",
        );
        write_org(src_dir.path(), "a.org", NODE_A);

        let out = TempDir::new().unwrap();
        let output = out.path().join("fixture.cozo");
        let stats = build_org_kb(
            src_dir.path(),
            &output,
            &OrgKbBuildOptions {
                engine: TEST_DISK_ENGINE,
                ..OrgKbBuildOptions::default()
            },
        )
        .expect("build ok");
        assert_eq!(stats.nodes, 2);

        // Reopen independently: the point of building to disk is that another
        // process can read it back.
        let reopened = CozoKbStore::open_with_engine(&output, TEST_DISK_ENGINE).expect("reopen");
        let node = reopened
            .get_node("test:a")
            .expect("query ok")
            .expect("node must survive the round trip to disk");
        assert_eq!(node.title, "Node A");
        assert_eq!(node.source, Some(NodeSource::Seed));
    }

    /// The property the staging rename exists to guarantee: `output_path` must
    /// not exist until the build is finished.
    ///
    /// The oracle is how many times a concurrent watcher sees the path at all.
    /// Building in place, the file appears the instant the store opens and then
    /// fills for the rest of the build, so a watcher sees it over and over
    /// (~40-50 times here); staged, it can only ever appear once, at the rename.
    /// That distinction is the bug itself, not a proxy for it — every consumer
    /// decides a built store is usable from `Path::exists()` alone
    /// (`guidance::resolve_guidance_db_path`'s cache arm, and
    /// `build_guidance_from_embedded_corpus`'s early return), so a path that
    /// exists mid-build is a store handed out mid-build.
    ///
    /// Deliberately `<= 1` rather than `== 1`: a starved poller thread may never
    /// tick between the rename and the builder returning, and 0 sightings is the
    /// same guarantee holding. The failing direction is not close to the bound.
    #[test]
    fn the_final_path_does_not_exist_until_the_build_is_finished() {
        let src_dir = TempDir::new().unwrap();
        write_org(
            src_dir.path(),
            "index.org",
            ":PROPERTIES:\n:ID: index\n:END:\n#+title: Index\n\nEntry point.\n",
        );
        // Enough nodes that the build spans a meaningful number of poll ticks.
        for i in 0..60 {
            write_org(
                src_dir.path(),
                &format!("n{i}.org"),
                &format!(":PROPERTIES:\n:ID: test:n{i}\n:END:\n#+title: Node {i}\n\nBody {i}.\n"),
            );
        }

        let out = TempDir::new().unwrap();
        let output = out.path().join("fixture.cozo");
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        let poll_output = output.clone();
        let poll_done = done.clone();
        let poller = std::thread::spawn(move || {
            let mut sightings = 0usize;
            while !poll_done.load(std::sync::atomic::Ordering::Relaxed) {
                if poll_output.exists() {
                    sightings += 1;
                }
                std::thread::yield_now();
            }
            sightings
        });

        let stats = build_org_kb(
            src_dir.path(),
            &output,
            &OrgKbBuildOptions {
                engine: TEST_DISK_ENGINE,
                ..OrgKbBuildOptions::default()
            },
        )
        .expect("build ok");
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        let sightings = poller.join().expect("poller ok");

        assert_eq!(stats.nodes, 61);
        assert!(
            sightings <= 1,
            "the final path was visible {sightings} times during the build — it is \
             being built in place again, so readers can open a partial store"
        );
        // And the finished article is intact and complete.
        let reopened = CozoKbStore::open_with_engine(&output, TEST_DISK_ENGINE).expect("reopen");
        assert!(matches!(reopened.get_node("index"), Ok(Some(_))));
        assert!(matches!(reopened.get_node("test:n59"), Ok(Some(_))));
    }

    /// A build that fails validation must leave NOTHING at the final path — not
    /// the partial store it just built, and not a stale one from a prior run.
    /// Otherwise the failure poisons the cache: readers see a file, trust it,
    /// and the builder's early `exists()` return never rebuilds it.
    #[test]
    fn a_failed_build_leaves_no_store_and_no_staging_behind() {
        let src_dir = TempDir::new().unwrap();
        write_org(src_dir.path(), "a.org", NODE_A); // no index node

        let out = TempDir::new().unwrap();
        let output = out.path().join("fixture.cozo");
        build_org_kb(
            src_dir.path(),
            &output,
            &OrgKbBuildOptions {
                engine: TEST_DISK_ENGINE,
                require_index: true,
                ..OrgKbBuildOptions::default()
            },
        )
        .expect_err("a corpus with no index node must fail");

        assert!(
            !output.exists(),
            "a failed build must not leave a store at the final path"
        );
        let leftovers: Vec<String> = std::fs::read_dir(out.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            leftovers.is_empty(),
            "staging artifacts left behind: {leftovers:?}"
        );
    }

    #[test]
    fn build_org_kb_refuses_a_guidance_corpus_with_no_index_node() {
        let src_dir = TempDir::new().unwrap();
        write_org(src_dir.path(), "a.org", NODE_A);

        let out = TempDir::new().unwrap();
        let err = build_org_kb(
            src_dir.path(),
            &out.path().join("fixture.cozo"),
            &OrgKbBuildOptions {
                engine: TEST_DISK_ENGINE,
                ..OrgKbBuildOptions::default()
            },
        )
        .expect_err("require_index must be enforced by the composed pipeline too");
        assert!(matches!(err, KbBuildError::MissingIndexNode(_)), "{err:?}");
    }

    // --- open_fresh_store ---

    #[test]
    fn open_fresh_store_creates_parent_dirs_and_removes_stale_output() {
        let tmp = TempDir::new().unwrap();
        let output = tmp.path().join("nested").join("dir").join("out.cozo");

        // First open: parent dirs don't exist yet.
        {
            let _store = open_fresh_store(&output, RELEASE_ASSET_ENGINE).expect("open ok");
        }
        assert!(output.exists(), "store output should exist after opening");

        // Write a marker file inside the sled directory to prove the second
        // open really starts fresh (removes the old directory contents)
        // rather than reusing stale state.
        let marker = output.join("__marker__");
        if output.is_dir() {
            std::fs::write(&marker, b"stale").unwrap();
        }

        {
            let _store = open_fresh_store(&output, RELEASE_ASSET_ENGINE).expect("open ok");
        }
        if output.is_dir() {
            assert!(
                !marker.exists(),
                "open_fresh_store must remove the prior store before reopening"
            );
        }
    }
}
