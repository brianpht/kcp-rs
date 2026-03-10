// benches/throughput.rs
//!
//! Benchmark design principles:
//! - `black_box` on BOTH input AND output to prevent dead-code elimination
//! - Measure steady-state hot path performance
//! - Avoid optimizer shortcuts that don't reflect real usage

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use kcp_rs::{Kcp, KcpConfig, KcpOutput, KcpResult};
use std::hint::black_box;

/// Null output that prevents optimizer from eliminating the output path
struct NullOutput;

impl KcpOutput for NullOutput {
    #[inline]
    fn output(&mut self, data: &[u8]) -> KcpResult<usize> {
        // black_box the slice reference to prevent:
        // 1. Dead code elimination of the entire output call
        // 2. Constant propagation of data contents
        black_box(data);
        Ok(data.len())
    }
}

/// Benchmark send() hot path with various payload sizes
///
/// Measures single send operation on fresh-enough state.
/// Uses update() between sends to flush and reset buffer state.
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
                let mut ts = 0u32;
                b.iter(|| {
                    // Flush previous data to prevent buffer-full
                    ts = ts.wrapping_add(100);
                    let _ = kcp.update(ts);
                    // black_box input: prevent constant propagation
                    // black_box output: prevent dead code elimination
                    black_box(kcp.send(black_box(&data)))
                });
            },
        );
    }

    group.finish();
}

/// Benchmark update() cycle - the main timer-driven entry point
fn bench_update(c: &mut Criterion) {
    let mut group = c.benchmark_group("update");

    // Empty state: measures baseline update overhead
    group.bench_function("empty", |b| {
        let mut kcp = Kcp::with_config(1, NullOutput, KcpConfig::fast());
        let mut ts = 0u32;
        b.iter(|| {
            ts = ts.wrapping_add(10);
            // black_box both input timestamp and result
            black_box(kcp.update(black_box(ts)))
        });
    });

    // With pending data: measures update with flush work
    group.bench_function("with_data", |b| {
        let mut kcp = Kcp::with_config(1, NullOutput, KcpConfig::fast());
        let data = [0u8; 1024];
        let mut ts = 0u32;
        b.iter(|| {
            // Re-add data each iteration to ensure flush has work
            let _ = kcp.send(&data);
            ts = ts.wrapping_add(100);
            black_box(kcp.update(black_box(ts)))
        });
    });

    group.finish();
}

/// Benchmark recv() hot path - reading from receive buffer
fn bench_recv(c: &mut Criterion) {
    let mut group = c.benchmark_group("recv");

    group.bench_function("no_data", |b| {
        let mut kcp = Kcp::with_config(1, NullOutput, KcpConfig::fast());
        let mut buf = [0u8; 1500];
        b.iter(|| {
            // Measure early-return path (WouldBlock)
            black_box(kcp.recv(black_box(&mut buf)))
        });
    });

    group.finish();
}

/// Benchmark codec operations (encode/decode segment headers)
fn bench_codec(c: &mut Criterion) {
    use kcp_rs::codec::{decode_segment, encode_segment};
    use kcp_rs::segment::SegmentHeader;

    let mut group = c.benchmark_group("codec");

    group.bench_function("encode_header", |b| {
        let header = SegmentHeader {
            conv: 1,
            cmd: 81, // CMD_PUSH
            frg: 0,
            wnd: 128,
            ts: 12345,
            sn: 1,
            una: 0,
            len: 64,
        };
        let mut buf = [0u8; 128];
        b.iter(|| black_box(encode_segment(black_box(&mut buf), black_box(&header), &[])))
    });

    group.bench_function("decode_header", |b| {
        // Pre-encoded valid segment
        let mut buf = [0u8; 128];
        let header = SegmentHeader {
            conv: 1,
            cmd: 81,
            frg: 0,
            wnd: 128,
            ts: 12345,
            sn: 1,
            una: 0,
            len: 0,
        };
        let _ = encode_segment(&mut buf, &header, &[]);

        b.iter(|| black_box(decode_segment(black_box(&buf))))
    });

    group.finish();
}

criterion_group!(benches, bench_send, bench_update, bench_recv, bench_codec);
criterion_main!(benches);