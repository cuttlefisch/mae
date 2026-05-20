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

criterion_group!(
    benches,
    bench_crdt_creation,
    bench_crdt_encode,
    bench_crdt_apply_update,
    bench_crdt_reconcile
);
criterion_main!(benches);
