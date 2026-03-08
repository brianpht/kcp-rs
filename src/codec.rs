//! Wire format encoding/decoding
//! All operations are allocation-free and use little-endian

use crate::constants::*;
use crate::segment::SegmentHeader;

/// Result of encoding a segment
#[derive(Debug, Clone, Copy)]
pub struct EncodeResult {
    /// Number of bytes written to buffer
    pub bytes_written: usize,
}

/// Result of decoding a segment
#[derive(Debug, Clone, Copy)]
pub struct DecodeResult {
    /// Decoded header
    pub header: SegmentHeader,
    /// Offset where data begins
    pub data_offset: usize,
    /// Total length of segment (header + data)
    pub total_len: usize,
}

/// Encode segment to buffer
#[inline]
pub fn encode_segment(
    buf: &mut [u8],
    header: &SegmentHeader,
    data: &[u8],
) -> Option<EncodeResult> {
    let total_len = HEADER_SIZE + data.len();
    if buf.len() < total_len {
        return None;
    }

    header.encode(&mut buf[..HEADER_SIZE])?;
    buf[HEADER_SIZE..total_len].copy_from_slice(data);

    Some(EncodeResult {
        bytes_written: total_len,
    })
}

/// Decode segment from buffer
#[inline]
pub fn decode_segment(buf: &[u8]) -> Option<DecodeResult> {
    if buf.len() < HEADER_SIZE {
        return None;
    }

    let header = SegmentHeader::decode(buf)?;
    let data_len = header.len as usize;
    let total_len = HEADER_SIZE + data_len;

    if buf.len() < total_len {
        return None;
    }

    Some(DecodeResult {
        header,
        data_offset: HEADER_SIZE,
        total_len,
    })
}

/// Validate conv field
#[inline(always)]
pub const fn validate_conv(buf: &[u8], expected: u32) -> bool {
    if buf.len() < 4 {
        return false;
    }
    let conv = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    conv == expected
}

/// Extract conv without full decode
#[inline(always)]
pub const fn peek_conv(buf: &[u8]) -> Option<u32> {
    if buf.len() < 4 {
        return None;
    }
    Some(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]))
}