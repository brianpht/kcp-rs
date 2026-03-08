//! KCP Segment structure
//!
//! Fixed-size, cache-aligned, no heap allocation

use crate::constants::*;
use crate::sequence::Sequence;

/// Segment state in buffer
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum SegmentState {
    /// Slot is empty/available
    #[default]
    Empty = 0,
    /// Segment is pending (queued, not yet sent)
    Pending = 1,
    /// Segment has been sent, awaiting ACK
    Sent = 2,
    /// Segment has been acknowledged
    Acked = 3,
}

/// Segment header - 24 bytes, matches wire format exactly
/// 
/// Wire format (little-endian):
/// ```text
/// | conv (4) | cmd (1) | frg (1) | wnd (2) | ts (4) | sn (4) | una (4) | len (4) |
/// ```
#[derive(Clone, Copy, Debug, Default)]
#[repr(C, packed)]
pub struct SegmentHeader {
    /// Conversation ID
    pub conv: u32,
    /// Command type (CMD_PUSH, CMD_ACK, CMD_WASK, CMD_WINS)
    pub cmd: u8,
    /// Fragment index (0 = last fragment)
    pub frg: u8,
    /// Window size
    pub wnd: u16,
    /// Timestamp
    pub ts: u32,
    /// Sequence number
    pub sn: u32,
    /// Unacknowledged sequence number
    pub una: u32,
    /// Data length
    pub len: u32,
}

impl SegmentHeader {
    /// Encode header to buffer (little-endian)
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

    /// Decode header from buffer (little-endian)
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

/// Send segment - contains transmission metadata
/// Size: optimized for cache line (≤64 bytes for hot fields)
#[derive(Clone, Copy)]
#[repr(C)]
pub struct SendSegment {
    // Hot fields first (accessed in hot path)
    /// Sequence number
    pub sn: Sequence,
    /// Resend timestamp
    pub resend_ts: u32,
    /// Retransmission timeout
    pub rto: u32,
    /// Fast ACK count
    pub fastack: u16,
    /// Transmission count
    pub xmit: u16,
    /// Current state
    pub state: SegmentState,
    /// Fragment index
    pub frg: u8,
    /// Data length
    pub data_len: u16,
    // 20 bytes so far

    // Cold fields
    /// Send timestamp
    pub ts: u32,
    // 24 bytes total for hot struct

    /// Offset in data buffer
    pub data_offset: u32,
}

impl Default for SendSegment {
    fn default() -> Self {
        Self {
            sn: Sequence::new(0),
            resend_ts: 0,
            rto: RTO_DEFAULT,
            fastack: 0,
            xmit: 0,
            state: SegmentState::Empty,
            frg: 0,
            data_len: 0,
            ts: 0,
            data_offset: 0,
        }
    }
}

/// Receive segment - for reassembly
#[derive(Clone, Copy)]
#[repr(C)]
pub struct RecvSegment {
    /// Sequence number
    pub sn: Sequence,
    /// Fragment index
    pub frg: u8,
    /// Current state
    pub state: SegmentState,
    /// Data length
    pub data_len: u16,
    /// Offset in data buffer
    pub data_offset: u32,
}

impl Default for RecvSegment {
    fn default() -> Self {
        Self {
            sn: Sequence::new(0),
            frg: 0,
            state: SegmentState::Empty,
            data_len: 0,
            data_offset: 0,
        }
    }
}

/// ACK entry for pending acknowledgments
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct AckEntry {
    /// Sequence number being acknowledged
    pub sn: u32,
    /// Timestamp of original packet
    pub ts: u32,
}