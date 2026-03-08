//! Lock-free, allocation-free ring buffers
//!
//! All indexing uses bitwise AND with mask (capacity - 1)
//! NEVER uses modulo (%)

use crate::constants::*;
use crate::segment::{SendSegment, RecvSegment, AckEntry, SegmentState};
use crate::sequence::Sequence;

/// Generic ring buffer with compile-time capacity
/// Invariants:
/// - Capacity is always power of 2
/// - Index = sequence & mask (never modulo)
/// - Single writer, lock-free
#[repr(C)]
pub struct RingBuffer<T, const N: usize> {
    data: [T; N],
    head: u32,  // oldest valid entry
    tail: u32,  // next insert position
    count: u32,
}

impl<T: Copy + Default, const N: usize> Default for RingBuffer<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy + Default, const N: usize> RingBuffer<T, N> {
    const MASK: usize = N - 1;

    /// Create new ring buffer
    /// N must be power of 2
    #[inline]
    pub fn new() -> Self {
        const { assert!(N.is_power_of_two(), "Capacity must be power of 2") };
        Self {
            data: [T::default(); N],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    /// Get buffer capacity
    #[inline(always)]
    pub const fn capacity(&self) -> usize {
        N
    }

    /// Get number of items in buffer
    #[inline(always)]
    pub const fn len(&self) -> u32 {
        self.count
    }

    /// Check if buffer is empty
    #[inline(always)]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Check if buffer is full
    #[inline(always)]
    pub const fn is_full(&self) -> bool {
        self.count as usize >= N
    }

    /// Get index using bitwise AND (NOT modulo)
    #[inline(always)]
    const fn index(pos: u32) -> usize {
        (pos as usize) & Self::MASK
    }

    /// Push item to tail
    #[inline]
    pub fn push(&mut self, item: T) -> bool {
        if self.is_full() {
            return false;
        }
        let idx = Self::index(self.tail);
        self.data[idx] = item;
        self.tail = self.tail.wrapping_add(1);
        self.count = self.count.wrapping_add(1);
        true
    }

    /// Pop item from head
    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        if self.is_empty() {
            return None;
        }
        let idx = Self::index(self.head);
        let item = self.data[idx];
        self.head = self.head.wrapping_add(1);
        self.count = self.count.wrapping_sub(1);
        Some(item)
    }

    /// Get reference by sequence number
    #[inline(always)]
    pub fn get(&self, seq: u32) -> Option<&T> {
        let idx = Self::index(seq);
        Some(&self.data[idx])
    }

    /// Get mutable reference by sequence number
    #[inline(always)]
    pub fn get_mut(&mut self, seq: u32) -> Option<&mut T> {
        let idx = Self::index(seq);
        Some(&mut self.data[idx])
    }

    /// Clear all entries
    #[inline]
    pub fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.count = 0;
    }
}

/// Send buffer specialized for KCP
///
/// Preallocated buffer for outgoing segments with O(1) access.
/// Uses sequence number for indexing via bitwise AND.
#[repr(C)]
pub struct SendBuffer {
    pub(crate) segments: [SendSegment; SND_BUF_CAPACITY],
    pub(crate) data: [u8; SND_BUF_CAPACITY * MSS_DEFAULT],
    snd_una: Sequence,  // oldest unacked
    snd_nxt: Sequence,  // next to send
    count: u32,
}

impl Default for SendBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl SendBuffer {
    /// Create new send buffer with preallocated storage
    pub const fn new() -> Self {
        Self {
            segments: [SendSegment {
                sn: Sequence(0),
                resend_ts: 0,
                rto: RTO_DEFAULT,
                fastack: 0,
                xmit: 0,
                state: SegmentState::Empty,
                frg: 0,
                data_len: 0,
                ts: 0,
                data_offset: 0,
            }; SND_BUF_CAPACITY],
            data: [0u8; SND_BUF_CAPACITY * MSS_DEFAULT],
            snd_una: Sequence(0),
            snd_nxt: Sequence(0),
            count: 0,
        }
    }

