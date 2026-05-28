use criterion::{black_box, criterion_group, criterion_main, Criterion};
use mae_sync::text::TextSync;

fn bench_crdt_creation(c: &mut Criterion) {
    c.bench_function("textsync_new_empty", |b| {
        b.iter(|| black_box(TextSync::with_client_id("", 1)));
    });

    c.bench_function("textsync_new_1k", |b| {
        let content: String = (0..1_000).map(|i| format!("line {i}\n")).collect();
        b.iter(|| black_box(TextSync::with_client_id(black_box(&content), 1)));
    });
}

fn bench_crdt_encode(c: &mut Criterion) {
    let content: String = (0..1_000).map(|i| format!("line {i}\n")).collect();
    let sync = TextSync::with_client_id(&content, 1);

    c.bench_function("encode_state_1k", |b| {
        b.iter(|| black_box(sync.encode_state()));
    });

    c.bench_function("state_vector_1k", |b| {
        b.iter(|| black_box(sync.state_vector()));
    });
}

fn bench_crdt_apply_update(c: &mut Criterion) {
    let content: String = (0..100).map(|i| format!("line {i}\n")).collect();

    c.bench_function("apply_small_update", |b| {
        // Create a "remote" update by making an edit on a separate doc.
        let mut remote = TextSync::with_client_id(&content, 2);
        let update = remote.insert(0, "hello ");

        b.iter_batched(
            || TextSync::with_client_id(&content, 1),
            |mut local| {
                let _ = local.apply_update(black_box(&update));
                black_box(&local);
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

fn bench_crdt_reconcile(c: &mut Criterion) {
    let content: String = (0..100).map(|i| format!("line {i}\n")).collect();

    c.bench_function("reconcile_to_small_diff", |b| {
        b.iter_batched(
            || {
                let sync = TextSync::with_client_id(&content, 1);
                let mut modified = content.clone();
                modified.insert_str(50, "INSERTED");
                (sync, modified)
            },
            |(mut sync, target)| {
                sync.reconcile_to(black_box(&target));
                black_box(&sync);
            },
            criterion::BatchSize::SmallInput,
        );
    });

    c.bench_function("reconcile_to_noop", |b| {
        b.iter_batched(
            || TextSync::with_client_id(&content, 1),
            |mut sync| {
                sync.reconcile_to(black_box(&content));
                black_box(&sync);
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

fn bench_crdt_apply_update_10k_history(c: &mut Criterion) {
    c.bench_function("apply_update_10k_history", |b| {
        // Build a doc with 10,000 prior edits.
        let mut base = TextSync::with_client_id("", 1);
        for i in 0..10_000 {
            base.insert(i as u32, "x");
        }
        let state = base.encode_state();

        // Create a remote edit.
        let mut remote = TextSync::with_client_id("", 2);
        remote.apply_update(&state).unwrap();
        let update = remote.insert(5000, "hello");

        b.iter_batched(
            || TextSync::from_state(black_box(&state)).unwrap(),
            |mut local| {
                let _ = local.apply_update(black_box(&update));
                black_box(&local);
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

fn bench_crdt_merge_diverged(c: &mut Criterion) {
    c.bench_function("merge_diverged_1k_edits", |b| {
        b.iter_batched(
            || {
                let mut doc_a = TextSync::with_client_id("base", 1);
                let mut doc_b = TextSync::with_client_id("base", 2);
                // Sync initial state.
                let state = doc_a.encode_state();
                doc_b.apply_update(&state).unwrap();
                // Each makes 1000 independent edits.
                for i in 0..1000 {
                    doc_a.insert(i as u32, "A");
                    doc_b.insert(i as u32, "B");
                }
                let state_a = doc_a.encode_state();
                let state_b = doc_b.encode_state();
                (doc_a, state_b, doc_b, state_a)
            },
            |(mut doc_a, state_b, mut doc_b, state_a)| {
                doc_a.apply_update(black_box(&state_b)).unwrap();
                doc_b.apply_update(black_box(&state_a)).unwrap();
                assert_eq!(doc_a.content(), doc_b.content());
                black_box(&doc_a);
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

criterion_group!(
    benches,
    bench_crdt_creation,
    bench_crdt_encode,
    bench_crdt_apply_update,
    bench_crdt_reconcile,
    bench_crdt_apply_update_10k_history,
    bench_crdt_merge_diverged
);
criterion_main!(benches);
