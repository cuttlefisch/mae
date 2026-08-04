//! Tests for [`super`] — the shared inotify watcher.
//!
//! Extracted under CLAUDE.md's file-ceiling remedy (910 lines, ~321 of them
//! inline tests) after the one-instance-per-process rework. Declared with
//! `#[path]` from the parent; the indirection adds a module level, so the
//! inner `mod tests` uses `use super::super::*`.

#[cfg(test)]
mod tests {
    use super::super::*;
    use tempfile::TempDir;

    const SAMPLE: &str = ":PROPERTIES:\n:ID: abc-123\n:END:\n#+title: Test\nbody [[id:xyz]]\n";

    /// Build a routing table directly, without touching the OS — the routing
    /// decision is pure logic and deserves coverage that doesn't depend on
    /// scarce kernel resources (and that runs identically on macOS, where the
    /// `/proc`-based instance oracle can't).
    fn routes_over(roots: &[(&str, RegId)]) -> Routes {
        let mut routes = Routes::default();
        for (root, id) in roots {
            let root = PathBuf::from(root);
            routes.regs.insert(
                *id,
                Registration {
                    depth: root.components().count(),
                    root,
                    queue: VecDeque::new(),
                    errors: 0,
                },
            );
        }
        routes
    }

    /// Longest-prefix routing, adversarially: a nested registration must WIN
    /// over its ancestor (not merely also-receive), a sibling whose name
    /// shares a prefix must not match at all, and two registrations on the
    /// same root must both receive — the three ways first-match or naive
    /// string-prefix routing would misdeliver.
    #[test]
    fn routing_resolves_longest_prefix_not_first_match() {
        // /kb, /kb/inner, /kb/inn (a sibling whose name is a string prefix of
        // "inner"), and a second registration on /kb/inner (a tie).
        let routes = routes_over(&[
            ("/kb", 1),
            ("/kb/inner", 2),
            ("/kb/inn", 3),
            ("/kb/inner", 4),
        ]);
        // Sanity: ids 2 and 4 both registered /kb/inner (a tie, not an
        // overwrite) — otherwise the tie assertion below would be vacuous.
        assert_eq!(routes.regs.len(), 4);

        let mut deep = SharedDirWatcher::targets(&routes, &[PathBuf::from("/kb/inner/a.org")]);
        deep.sort_unstable();
        assert_eq!(
            deep,
            vec![2, 4],
            "a file under the inner root belongs to BOTH inner registrations \
             and to neither the ancestor (/kb) nor the string-prefix sibling (/kb/inn)"
        );

        let shallow = SharedDirWatcher::targets(&routes, &[PathBuf::from("/kb/top.org")]);
        assert_eq!(shallow, vec![1], "a file only under /kb stays with /kb");

        let sibling = SharedDirWatcher::targets(&routes, &[PathBuf::from("/kb/inn/x.org")]);
        assert_eq!(sibling, vec![3], "/kb/inn owns its own file");

        let outside = SharedDirWatcher::targets(&routes, &[PathBuf::from("/elsewhere/x.org")]);
        assert!(outside.is_empty(), "unwatched paths route nowhere");
    }

    /// A rename event carries two paths that can belong to two different
    /// registrations; both must be notified, each resolved independently.
    #[test]
    fn routing_unions_targets_across_an_events_paths() {
        let routes = routes_over(&[("/a", 1), ("/b", 2), ("/b/deep", 3)]);
        let mut t = SharedDirWatcher::targets(
            &routes,
            &[
                PathBuf::from("/a/from.org"),
                PathBuf::from("/b/deep/to.org"),
            ],
        );
        t.sort_unstable();
        assert_eq!(t, vec![1, 3]);
    }

    /// A watched file (`StoreWatcher`) inside a watched directory must keep
    /// its own events: the exact path is a strictly longer prefix match. This
    /// is what makes folding the store/registry watchers onto the same
    /// instance safe.
    #[test]
    fn an_exact_file_registration_beats_a_containing_directory() {
        let routes = routes_over(&[("/data", 1), ("/data/kb-registry.toml", 2)]);
        assert_eq!(
            SharedDirWatcher::targets(&routes, &[PathBuf::from("/data/kb-registry.toml")]),
            vec![2]
        );
        assert_eq!(
            SharedDirWatcher::targets(&routes, &[PathBuf::from("/data/other.org")]),
            vec![1]
        );
    }

