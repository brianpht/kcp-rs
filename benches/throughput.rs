// benches/throughput.rs
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use kcp_rs::{Kcp, KcpConfig, KcpOutput, KcpResult};
use std::hint::black_box;

struct NullOutput;

impl KcpOutput for NullOutput {
    fn output(&mut self, data: &[u8]) -> KcpResult<usize> {
        black_box(data);
        Ok(data.len())
    }
}

fn bench_send(c: &mut Criterion) {
    let mut group = c.benchmark_group("send");

    for size in [64, 256, 1024, 4096].iter() {
        group.throughput(Throughput::Bytes(*size as u64));
        group.bench_with_input(
            format!("{}_bytes", size),
            size,
            |b, &size| {
                let mut kcp = Kcp::with_config(1, NullOutput, KcpConfig::fast());
                let data = vec![0u8; size];
                b.iter(|| {
                    let _ = kcp.send(black_box(&data));
                });
            },
        );
    }

    group.finish();
}

fn bench_update(c: &mut Criterion) {
    let mut group = c.benchmark_group("update");

    group.bench_function("empty", |b| {
        let mut kcp = Kcp::with_config(1, NullOutput, KcpConfig::fast());
        let mut ts = 0u32;
        b.iter(|| {
            ts = ts.wrapping_add(10);
            let _ = kcp.update(black_box(ts));
        });
    });

    group.bench_function("with_data", |b| {
        let mut kcp = Kcp::with_config(1, NullOutput, KcpConfig::fast());
        let data = [0u8; 1024];
        let _ = kcp.send(&data);
        let mut ts = 0u32;
        b.iter(|| {
            ts = ts.wrapping_add(10);
            let _ = kcp.update(black_box(ts));
        });
    });

    group.finish();
}

criterion_group!(benches, bench_send, bench_update);
criterion_main!(benches);