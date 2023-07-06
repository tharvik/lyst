use std::path::Path;

use tokio::runtime::Runtime;

use lyst::Mohawk;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

async fn test_known_file(filename: &str) {
    let path = Path::new("myst").join(filename);

    Mohawk::open(&path).await.expect("to parse Mohawk file");
}

fn criterion_benchmark(c: &mut Criterion) {
    let rt = Runtime::new().expect("get tokio runtime");
    let filename = "MYST.DAT";

    c.bench_with_input(
        BenchmarkId::new("load mohawk", filename),
        &filename,
        |b, &s| {
            b.to_async(&rt).iter(|| test_known_file(s));
        },
    );
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