    #[test]
    fn component_wise_prefix_never_matches_a_partial_name() {
        assert!(is_prefix_of(Path::new("/a/bc"), Path::new("/a/bc/d.org")));
        assert!(is_prefix_of(Path::new("/a/bc"), Path::new("/a/bc")));
        assert!(!is_prefix_of(Path::new("/a/bc"), Path::new("/a/bcd/e.org")));
        assert!(!is_prefix_of(Path::new("/a/bc/d"), Path::new("/a/bc")));
    }

    #[test]
    fn store_watcher_detects_external_modification() {
        // The basis of cross-instance freshness: another process modifying the shared
        // store file must be observable via drain_changed().
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("primary.cozo");
        std::fs::write(&path, b"v1").unwrap();
        let w = StoreWatcher::new(&path).unwrap();

        // Simulate another process committing to the store.
        std::fs::write(&path, b"v2-committed-by-another-process").unwrap();
        assert!(
            wait_for(|| w.drain_changed()),
            "store watcher must detect an external modification of the store file"
        );
    }

    #[test]
    fn watcher_reports_upsert_on_file_create() {
        let tmp = TempDir::new().unwrap();
        let w = OrgDirWatcher::new(tmp.path()).unwrap();

        let path = tmp.path().join("a.org");
        std::fs::write(&path, SAMPLE).unwrap();
        // The watcher emits normalized (canonical) paths so they match across
        // the /var → /private/var symlink on macOS; compare against canonical.
        let expected = path.canonicalize().unwrap();

        let got = wait_for(|| {
            w.drain()
                .iter()
                .any(|c| matches!(c, OrgChange::Upserted(p) if p == &expected))
        });
        assert!(got, "did not observe upsert for newly-created file");
    }

    #[test]
    fn watcher_ignores_non_org_files() {
        let tmp = TempDir::new().unwrap();
        let w = OrgDirWatcher::new(tmp.path()).unwrap();
        std::fs::write(tmp.path().join("notes.txt"), "ignore me").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let changes = w.drain();
        assert!(
            changes
                .iter()
                .all(|c| !matches!(c, OrgChange::Upserted(p) if p.extension().and_then(|e| e.to_str()) != Some("org"))),
            "non-org change leaked through: {changes:?}"
        );
    }

    /// The defect in `docs/INOTIFY_INSTANCE_EXHAUSTION.md`, pinned empirically:
    /// N watched directories must cost O(1) inotify *instances*, not N. Measured
    /// as a delta so a watcher some other test in this process already holds
    /// can't mask (or manufacture) a failure. Skipped where instances aren't a
    /// concept (macOS/FSEvents) rather than asserted blindly — cross-platform
    /// parity means "no false claim on the other OS", not "assert the Linux
    /// number everywhere".
    #[test]
    fn many_watched_dirs_cost_one_inotify_instance() {
        let Some(before) = inotify_instance_count() else {
            return; // not Linux: no per-user instance cap to exhaust
        };
        let dirs: Vec<TempDir> = (0..8).map(|_| TempDir::new().unwrap()).collect();
        let watchers: Vec<OrgDirWatcher> = dirs
            .iter()
            .map(|d| OrgDirWatcher::new(d.path()).unwrap())
            .collect();
        let after = inotify_instance_count().unwrap();
        let spent = after.saturating_sub(before);
        // The design bound is 1. The assertion allows 2 only to absorb a
        // concurrent test in this same binary dropping the last handle and
        // re-creating the shared watcher while this one measures — never to
        // tolerate per-directory growth, which starts at 8 here.
        assert!(
            spent <= 2,
            "8 watched directories must cost ~1 inotify instance, not one each \
             (before={before}, after={after}, spent={spent})"
        );
        drop(watchers);
    }

