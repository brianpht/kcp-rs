//! Forward Error Correction (FEC) using Reed-Solomon codes
//!
//! Design principles:
//! - Zero allocation in hot path
//! - Preallocated shard buffers
//! - Inline GF(2^8) arithmetic with lookup tables
//!
//! # FEC Group Structure
//! ```text
//! Group: [D0] [D1] [D2] [D3] [P0] [P1]
//!         └─── data shards ───┘ └─ parity ─┘
//!              k = 4              m = 2
//! ```
//!
//! Any k shards can reconstruct all k data shards.

mod gf256;
mod encoder;
mod decoder;
mod buffer;

pub use gf256::GF256;
pub use encoder::FecEncoder;
pub use decoder::FecDecoder;
pub use buffer::{FecSendBuffer, FecRecvBuffer};

#[cfg(not(feature = "small-buffers"))]
use crate::constants::MSS_DEFAULT;

/// FEC Configuration
#[derive(Clone, Copy, Debug)]
pub struct FecConfig {
    /// Number of data shards (k)
    pub data_shards: u8,
    /// Number of parity shards (m)
    pub parity_shards: u8,
}

impl Default for FecConfig {
    fn default() -> Self {
        Self {
            data_shards: 4,
            parity_shards: 2,
        }
    }
}

impl FecConfig {
    /// Create FEC config with specified shards
    ///
    /// # Panics
    /// Panics if data_shards + parity_shards > MAX_SHARDS
    pub const fn new(data_shards: u8, parity_shards: u8) -> Self {
        assert!(
            (data_shards as usize + parity_shards as usize) <= MAX_SHARDS,
            "Total shards exceeds maximum"
        );
        assert!(data_shards > 0, "Must have at least 1 data shard");
        assert!(parity_shards > 0, "Must have at least 1 parity shard");
        Self {
            data_shards,
            parity_shards,
        }
    }

    /// Total number of shards (k + m)
    #[inline(always)]
    pub const fn total_shards(&self) -> u8 {
        self.data_shards + self.parity_shards
    }

    /// Overhead ratio (parity/data)
    #[inline(always)]
    pub const fn overhead_percent(&self) -> u32 {
        (self.parity_shards as u32 * 100) / self.data_shards as u32
    }

    /// Low latency config: k=2, m=1 (50% overhead, recover from 1 loss)
    pub const fn low_latency() -> Self {
        Self {
            data_shards: 2,
            parity_shards: 1,
        }
    }

    /// Balanced config: k=4, m=2 (50% overhead, recover from 2 losses)
    pub const fn balanced() -> Self {
        Self {
            data_shards: 4,
            parity_shards: 2,
        }
    }

    /// High protection: k=8, m=4 (50% overhead, recover from 4 losses)
    pub const fn high_protection() -> Self {
        Self {
            data_shards: 8,
            parity_shards: 4,
        }
    }

    /// Bandwidth efficient: k=10, m=2 (20% overhead, recover from 2 losses)
    pub const fn bandwidth_efficient() -> Self {
        Self {
            data_shards: 10,
            parity_shards: 2,
        }
    }
}

/// Maximum number of shards per FEC group (must be power of 2 - 1 for GF(256))
pub const MAX_SHARDS: usize = 16;

/// Maximum shard size (same as MSS for simplicity)
#[cfg(feature = "small-buffers")]
pub const MAX_SHARD_SIZE: usize = 64;

/// Maximum shard size (same as MSS for simplicity)
#[cfg(not(feature = "small-buffers"))]
pub const MAX_SHARD_SIZE: usize = MSS_DEFAULT;

/// FEC group capacity (power of 2)
#[cfg(feature = "small-buffers")]
pub const FEC_GROUP_CAPACITY: usize = 8;

/// FEC group capacity (power of 2)
#[cfg(not(feature = "small-buffers"))]
pub const FEC_GROUP_CAPACITY: usize = 64;

/// FEC group mask for indexing
pub const FEC_GROUP_MASK: usize = FEC_GROUP_CAPACITY - 1;

// Compile-time assertions
const _: () = assert!(MAX_SHARDS <= 255, "MAX_SHARDS must fit in u8");
const _: () = assert!(FEC_GROUP_CAPACITY.is_power_of_two(), "FEC_GROUP_CAPACITY must be power of 2");

/// FEC shard header (prepended to each shard)
///
/// Wire format (4 bytes):
/// ```text
/// | group_id (2) | shard_idx (1) | shard_count (1) |
/// ```
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct FecShardHeader {
    /// FEC group identifier (wrapping counter)
    pub group_id: u16,
    /// Shard index within group (0..k+m-1)
    pub shard_idx: u8,
    /// Total shard count in group (k+m)
    pub shard_count: u8,
}

/// FEC header size in bytes
pub const FEC_HEADER_SIZE: usize = 4;

impl FecShardHeader {
    /// Create new shard header
    #[inline]
    pub const fn new(group_id: u16, shard_idx: u8, shard_count: u8) -> Self {
        Self {
            group_id,
            shard_idx,
            shard_count,
        }
    }

    /// Encode header to buffer (little-endian)
    #[inline]
    pub fn encode(&self, buf: &mut [u8]) -> Option<()> {
        if buf.len() < FEC_HEADER_SIZE {
            return None;
        }
        buf[0..2].copy_from_slice(&self.group_id.to_le_bytes());
        buf[2] = self.shard_idx;
        buf[3] = self.shard_count;
        Some(())
    }

    /// Decode header from buffer (little-endian)
    #[inline]
    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < FEC_HEADER_SIZE {
            return None;
        }
        Some(Self {
            group_id: u16::from_le_bytes([buf[0], buf[1]]),
            shard_idx: buf[2],
            shard_count: buf[3],
        })
    }

    /// Check if this is a data shard (not parity)
    #[inline(always)]
    pub const fn is_data_shard(&self, data_shards: u8) -> bool {
        self.shard_idx < data_shards
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fec_config_default() {
        let config = FecConfig::default();
        assert_eq!(config.data_shards, 4);
        assert_eq!(config.parity_shards, 2);
        assert_eq!(config.total_shards(), 6);
        assert_eq!(config.overhead_percent(), 50);
    }

    #[test]
    fn test_fec_config_presets() {
        let low = FecConfig::low_latency();
        assert_eq!(low.total_shards(), 3);

        let balanced = FecConfig::balanced();
        assert_eq!(balanced.total_shards(), 6);

        let high = FecConfig::high_protection();
        assert_eq!(high.total_shards(), 12);

        let efficient = FecConfig::bandwidth_efficient();
        assert_eq!(efficient.overhead_percent(), 20);
    }

    #[test]
    fn test_shard_header_encode_decode() {
        let header = FecShardHeader::new(0x1234, 3, 6);
        let mut buf = [0u8; 4];

        header.encode(&mut buf).unwrap();
        let decoded = FecShardHeader::decode(&buf).unwrap();

        assert_eq!(decoded.group_id, 0x1234);
        assert_eq!(decoded.shard_idx, 3);
        assert_eq!(decoded.shard_count, 6);
    }

    #[test]
    fn test_is_data_shard() {
        let data_shards = 4;

        assert!(FecShardHeader::new(0, 0, 6).is_data_shard(data_shards));
        assert!(FecShardHeader::new(0, 3, 6).is_data_shard(data_shards));
        assert!(!FecShardHeader::new(0, 4, 6).is_data_shard(data_shards));
        assert!(!FecShardHeader::new(0, 5, 6).is_data_shard(data_shards));
    }
}

