use std::iter;

use criterion::{
    criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, BenchmarkId, Criterion,
    Throughput,
};

use packbits::{Decoder, Encoder};

// outside buffers to avoid benchmarking allocations
fn roundtrip(input_buffer: &mut Vec<u8>, output_buffer: &mut Vec<u8>, data: &[u8]) {
    let mut encoder = Encoder::new();
    input_buffer.clear();
    encoder.encode_on(data, input_buffer);
    encoder.finalize_on(input_buffer);

    let mut decoder = Decoder::new();
    output_buffer.clear();
    decoder.decode_on(input_buffer.as_slice(), output_buffer);
    decoder.finalize().unwrap();
}

fn bench_roundtrip(mut group: BenchmarkGroup<'_, WallTime>, get_input: impl Fn(usize) -> Vec<u8>) {
    let sizes = iter::successors(Some(1_usize), |prev| prev.checked_mul(2))
        .take(16)
        .collect::<Vec<_>>();

    for size in sizes {
        let input = get_input(size);
        assert_eq!(input.len(), size);

        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &input, |b, input| {
            let mut input_buffer = Vec::with_capacity(size * 2);
            let mut output_buffer = Vec::with_capacity(size);
            b.iter(|| roundtrip(&mut input_buffer, &mut output_buffer, input))
        });
    }
}

fn single_repeated_byte(c: &mut Criterion) {
    bench_roundtrip(c.benchmark_group("single_repeated_byte"), |size| {
        iter::repeat(0u8).take(size).collect()
    })
}

fn incrementing_counter(c: &mut Criterion) {
    bench_roundtrip(c.benchmark_group("incrementing_counter"), |size| {
        (0..size).map(|b| (b & 0xFF) as u8).collect()
    })
}

fn mixed_load(c: &mut Criterion) {
    const REPEAT_SIZE: usize = 5; // size of chunk of repeated byte
    const RANDOM_SIZE: u8 = 5; // size of unrepeated bytes

    bench_roundtrip(c.benchmark_group("mixed_load"), |size| {
        iter::successors(Some(true), |prev| Some(!prev))
            .flat_map(|repeat| {
                if repeat {
                    [0u8; REPEAT_SIZE].to_vec()
                } else {
                    (0u8..RANDOM_SIZE).collect::<Vec<u8>>()
                }
            })
            .take(size)
            .collect()
    })
}

criterion_group!(
    benches,
    single_repeated_byte,
    incrementing_counter,
    mixed_load
);
criterion_main!(benches);