    /// Get number of segments in buffer
    #[inline(always)]
    pub const fn len(&self) -> u32 {
        self.count
    }

    /// Check if buffer is full
    #[inline(always)]
    pub const fn is_full(&self) -> bool {
        self.count as usize >= SND_BUF_CAPACITY
    }

    #[inline(always)]
    fn seq_to_index(sn: Sequence) -> usize {
        sn.to_index(SND_BUF_MASK)
    }

    /// Insert new segment
    #[inline]
    pub fn insert(&mut self, sn: Sequence, frg: u8, data: &[u8]) -> bool {
        if self.is_full() || data.len() > MSS_DEFAULT {
            return false;
        }

        let idx = Self::seq_to_index(sn);
        let data_offset = idx * MSS_DEFAULT;

        // Copy data to preallocated buffer
        self.data[data_offset..data_offset + data.len()].copy_from_slice(data);

        self.segments[idx] = SendSegment {
            sn,
            resend_ts: 0,
            rto: RTO_DEFAULT,
            fastack: 0,
            xmit: 0,
            state: SegmentState::Pending,
            frg,
            data_len: data.len() as u16,
            ts: 0,
            data_offset: data_offset as u32,
        };

        self.count = self.count.wrapping_add(1);
        true
    }

    /// Get segment by sequence number
    #[inline(always)]
    pub fn get(&self, sn: Sequence) -> Option<&SendSegment> {
        let idx = Self::seq_to_index(sn);
        let seg = &self.segments[idx];
        if seg.state != SegmentState::Empty && seg.sn == sn {
            Some(seg)
        } else {
            None
        }
    }

    /// Get mutable segment
    #[inline(always)]
    pub fn get_mut(&mut self, sn: Sequence) -> Option<&mut SendSegment> {
        let idx = Self::seq_to_index(sn);
        let seg = &mut self.segments[idx];
        if seg.state != SegmentState::Empty && seg.sn == sn {
            Some(seg)
        } else {
            None
        }
    }

    /// Get segment data
    #[inline]
    pub fn get_data(&self, seg: &SendSegment) -> &[u8] {
        let offset = seg.data_offset as usize;
        &self.data[offset..offset + seg.data_len as usize]
    }

    /// Mark segment as acked
    #[inline]
    pub fn ack(&mut self, sn: Sequence) -> bool {
        if let Some(seg) = self.get_mut(sn)
            && seg.state == SegmentState::Sent
        {
            seg.state = SegmentState::Acked;
            self.count = self.count.saturating_sub(1);
            return true;
        }
        false
    }

    /// Remove all segments with sn < una
    #[inline]
    pub fn shrink(&mut self, una: Sequence) {
        while self.count > 0 {
            let idx = Self::seq_to_index(self.snd_una);
            let seg = &mut self.segments[idx];

            if seg.state != SegmentState::Empty && seg.sn.is_before(una) {
                seg.state = SegmentState::Empty;
                self.snd_una = self.snd_una.increment();
                self.count = self.count.saturating_sub(1);
            } else {
                break;
            }
        }
    }

    /// Iterator over segments that need sending
    #[inline]
    pub fn iter_pending(&self) -> impl Iterator<Item = (usize, &SendSegment)> {
        self.segments.iter().enumerate().filter(|(_, s)| {
            s.state == SegmentState::Pending || s.state == SegmentState::Sent
        })
    }

    /// Get indices of pending segments (for safe mutation)
    #[inline]
    pub fn pending_indices(&self) -> [Option<usize>; SND_BUF_CAPACITY] {
        let mut indices = [None; SND_BUF_CAPACITY];
        let mut count = 0;
        for (idx, seg) in self.segments.iter().enumerate() {
            if seg.state == SegmentState::Pending || seg.state == SegmentState::Sent {
                indices[count] = Some(idx);
                count += 1;
            }
        }
        indices
    }

