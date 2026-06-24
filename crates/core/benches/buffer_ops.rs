use criterion::{criterion_group, criterion_main, Criterion};
use mae_core::Buffer;
use std::hint::black_box;

fn bench_buffer_creation(c: &mut Criterion) {
    c.bench_function("buffer_create_empty", |b| {
        b.iter(|| black_box(Buffer::new()));
    });

    c.bench_function("buffer_create_1k_lines", |b| {
        let content: String = (0..1_000).map(|i| format!("line {i}\n")).collect();
        b.iter(|| {
            let mut buf = Buffer::new();
            buf.insert_text_at(0, black_box(&content));
            black_box(&buf);
        });
    });
}

fn bench_buffer_insert(c: &mut Criterion) {
    let base: String = (0..10_000).map(|i| format!("line {i}\n")).collect();

    c.bench_function("insert_beginning_10k", |b| {
        b.iter_batched(
            || {
                let mut buf = Buffer::new();
                buf.insert_text_at(0, &base);
                buf
            },
            |mut buf| {
                buf.insert_text_at(0, "inserted\n");
                black_box(&buf);
            },
            criterion::BatchSize::SmallInput,
        );
    });

    c.bench_function("insert_middle_10k", |b| {
        b.iter_batched(
            || {
                let mut buf = Buffer::new();
                buf.insert_text_at(0, &base);
                buf
            },
            |mut buf| {
                let mid = buf.rope().len_chars() / 2;
                buf.insert_text_at(mid, "inserted\n");
                black_box(&buf);
            },
            criterion::BatchSize::SmallInput,
        );
    });

    c.bench_function("insert_end_10k", |b| {
        b.iter_batched(
            || {
                let mut buf = Buffer::new();
                buf.insert_text_at(0, &base);
                buf
            },
            |mut buf| {
                let end = buf.rope().len_chars();
                buf.insert_text_at(end, "inserted\n");
                black_box(&buf);
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

fn bench_buffer_text(c: &mut Criterion) {
    let content: String = (0..10_000).map(|i| format!("line {i}\n")).collect();
    let mut buf = Buffer::new();
    buf.insert_text_at(0, &content);

    c.bench_function("buffer_text_10k", |b| {
        b.iter(|| black_box(buf.text()));
    });

    c.bench_function("buffer_line_count_10k", |b| {
        b.iter(|| black_box(buf.line_count()));
    });
}

criterion_group!(
    benches,
    bench_buffer_creation,
    bench_buffer_insert,
    bench_buffer_text
);
criterion_main!(benches);
