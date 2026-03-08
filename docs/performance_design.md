# Performance Design

> **Deterministic, allocation-free, lock-free KCP protocol implementation in Rust.**

---

## Table of Contents

- [Project Overview](#project-overview)
- [Governance Model](#governance-model)
- [Deployment Assumptions](#deployment-assumptions)
- [Performance Targets](#performance-targets)
- [Architecture Overview](#architecture-overview)
- [Core Design Principles](#core-design-principles)
  - [1. Determinism First](#1-determinism-first)
  - [2. Allocation-Free Hot Path](#2-allocation-free-hot-path)
  - [3. Cache-Oriented Design](#3-cache-oriented-design)
  - [4. Branch Predictability](#4-branch-predictability)
  - [5. Lock-Free Model](#5-lock-free-model)
  - [6. Sequence Arithmetic](#6-sequence-arithmetic)
  - [7. Ring Buffer Discipline](#7-ring-buffer-discipline)
  - [8. Wire Format (Little-Endian)](#8-wire-format-little-endian)
  - [9. Fragmentation & Reassembly](#9-fragmentation--reassembly)
  - [10. Congestion Control](#10-congestion-control)
  - [11. Unsafe Policy](#11-unsafe-policy)
  - [12. Performance Budget](#12-performance-budget)
- [Data Structures](#data-structures)
- [Hot Path Operations](#hot-path-operations)
- [Final Principle](#final-principle)

---

## Project Overview

### What is KCP?

KCP is a fast and reliable ARQ (Automatic Repeat-reQuest) protocol designed to reduce latency compared to TCP. This implementation focuses on:

- **Zero allocation** in steady-state operation
- **Deterministic latency** under all network conditions
- **Lock-free** single-writer architecture
- **`no_std` compatible** for embedded/bare-metal use

### Target Domains

| Domain                          | Use Case                              |
|---------------------------------|---------------------------------------|
| Real-time multiplayer games     | Game state synchronization            |
| Live streaming                  | Low-latency media transport           |
| VPN/Proxy tunneling             | Reliable overlay networks             |
| IoT & Embedded systems          | Resource-constrained environments     |
| High-frequency trading          | Ultra-low latency messaging           |

### Inspiration

| Source                    | Contribution                            |
|---------------------------|-----------------------------------------|
| Original KCP (C)          | Protocol specification and algorithms   |
| Aeron transport design    | Lock-free, zero-copy principles         |
| Mechanical Sympathy       | Hardware-aware optimization             |
| Glenn Fiedler's articles  | Reliable UDP patterns                   |

---

## Governance Model

This project defines **two layers** of performance governance:

| Layer            | Document                          | Purpose                      |
|------------------|-----------------------------------|------------------------------|
| **Architecture** | `docs/performance_design.md`      | Defines intent and reasoning |
| **Enforcement**  | `.github/copilot-instructions.md` | Enforces non-negotiable rules|

### Conflict Resolution

```
If enforcement rules conflict with architecture
    → Architecture must be updated first

Benchmarks are the final authority.
```

### Auto-Reject Rules

The following patterns are **automatically rejected** in code review:

| Pattern                              | Reason                           |
|--------------------------------------|----------------------------------|
| `Mutex` in transport                 | Blocking degrades latency        |
| `HashMap` in hot path                | Non-deterministic access time    |
| `%` (modulo) in ring index           | Use `& (capacity - 1)` instead   |
| `unwrap()` in parsing                | Must handle malformed input      |
| Trait object in packet processing    | vtable indirection               |
| Allocation inside loop               | Heap allocation in hot path      |
| Sequence comparison using `>`        | Use wrapping arithmetic          |
| `Vec` growth / `Box` / `String` in hot path | Heap allocation            |

---

## Deployment Assumptions

| Assumption           | Value                                             |
|----------------------|---------------------------------------------------|
| Primary target       | x86_64, aarch64                                   |
| Wire format          | **Little-endian (protocol-defined)**              |
| Cluster architecture | Same-architecture expected                        |
| Memory model         | Preallocated buffers at initialization            |
| Threading            | Single-writer, caller-driven event loop           |

> ⚠️ **Note**: Cross-endian compatibility requires explicit byte-order handling in wire format.

---

## Performance Targets

| Metric                 | Target      | Current   | Status |
|------------------------|-------------|-----------|--------|
| Small packet send      | < 200 ns    | ~2.6 ns   | ✅      |
| Update cycle (empty)   | < 100 ns    | ~47 ns    | ✅      |
| Update cycle (data)    | < 100 ns    | ~45 ns    | ✅      |
| Header encode          | < 10 ns     | ~3 ns     | ✅      |
| Header decode          | < 10 ns     | ~2 ns     | ✅      |
| Allocation (steady)    | 0           | 0         | ✅      |
| Cache miss (steady)    | 0           | 0         | ✅      |

### Regression Policy

- **> 10% regression** → requires justification and rollback consideration
- **Latency variance > average** → investigate immediately
- **Tail latency (p99)** matters more than average

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                        Application                          │
├─────────────────────────────────────────────────────────────┤
│                      Kcp<O: KcpOutput>                      │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │  snd_queue  │→ │   snd_buf   │→ │   output (UDP)      │  │
│  │ (pending)   │  │ (in-flight) │  │   callback          │  │
│  └─────────────┘  └─────────────┘  └─────────────────────┘  │
│                                                             │
│  ┌─────────────┐  ┌─────────────┐                           │
│  │   rcv_buf   │← │   input     │← (from network)           │
│  │ (reassembly)│  │  (parsing)  │                           │
│  └─────────────┘  └─────────────┘                           │
│                                                             │
│  ┌─────────────┐                                            │
│  │  ack_list   │  (pending ACKs to send)                    │
│  └─────────────┘                                            │
└─────────────────────────────────────────────────────────────┘
```

### Key Components

| Component    | Type         | Purpose                              |
|--------------|--------------|--------------------------------------|
| `snd_queue`  | `SendBuffer` | Pending segments awaiting cwnd       |
| `snd_buf`    | `SendBuffer` | In-flight segments awaiting ACK      |
| `rcv_buf`    | `RecvBuffer` | Received segments for reassembly     |
| `ack_list`   | `AckList`    | Pending ACKs to piggyback/send       |
| `output_buf` | `[u8; MTU]`  | Preallocated output buffer           |

---

## Core Design Principles

### 1. Determinism First

```
Correctness > Determinism > Latency > Throughput
```

> ⚠️ **Unbounded memory or nondeterministic latency is a correctness failure.**

#### Must Be Deterministic Under

| Condition      | Required |
|----------------|----------|
| Packet loss    | ✅        |
| Reordering     | ✅        |
| Duplication    | ✅        |
| Sequence wrap  | ✅        |
| Clock drift    | ✅        |
| Window = 0     | ✅        |

**No randomness in protocol logic.**

---

### 2. Allocation-Free Hot Path

#### Hot Path Operations

| Operation          | Allocation Allowed? |
|--------------------|---------------------|
| `send()`           | ❌                   |
| `recv()`           | ❌                   |
| `input()`          | ❌                   |
| `update()`         | ❌                   |
| `flush()`          | ❌                   |
| ACK processing     | ❌                   |
| Loss detection     | ❌                   |
| Fragment reassembly| ❌                   |

#### Memory Strategy

- All buffers **preallocated at initialization**
- Buffer capacities are **compile-time constants**
- **Reuse everything** - no temporary allocations
- Data stored in contiguous arrays with index-based access

```rust
// ✅ Correct - preallocated
pub struct SendBuffer {
    segments: [SendSegment; SND_BUF_CAPACITY],
    data: [u8; SND_BUF_CAPACITY * MSS_DEFAULT],
    // ...
}

// ❌ Forbidden - dynamic allocation
pub struct SendBuffer {
    segments: Vec<SendSegment>,
    data: Vec<u8>,
}
```

---

### 3. Cache-Oriented Design

#### CPU Memory Latency Reference

| Level | Latency  |
|-------|----------|
| L1    | ~1 ns    |
| L2    | ~3 ns    |
| L3    | ~10 ns   |
| RAM   | ~100 ns  |

#### Rules

| Rule                          | Priority |
|-------------------------------|----------|
| Contiguous memory layout      | Required |
| Avoid pointer chasing         | Required |
| Power-of-two ring buffers     | Required |
| Hot fields first in struct    | Required |
| Hot structs ≤ 64 bytes        | Required |
| Separate hot/cold data        | Required |

#### Struct Layout Example

```rust
/// Send segment - hot fields first
#[repr(C)]
pub struct SendSegment {
    // Hot fields (accessed every update) - 20 bytes
    pub sn: Sequence,          // 4 bytes
    pub resend_ts: u32,        // 4 bytes
    pub rto: u32,              // 4 bytes
    pub fastack: u16,          // 2 bytes
    pub xmit: u16,             // 2 bytes
    pub state: SegmentState,   // 1 byte
    pub frg: u8,               // 1 byte
    pub data_len: u16,         // 2 bytes

    // Cold fields
    pub ts: u32,               // 4 bytes
    pub data_offset: u32,      // 4 bytes
}
```

---

### 4. Branch Predictability

**Mispredict penalty**: ~15–20 cycles (~5–7 ns)

| Rule                            | Priority       |
|---------------------------------|----------------|
| Fast path first                 | Required       |
| Error paths marked `#[cold]`    | Required       |
| Avoid data-dependent divergence | Required       |
| Early return on error           | Required       |

```rust
// ✅ Correct - fast path first
pub fn input(&mut self, data: &[u8]) -> KcpResult<()> {
    if data.len() < HEADER_SIZE {
        return Err(KcpError::InvalidPacket);  // Cold path
    }
    // Fast path continues...
}

// Error enum with cold annotation
#[derive(Debug)]
pub enum KcpError {
    BufferTooSmall,
    InvalidPacket,
    // ...
}
```

---

### 5. Lock-Free Model

**Default**: Single-writer principle.

- KCP instance owned by single thread
- Caller drives event loop via `update()`
- No internal synchronization required

#### Atomic Ordering (If Ever Needed)

| Ordering  | Use Case             |
|-----------|----------------------|
| `Relaxed` | Counters             |
| `Release` | Publish data         |
| `Acquire` | Consume data         |
| `SeqCst`  | **Avoid in hot path**|

---

### 6. Sequence Arithmetic

KCP uses 32-bit sequence numbers that wrap around. Proper comparison requires **half-range rule**.

| Rule                           | Status        |
|--------------------------------|---------------|
| Use wrapping arithmetic        | Required      |
| Half-range rule for comparison | Required      |
| Test wrap-around cases         | Required      |
| Naive `>` comparison           | **Forbidden** |
| Non-wrapping subtraction       | **Forbidden** |

```rust
/// Sequence number with proper wrapping
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Sequence(pub u32);

impl Sequence {
    /// Half range for sequence comparison (2^31)
    const HALF_RANGE: u32 = 1 << 31;

    /// Increment with wrapping
    #[inline(always)]
    pub const fn increment(self) -> Self {
        Self(self.0.wrapping_add(1))
    }

    /// Check if self is after other (half-range comparison)
    #[inline(always)]
    pub const fn is_after(self, other: Self) -> bool {
        let diff = self.0.wrapping_sub(other.0);
        diff > 0 && diff < Self::HALF_RANGE
    }

    /// Convert to ring buffer index
    #[inline(always)]
    pub const fn to_index(self, mask: usize) -> usize {
        (self.0 as usize) & mask
    }
}
```

---

### 7. Ring Buffer Discipline

#### Capacity

- **MUST** be power-of-two
- Verified at compile time

```rust
/// Compile-time assertion
const _: () = assert!(SND_BUF_CAPACITY.is_power_of_two());
```

#### Indexing

```rust
// ✅ Correct - bitwise AND
index = seq & (capacity - 1)
index = seq.to_index(MASK)

// ❌ Forbidden - modulo
index = seq % capacity
```

| Rule                             | Status   |
|----------------------------------|----------|
| Power-of-two capacity            | Required |
| Bitwise AND for indexing         | Required |
| Never use `%` in hot path        | Required |
| Overwrites must be deterministic | Required |
| Precomputed masks                | Required |

---

### 8. Wire Format (Little-Endian)

Wire format uses **Little-Endianness only**.

#### KCP Header Format (24 bytes)

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                        conv (4 bytes)                        |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|     cmd       |     frg       |           wnd                |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                         ts (4 bytes)                         |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                         sn (4 bytes)                         |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                        una (4 bytes)                         |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                        len (4 bytes)                         |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                        data (variable)                       |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

#### Encoding Example

```rust
impl SegmentHeader {
    #[inline]
    pub fn encode(&self, buf: &mut [u8]) -> Option<()> {
        if buf.len() < HEADER_SIZE {
            return None;
        }
        buf[0..4].copy_from_slice(&self.conv.to_le_bytes());
        buf[4] = self.cmd;
        buf[5] = self.frg;
        buf[6..8].copy_from_slice(&self.wnd.to_le_bytes());
        buf[8..12].copy_from_slice(&self.ts.to_le_bytes());
        buf[12..16].copy_from_slice(&self.sn.to_le_bytes());
        buf[16..20].copy_from_slice(&self.una.to_le_bytes());
        buf[20..24].copy_from_slice(&self.len.to_le_bytes());
        Some(())
    }

    #[inline]
    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < HEADER_SIZE {
            return None;
        }
        Some(Self {
            conv: u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
            cmd: buf[4],
            frg: buf[5],
            wnd: u16::from_le_bytes([buf[6], buf[7]]),
            ts: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
            sn: u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
            una: u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]),
            len: u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]),
        })
    }
}
```

#### Rules

| Rule                               | Status   |
|------------------------------------|----------|
| Little-endian only                 | Required |
| No pointer casting                 | Required |
| No host-endian assumptions         | Required |
| Fixed-size headers                 | Required |
| Header read/write < 10 ns          | Required |

---

### 9. Fragmentation & Reassembly

Large messages are split into MSS-sized fragments with decreasing fragment numbers.

#### Fragment Numbering

```
Message: [   Fragment 2   |   Fragment 1   |   Fragment 0   ]
                 ↓               ↓                ↓
Packet:    frg=2, sn=N     frg=1, sn=N+1    frg=0, sn=N+2
```

- `frg=0` indicates last fragment
- Receiver waits for all fragments before delivering

#### Requirements

| Requirement                    | Status    |
|--------------------------------|-----------|
| Bounded fragment count         | Required  |
| Allocation-free reassembly     | Required  |
| Deterministic expiration       | Required  |
| Fragment validation            | Required  |
| Trust fragment count from wire | **Never** |

---

### 10. Congestion Control

KCP implements optional congestion control with configurable modes.

#### Modes

| Mode    | `nodelay` | `nc`  | Behavior                      |
|---------|-----------|-------|-------------------------------|
| Normal  | false     | false | TCP-like congestion control   |
| Fast    | true      | false | Reduced RTO, fast retransmit  |
| NoCC    | true      | true  | No congestion control         |

#### RTO Calculation (RFC 6298)

```rust
fn update_rtt(&mut self, rtt: u32) {
    if self.rx_srtt == 0 {
        self.rx_srtt = rtt;
        self.rx_rttval = rtt / 2;
    } else {
        let delta = (rtt - self.rx_srtt).abs();
        self.rx_rttval = (3 * self.rx_rttval + delta) / 4;
        self.rx_srtt = (7 * self.rx_srtt + rtt) / 8;
    }
    self.rx_rto = self.rx_srtt + max(interval, 4 * self.rx_rttval);
}
```

#### Fast Retransmit

- Triggered after `fastresend` duplicate ACKs
- Bypasses RTO timer for faster recovery
- Configurable via `KcpConfig::resend`

---

### 11. Unsafe Policy

This crate uses `#![forbid(unsafe_code)]` by default.

#### Allowed Only If

| Condition                | Required |
|--------------------------|----------|
| Measurable gain proven   | ✅        |
| Benchmarked before/after | ✅        |
| Invariants documented    | ✅        |
| Fuzz-tested              | ✅        |
| Approved in code review  | ✅        |

> ❌ **Unsafe without justification → reject.**

---

### 12. Performance Budget

#### Small Packet Send Path

| Metric                  | Target       |
|-------------------------|--------------|
| Allocation              | **None**     |
| Latency                 | **< 200 ns** |
| Steady-state cache miss | **None**     |

#### Update Cycle

| Metric                  | Target       |
|-------------------------|--------------|
| Empty update            | **< 100 ns** |
| With pending data       | **< 100 ns** |
| With retransmit         | **< 200 ns** |

#### Investigation Triggers

```
p99 > p50 × 2  → investigate variance
p99 > 1 μs     → investigate outliers
allocation > 0 → investigate hot path
```

---

## Data Structures

### Buffer Capacities

| Buffer           | Capacity | Size (bytes)      |
|------------------|----------|-------------------|
| `SND_BUF`        | 256      | ~352 KB           |
| `RCV_BUF`        | 256      | ~352 KB           |
| `ACK_LIST`       | 128      | ~1 KB             |
| Total per `Kcp`  | -        | **~705 KB**       |

> ⚠️ **Note**: Use `small-buffers` feature for testing to reduce to ~88 KB.

### Segment States

```
Empty → Pending → Sent → Acked
          ↓        ↑
          └────────┘ (retransmit)
```

| State   | Description                          |
|---------|--------------------------------------|
| Empty   | Slot available for use               |
| Pending | Queued, not yet transmitted          |
| Sent    | Transmitted, awaiting ACK            |
| Acked   | Acknowledged, ready for cleanup      |

---

## Hot Path Operations

### `send()` - O(n) where n = fragments

```
1. Calculate fragment count
2. For each fragment:
   a. Insert into snd_queue (index-based)
   b. Copy data to preallocated buffer
3. Update snd_nxt
```

### `update()` - O(1) amortized

```
1. Update current time
2. Check flush timer
3. If timer expired:
   a. Flush ACKs
   b. Handle window probe
   c. Move queue → buffer (cwnd limited)
   d. Flush data segments
```

### `input()` - O(n) where n = segments in packet

```
1. Validate header size
2. For each segment:
   a. Decode header
   b. Validate conv
   c. Update remote window
   d. Process UNA (cumulative ACK)
   e. Process by command type
3. Update fastack counters
4. Update congestion window
```

### `recv()` - O(m) where m = message fragments

```
1. Check if first segment ready
2. Verify all fragments present
3. Copy data to user buffer
4. Clear segments
5. Advance rcv_nxt
```

---

## Final Principle

| Layer        | Role              |
|--------------|-------------------|
| Architecture | Defines intent    |
| Enforcement  | Ensures invariants|
| Benchmarks   | Validates reality |

```
Architecture defines intent.
Enforcement ensures invariants.
Benchmarks validate reality.
```

---

## References

- [KCP Protocol (Original C)](https://github.com/skywind3000/kcp)
- [KCP Protocol Specification](https://github.com/skywind3000/kcp/wiki/KCP-Protocol-Specification)
- [Aeron - Efficient Reliable UDP](https://github.com/real-logic/aeron)
- [Glenn Fiedler - Reliable UDP](https://gafferongames.com/post/reliability_ordering_and_congestion_avoidance_over_udp/)