    /// Get segment by index (unchecked bounds for hot path)
    #[inline(always)]
    pub fn get_by_index(&self, idx: usize) -> &SendSegment {
        &self.segments[idx]
    }

    /// Get mutable segment by index
    #[inline(always)]
    pub fn get_mut_by_index(&mut self, idx: usize) -> &mut SendSegment {
        &mut self.segments[idx]
    }

    /// Get data by index
    #[inline]
    pub fn get_data_by_index(&self, idx: usize) -> &[u8] {
        let seg = &self.segments[idx];
        let offset = seg.data_offset as usize;
        &self.data[offset..offset + seg.data_len as usize]
    }

    /// Increment fastack counter for segments before sn
    #[inline]
    pub fn increment_fastack(&mut self, sn: Sequence, una: Sequence) {
        for seg in self.segments.iter_mut() {
            if seg.state == SegmentState::Sent {
                // Only for segments in flight: una <= seg.sn < sn
                if !seg.sn.is_before(una) && seg.sn.is_before(sn) {
                    seg.fastack = seg.fastack.saturating_add(1);
                }
            }
        }
    }

    /// Get snd_una (oldest unacked)
    #[inline(always)]
    pub const fn snd_una(&self) -> Sequence {
        self.snd_una
    }

    /// Set snd_una
    #[inline(always)]
    pub fn set_snd_una(&mut self, una: Sequence) {
        self.snd_una = una;
    }

    /// Get snd_nxt (next to send)
    #[inline(always)]
    pub const fn snd_nxt(&self) -> Sequence {
        self.snd_nxt
    }

    /// Set snd_nxt
    #[inline(always)]
    pub fn set_snd_nxt(&mut self, nxt: Sequence) {
        self.snd_nxt = nxt;
    }

    /// Check if empty
    #[inline(always)]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// Receive buffer for reassembly
///
/// Preallocated buffer for incoming segments with O(1) access.
/// Handles out-of-order delivery and fragment reassembly.
#[repr(C)]
pub struct RecvBuffer {
    segments: [RecvSegment; RCV_BUF_CAPACITY],
    data: [u8; RCV_BUF_CAPACITY * MSS_DEFAULT],
    rcv_nxt: Sequence,
    count: u32,
}

impl Default for RecvBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl RecvBuffer {
    /// Create new receive buffer with preallocated storage
    pub const fn new() -> Self {
        Self {
            segments: [RecvSegment {
                sn: Sequence(0),
                frg: 0,
                state: SegmentState::Empty,
                data_len: 0,
                data_offset: 0,
            }; RCV_BUF_CAPACITY],
            data: [0u8; RCV_BUF_CAPACITY * MSS_DEFAULT],
            rcv_nxt: Sequence(0),
            count: 0,
        }
    }

    #[inline(always)]
    fn seq_to_index(sn: Sequence) -> usize {
        sn.to_index(RCV_BUF_MASK)
    }

    /// Insert received segment
    #[inline]
    pub fn insert(&mut self, sn: Sequence, frg: u8, data: &[u8]) -> bool {
        // Check if within receive window
        let rcv_wnd_end = self.rcv_nxt.add(RCV_BUF_CAPACITY as u32);
        if !sn.is_in_range(self.rcv_nxt, rcv_wnd_end) {
            return false;
        }

        let idx = Self::seq_to_index(sn);

        // Check for duplicate
        if self.segments[idx].state != SegmentState::Empty
            && self.segments[idx].sn == sn {
            return false;  // Duplicate
        }

        let data_offset = idx * MSS_DEFAULT;
        let data_len = data.len().min(MSS_DEFAULT);

        self.data[data_offset..data_offset + data_len].copy_from_slice(&data[..data_len]);

        self.segments[idx] = RecvSegment {
            sn,
            frg,
            state: SegmentState::Pending,
            data_len: data_len as u16,
            data_offset: data_offset as u32,
        };

        self.count = self.count.wrapping_add(1);
        true
    }

