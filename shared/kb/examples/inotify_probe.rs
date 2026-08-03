//! TEMPORARY measurement probe (deleted before the final commit).
//!
//! Measures, in one process, both watcher designs side by side, reporting the
//! inotify-instance count after each step — so the before/after is measured
//! rather than inferred, and immune to environment drift between two runs.
//!
//! * LEGACY — one `notify::recommended_watcher()` per watched directory, i.e.
//!   the exact shape `OrgDirWatcher::new` had before the fix.
//! * SHARED — today's `OrgDirWatcher`/`StoreWatcher`, registrations on one
//!   process-wide watcher.
use mae_kb::watch::{inotify_instance_count, OrgDirWatcher, StoreWatcher};
use notify::{RecursiveMode, Watcher};

fn count() -> String {
    match inotify_instance_count() {
        Some(n) => n.to_string(),
        None => "n/a (not Linux)".to_string(),
    }
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let tmp = std::env::temp_dir().join(format!("mae-inotify-probe-{}", std::process::id()));
    let dirs: Vec<std::path::PathBuf> = (0..n)
        .map(|i| {
            let d = tmp.join(format!("dir{i}"));
            std::fs::create_dir_all(&d).unwrap();
            d
        })
        .collect();
    let store_file = tmp.join("primary.cozo");
    std::fs::write(&store_file, b"x").unwrap();
    let registry_file = tmp.join("kb-registry.toml");
    std::fs::write(&registry_file, b"x").unwrap();

    println!("== baseline: {} instances ==", count());

    println!("-- LEGACY (one recommended_watcher per directory) --");
    {
        let mut legacy = Vec::new();
        for (i, d) in dirs.iter().enumerate() {
            let (tx, rx) = std::sync::mpsc::channel();
            match notify::recommended_watcher(move |res| {
                let _ = tx.send(res);
            }) {
                Ok(mut w) => match w.watch(d, RecursiveMode::Recursive) {
                    Ok(()) => {
                        legacy.push((w, rx));
                        println!("  {} dir(s) watched -> {} instances", i + 1, count());
                    }
                    Err(e) => {
                        println!("  dir {i} watch FAILED: {e}");
                        break;
                    }
                },
                Err(e) => {
                    println!("  dir {i} watcher FAILED: {e}");
                    break;
                }
            }
        }
        println!("  legacy peak: {} instances", count());
    }
    println!("  after drop: {} instances", count());

    println!("-- SHARED (OrgDirWatcher + StoreWatcher registrations) --");
    {
        let mut held: Vec<OrgDirWatcher> = Vec::new();
        for (i, d) in dirs.iter().enumerate() {
            match OrgDirWatcher::new(d) {
                Ok(w) => {
                    held.push(w);
                    println!("  {} dir(s) watched -> {} instances", i + 1, count());
                }
                Err(e) => {
                    println!("  dir {i} FAILED: {e}");
                    break;
                }
            }
        }
        let _sw = StoreWatcher::new(&store_file).map_err(|e| println!("  store FAILED: {e}"));
        let _rw = StoreWatcher::new(&registry_file).map_err(|e| println!("  registry FAILED: {e}"));
        println!("  + store + registry watcher -> {} instances", count());

        // Delivery check: every registration must still receive its own events.
        for (i, d) in dirs.iter().enumerate() {
            std::fs::write(
                d.join("probe.org"),
                ":PROPERTIES:\n:ID: probe\n:END:\n#+title: P\n",
            )
            .unwrap();
            let _ = i;
        }
        std::fs::write(&store_file, b"changed").unwrap();
        let mut seen = vec![false; held.len()];
        let ok = mae_kb::watch::wait_for(|| {
            for (i, w) in held.iter().enumerate() {
                if !w.drain().is_empty() {
                    seen[i] = true;
                }
            }
            seen.iter().all(|s| *s)
        });
        println!("  every dir watcher received its own event: {ok} ({seen:?})");
    }
    println!("  after drop: {} instances", count());
    let _ = std::fs::remove_dir_all(&tmp);
}
