//! ADR-120 P1a's benchmark (O5): the latency the record adds to a request at
//! 50, 200 and 1,000 events per second, bytes per event, and batch sizes, with
//! `synchronous=FULL` on a real disk (never tmpfs, where fsync is free).
//!
//! `#[ignore]`d: it measures, it does not gate. Run it with
//! `cargo test --lib activity::bench_tests -- --ignored --nocapture`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::event::{ActorKind, Event, EventKind, Principal, Transport};
use super::session::{self, SessionKey};
use super::store::Entry;
use super::writer::ActivityRecord;
use super::ActivitySettings;

/// A read as the listener records it: principal, session, route, KB, nodes.
fn read_event(i: u64) -> Entry {
    let mut ev = Event::new(EventKind::Read, super::now_us(), Transport::Https);
    ev.principal = Principal {
        id: Some(mae_mcp::identity::Identity::generate("bench").fingerprint()),
        kind: ActorKind::Person,
        iss: Some("https://idp.example".into()),
        sub: Some(format!("3f6c2a1e-8d4b-4f7a-9c1e-{:012}", i % 50)),
        on_behalf_of: None,
        grant: None,
    };
    ev.session = Some(session::random_id());
    ev.route = Some("kb/query.get".into());
    ev.kb_id = Some("kb-handbook".into());
    ev.node_ids = Some(vec![format!("20251107T101500-{i:06}")]);
    Entry::of(ev)
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() as f64 - 1.0) * p).round() as usize]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a measurement, run on demand (ADR-120 O5)"]
async fn added_latency_bytes_and_batches_at_three_rates() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    std::fs::create_dir_all(&root).unwrap();
    for rate in [50u64, 200, 1000] {
        let dir = tempfile::tempdir_in(&root).unwrap();
        let settings = ActivitySettings {
            enabled: true,
            ..ActivitySettings::default()
        };
        let rec = ActivityRecord::start(
            &dir.path().join(super::DB_FILE),
            settings,
            SessionKey::generate(),
        );
        let secs = 10;
        let n = rate * secs;
        let gap = Duration::from_secs_f64(1.0 / rate as f64);
        let start = Instant::now();
        let mut tasks = Vec::with_capacity(n as usize);
        for i in 0..n {
            let due = start + gap * i as u32;
            tokio::time::sleep_until(due.into()).await;
            let rec = Arc::clone(&rec);
            tasks.push(tokio::spawn(async move {
                let t = Instant::now();
                rec.commit(read_event(i)).await.expect("committed");
                t.elapsed()
            }));
        }
        let mut lat = Vec::with_capacity(tasks.len());
        for t in tasks {
            lat.push(t.await.unwrap());
        }
        lat.sort();
        let s = rec.stats();
        // Closing checkpoints the WAL into the file, so its size is the record's.
        rec.shutdown(Event::daemon(EventKind::DaemonStop, super::now_us()))
            .await;
        let file = std::fs::metadata(dir.path().join(super::DB_FILE))
            .unwrap()
            .len();
        println!(
            "rate {rate}/s, {n} events over {secs} s: added latency p50 {:?} p99 {:?} max {:?}; \
             {} bytes/event (link + body); {} batches, mean {:.2} events/batch, max {}; \
             on disk {} bytes/event with indexes",
            percentile(&lat, 0.50),
            percentile(&lat, 0.99),
            lat.last().unwrap(),
            s.bytes / s.events.max(1),
            s.batches,
            s.events as f64 / s.batches.max(1) as f64,
            s.max_batch,
            file / s.events.max(1),
        );
    }
}