    /// Get segment data
    #[inline]
    pub fn get_data(&self, seg: &RecvSegment) -> &[u8] {
        let offset = seg.data_offset as usize;
        &self.data[offset..offset + seg.data_len as usize]
    }

    /// Try to read complete message
    /// Returns number of bytes read, 0 if no complete message
    #[inline]
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        // Check if first segment is ready
        let idx = Self::seq_to_index(self.rcv_nxt);
        if self.segments[idx].state != SegmentState::Pending
            || self.segments[idx].sn != self.rcv_nxt {
            return 0;
        }

        // Calculate total message size
        let first_frg = self.segments[idx].frg;
        let msg_segments = first_frg as usize + 1;

        // Check if all fragments are available
        let mut total_len = 0usize;
        for i in 0..msg_segments {
            let sn = self.rcv_nxt.add(i as u32);
            let idx = Self::seq_to_index(sn);
            let seg = &self.segments[idx];

            if seg.state != SegmentState::Pending || seg.sn != sn {
                return 0;  // Incomplete
            }

            let expected_frg = (msg_segments - 1 - i) as u8;
            if seg.frg != expected_frg {
                return 0;  // Fragment mismatch
            }

            total_len = total_len.wrapping_add(seg.data_len as usize);
        }

        if total_len > buf.len() {
            return 0;  // Buffer too small
        }

        // Copy data
        let mut offset = 0usize;
        for i in 0..msg_segments {
            let sn = self.rcv_nxt.add(i as u32);
            let idx = Self::seq_to_index(sn);
            let seg = &self.segments[idx];

            let data = self.get_data(seg);
            buf[offset..offset + data.len()].copy_from_slice(data);
            offset = offset.wrapping_add(data.len());
        }

        // Clear segments
        for i in 0..msg_segments {
            let sn = self.rcv_nxt.add(i as u32);
            let idx = Self::seq_to_index(sn);
            self.segments[idx].state = SegmentState::Empty;
            self.count = self.count.saturating_sub(1);
        }

        self.rcv_nxt = self.rcv_nxt.add(msg_segments as u32);
        offset
    }

    /// Get next expected sequence number
    #[inline(always)]
    pub const fn rcv_nxt(&self) -> Sequence {
        self.rcv_nxt
    }
}

/// ACK list for pending acknowledgments
///
/// Ring buffer for queuing ACKs to be sent.
/// Uses FIFO ordering with O(1) push/pop.
#[repr(C)]
pub struct AckList {
    entries: [AckEntry; ACK_LIST_CAPACITY],
    head: u32,
    tail: u32,
    count: u32,
}

impl Default for AckList {
    fn default() -> Self {
        Self::new()
    }
}

impl AckList {
    /// Create new ACK list
    pub const fn new() -> Self {
        Self {
            entries: [AckEntry { sn: 0, ts: 0 }; ACK_LIST_CAPACITY],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    #[inline(always)]
    const fn index(pos: u32) -> usize {
        (pos as usize) & ACK_LIST_MASK
    }

    /// Push ACK entry to queue
    #[inline]
    pub fn push(&mut self, sn: u32, ts: u32) -> bool {
        if self.count as usize >= ACK_LIST_CAPACITY {
            return false;
        }
        let idx = Self::index(self.tail);
        self.entries[idx] = AckEntry { sn, ts };
        self.tail = self.tail.wrapping_add(1);
        self.count = self.count.wrapping_add(1);
        true
    }

    /// Pop ACK entry from queue
    #[inline]
    pub fn pop(&mut self) -> Option<AckEntry> {
        if self.count == 0 {
            return None;
        }
        let idx = Self::index(self.head);
        let entry = self.entries[idx];
        self.head = self.head.wrapping_add(1);
        self.count = self.count.wrapping_sub(1);
        Some(entry)
    }

    /// Clear all entries
    #[inline]
    pub fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.count = 0;
    }

    /// Get number of pending ACKs
    #[inline(always)]
    pub const fn len(&self) -> u32 {
        self.count
    }

    /// Check if list is empty
    #[inline(always)]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }
}