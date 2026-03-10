# Copilot Instructions

> **Deterministic, allocation-free, lock-free transport core.**
>
> ⚠️ If a change increases latency variance, allocation count, or branch entropy → **REJECT**.

---

## Critical Rules (Auto-Reject)

```
❌ Mutex in transport
❌ HashMap in hot path
❌ % (modulo) in ring index → use & (capacity - 1)
❌ unwrap() in parsing
❌ Trait object in packet processing
❌ Allocation inside loop
❌ Sequence comparison using > → use wrapping_sub + half-range
❌ Vec growth / Box / String in hot path
```

---

## Hot Path Operations

`send` | `receive` | `ack processing` | `loss detection` | `fragment reassembly` | `sequence buffer access`

**Requirements**: Allocation-free, O(1), Cache-local, Branch predictable, Single-writer

---

## Sequence Arithmetic

```rust
// ✅ CORRECT
a.wrapping_add(1)
a.wrapping_sub(b)
// Half-range comparison for sequence ordering

// ❌ FORBIDDEN
a > b
a - b
```

---

## Ring Buffer

```rust
// ✅ Capacity: power-of-two ONLY
index = seq & (capacity - 1)

// ❌ NEVER
index = seq % capacity
```

---

## Wire Format

- **Little-endian only** (`to_le_bytes`, `from_le_bytes`)
- Fixed-size headers
- No pointer casting
- No host-endian assumptions

---

## Memory

- All buffers preallocated at init
- All capacities power-of-two
- Reuse everything
- No heap allocation in steady state

---

## Cache & Branches

- Hot structs ≤ 64 bytes
- Hot fields first in struct
- Fast path first, error path `#[cold]`
- No pointer chasing

---

## Atomics (if needed)

`Relaxed` → counters | `Release` → publish | `Acquire` → consume | `SeqCst` → **NEVER in hot path**

---

## Unsafe Policy

Allowed only if: measurable gain + benchmarked + invariants documented + fuzz-tested

---

## Performance Budget

| Metric | Target |
|--------|--------|
| Small packet send | < 250ns |
| Update cycle (empty) | < 100ns |
| Update cycle (data) | < 250ns |
| Header encode/decode | < 10ns |
| Allocation | None |
| Cache miss (steady) | None |

> Regression > 10% → rollback or justify. **Latency variance > average.**

---

## Final Rule

```
Correctness > Determinism > Latency > Throughput
```

> Unbounded memory or nondeterministic latency = **correctness failure**.