    /// The failure mode that collapsing onto one instance could silently
    /// introduce: events delivered for the first registered directory only.
    /// Adversarial oracle — every one of the N watchers must see ITS OWN file
    /// and no other's, so neither "only the first works" nor "everyone gets
    /// everything" passes.
    #[test]
    fn every_watched_dir_still_receives_only_its_own_events() {
        let dirs: Vec<TempDir> = (0..4).map(|_| TempDir::new().unwrap()).collect();
        let watchers: Vec<OrgDirWatcher> = dirs
            .iter()
            .map(|d| OrgDirWatcher::new(d.path()).unwrap())
            .collect();
        let expected: Vec<PathBuf> = dirs
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let p = d.path().join(format!("note-{i}.org"));
                std::fs::write(&p, SAMPLE).unwrap();
                p.canonicalize().unwrap()
            })
            .collect();

        let mut seen: Vec<std::collections::HashSet<PathBuf>> =
            vec![Default::default(); watchers.len()];
        let all = wait_for(|| {
            for (i, w) in watchers.iter().enumerate() {
                for c in w.drain() {
                    if let OrgChange::Upserted(p) = c {
                        seen[i].insert(p);
                    }
                }
            }
            seen.iter()
                .enumerate()
                .all(|(i, s)| s.contains(&expected[i]))
        });
        assert!(
            all,
            "each watcher must observe its own directory's file: {seen:?}"
        );
        for (i, s) in seen.iter().enumerate() {
            for (j, other) in expected.iter().enumerate() {
                if i != j {
                    assert!(
                        !s.contains(other),
                        "watcher {i} must not receive watcher {j}'s event ({other:?})"
                    );
                }
            }
        }
    }

    /// Nested registrations: a file under the INNER directory belongs to the
    /// inner watcher, not the outer one — longest-prefix routing, not
    /// first-match. The outer watcher still owns files that are only under it.
    #[test]
    fn nested_dirs_route_by_longest_prefix() {
        let outer = TempDir::new().unwrap();
        let inner_path = outer.path().join("inner");
        std::fs::create_dir(&inner_path).unwrap();
        let w_outer = OrgDirWatcher::new(outer.path()).unwrap();
        let w_inner = OrgDirWatcher::new(&inner_path).unwrap();

        let inner_file = inner_path.join("deep.org");
        let outer_file = outer.path().join("shallow.org");
        std::fs::write(&inner_file, SAMPLE).unwrap();
        std::fs::write(&outer_file, SAMPLE).unwrap();
        let inner_file = inner_file.canonicalize().unwrap();
        let outer_file = outer_file.canonicalize().unwrap();

        let mut outer_seen: std::collections::HashSet<PathBuf> = Default::default();
        let mut inner_seen: std::collections::HashSet<PathBuf> = Default::default();
        let ok = wait_for(|| {
            for c in w_outer.drain() {
                if let OrgChange::Upserted(p) = c {
                    outer_seen.insert(p);
                }
            }
            for c in w_inner.drain() {
                if let OrgChange::Upserted(p) = c {
                    inner_seen.insert(p);
                }
            }
            outer_seen.contains(&outer_file) && inner_seen.contains(&inner_file)
        });
        assert!(ok, "outer={outer_seen:?} inner={inner_seen:?}");
        assert!(
            !outer_seen.contains(&inner_file),
            "the inner directory's file must route to the inner watcher only"
        );
        assert!(
            !inner_seen.contains(&outer_file),
            "the outer directory's file must not reach the inner watcher"
        );
    }

    /// A dropped handle must stop receiving — otherwise the shared instance
    /// would leak routing state (and keep an unwatch pending forever).
    #[test]
    fn dropping_one_watcher_leaves_the_others_working() {
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        let wa = OrgDirWatcher::new(a.path()).unwrap();
        let wb = OrgDirWatcher::new(b.path()).unwrap();
        drop(wa);
        let f = b.path().join("still-live.org");
        std::fs::write(&f, SAMPLE).unwrap();
        let expected = f.canonicalize().unwrap();
        assert!(
            wait_for(|| wb
                .drain()
                .iter()
                .any(|c| matches!(c, OrgChange::Upserted(p) if p == &expected))),
            "dropping one watcher must not stop delivery to the rest"
        );
    }

    #[test]
    fn watcher_reports_removed_with_ids_from_seed() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("a.org");
        std::fs::write(&path, SAMPLE).unwrap();
        let w = OrgDirWatcher::new(tmp.path()).unwrap();
        w.seed([(path.clone(), vec!["abc-123".to_string()])]);
        std::fs::remove_file(&path).unwrap();
        let got = wait_for(|| {
            w.drain().iter().any(
                |c| matches!(c, OrgChange::Removed(ids) if ids.contains(&"abc-123".to_string())),
            )
        });
        assert!(got, "did not observe Removed event with seeded id");
    }
}
