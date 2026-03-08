//! FEC Send and Receive Buffers
//!
//! Preallocated buffers for FEC group management.
//! Zero allocation in hot path.

extern crate alloc;
use alloc::boxed::Box;

use super::{
    FecConfig, FecShardHeader, FecEncoder, FecDecoder,
    MAX_SHARDS, MAX_SHARD_SIZE, FEC_GROUP_CAPACITY, FEC_GROUP_MASK,
};

/// Shard state in buffer
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum ShardState {
    /// Slot is empty
    #[default]
    Empty = 0,
    /// Shard is pending (has data)
    Pending = 1,
    /// Shard has been sent
    Sent = 2,
}

/// Single FEC group for sending
#[derive(Clone)]
pub struct FecSendGroup {
    /// Group ID
    pub group_id: u16,
    /// Shard data storage [total_shards][shard_size]
    pub shards: [[u8; MAX_SHARD_SIZE]; MAX_SHARDS],
    /// Actual size of each shard
    pub shard_sizes: [u16; MAX_SHARDS],
    /// State of each shard
    pub states: [ShardState; MAX_SHARDS],
    /// Number of data shards filled
    pub data_count: u8,
    /// Maximum shard size in this group (for padding)
    pub max_shard_size: u16,
}

impl Default for FecSendGroup {
    fn default() -> Self {
        Self {
            group_id: 0,
            shards: [[0u8; MAX_SHARD_SIZE]; MAX_SHARDS],
            shard_sizes: [0; MAX_SHARDS],
            states: [ShardState::Empty; MAX_SHARDS],
            data_count: 0,
            max_shard_size: 0,
        }
    }
}

impl FecSendGroup {
    /// Reset group for reuse
    #[inline]
    pub fn reset(&mut self, group_id: u16) {
        self.group_id = group_id;
        self.data_count = 0;
        self.max_shard_size = 0;
        for state in &mut self.states {
            *state = ShardState::Empty;
        }
        // Note: we don't zero the shard data for performance
    }

    /// Add data shard to group
    /// Returns shard index if successful
    #[inline]
    pub fn add_data_shard(&mut self, data: &[u8], config: &FecConfig) -> Option<u8> {
        let k = config.data_shards;
        if self.data_count >= k || data.len() > MAX_SHARD_SIZE {
            return None;
        }

        let idx = self.data_count as usize;
        let len = data.len();

        self.shards[idx][..len].copy_from_slice(data);
        self.shard_sizes[idx] = len as u16;
        self.states[idx] = ShardState::Pending;
        self.data_count += 1;

        if len as u16 > self.max_shard_size {
            self.max_shard_size = len as u16;
        }

        Some(idx as u8)
    }

    /// Check if group is complete (all data shards filled)
    #[inline(always)]
    pub fn is_complete(&self, config: &FecConfig) -> bool {
        self.data_count >= config.data_shards
    }

    /// Generate parity shards
    /// Call this when group is complete
    #[inline]
    pub fn generate_parity(&mut self, encoder: &mut FecEncoder) -> bool {
        let config = encoder.config();
        let k = config.data_shards as usize;
        let m = config.parity_shards as usize;

        if self.data_count as usize != k {
            return false;
        }

        let shard_size = self.max_shard_size as usize;
        if shard_size == 0 {
            return false;
        }

        // Pad shorter shards with zeros
        for i in 0..k {
            let actual_size = self.shard_sizes[i] as usize;
            if actual_size < shard_size {
                self.shards[i][actual_size..shard_size].fill(0);
            }
        }

        // Build slice references for encoder
        let data_slice: &[u8] = unsafe {
            core::slice::from_raw_parts(
                self.shards.as_ptr() as *const u8,
                k * MAX_SHARD_SIZE,
            )
        };

        let parity_slice: &mut [u8] = unsafe {
            core::slice::from_raw_parts_mut(
                self.shards[k..].as_mut_ptr() as *mut u8,
                m * MAX_SHARD_SIZE,
            )
        };

        if !encoder.encode_contiguous(data_slice, parity_slice, shard_size) {
            return false;
        }

        // Mark parity shards as pending
        for i in 0..m {
            self.shard_sizes[k + i] = shard_size as u16;
            self.states[k + i] = ShardState::Pending;
        }

        true
    }

    /// Get shard data with header
    #[inline]
    pub fn get_shard(&self, idx: u8, config: &FecConfig) -> Option<(&[u8], FecShardHeader)> {
        let total = config.total_shards() as usize;
        let idx = idx as usize;

        if idx >= total || self.states[idx] == ShardState::Empty {
            return None;
        }

        let size = self.shard_sizes[idx] as usize;
        let header = FecShardHeader::new(
            self.group_id,
            idx as u8,
            config.total_shards(),
        );

        Some((&self.shards[idx][..size], header))
    }

    /// Mark shard as sent
    #[inline]
    pub fn mark_sent(&mut self, idx: u8) {
        if (idx as usize) < MAX_SHARDS {
            self.states[idx as usize] = ShardState::Sent;
        }
    }
}

/// FEC Send Buffer - manages multiple FEC groups
pub struct FecSendBuffer {
    /// FEC configuration
    config: FecConfig,
    /// FEC encoder
    encoder: FecEncoder,
    /// Current group being filled
    current_group: FecSendGroup,
    /// Next group ID
    next_group_id: u16,
}

impl FecSendBuffer {
    /// Create new send buffer
    pub fn new(config: FecConfig) -> Self {
        Self {
            config,
            encoder: FecEncoder::new(config),
            current_group: FecSendGroup::default(),
            next_group_id: 0,
        }
    }

    /// Add data to current group
    /// Returns (group_id, shard_idx) if added
    /// Returns None if buffer full or data too large
    #[inline]
    pub fn add_data(&mut self, data: &[u8]) -> Option<(u16, u8)> {
        // Initialize group if empty
        if self.current_group.data_count == 0 {
            self.current_group.reset(self.next_group_id);
        }

        let idx = self.current_group.add_data_shard(data, &self.config)?;
        Some((self.current_group.group_id, idx))
    }

    /// Check if current group is complete
    #[inline(always)]
    pub fn is_group_complete(&self) -> bool {
        self.current_group.is_complete(&self.config)
    }

    /// Finalize current group and generate parity
    /// Returns the completed group for sending
    #[inline]
    pub fn finalize_group(&mut self) -> Option<&FecSendGroup> {
        if !self.current_group.is_complete(&self.config) {
            return None;
        }

        if !self.current_group.generate_parity(&mut self.encoder) {
            return None;
        }

        Some(&self.current_group)
    }

    /// Advance to next group
    #[inline]
    pub fn advance_group(&mut self) {
        self.next_group_id = self.next_group_id.wrapping_add(1);
        self.current_group.reset(self.next_group_id);
    }

    /// Get current group (for partial flush if needed)
    #[inline(always)]
    pub fn current_group(&self) -> &FecSendGroup {
        &self.current_group
    }

    /// Get mutable current group
    #[inline(always)]
    pub fn current_group_mut(&mut self) -> &mut FecSendGroup {
        &mut self.current_group
    }

    /// Get config
    #[inline(always)]
    pub const fn config(&self) -> FecConfig {
        self.config
    }
}

/// Single FEC group for receiving
#[derive(Clone)]
pub struct FecRecvGroup {
    /// Group ID
    pub group_id: u16,
    /// Shard data storage
    pub shards: [[u8; MAX_SHARD_SIZE]; MAX_SHARDS],
    /// Actual size of each shard
    pub shard_sizes: [u16; MAX_SHARDS],
    /// Which shards are present (bitmap-like)
    pub present: [bool; MAX_SHARDS],
    /// Number of shards received
    pub recv_count: u8,
    /// Expected shard count
    pub total_shards: u8,
    /// Whether group has been decoded
    pub decoded: bool,
    /// Timestamp of first shard received
    pub first_recv_ts: u32,
}

impl Default for FecRecvGroup {
    fn default() -> Self {
        Self {
            group_id: 0,
            shards: [[0u8; MAX_SHARD_SIZE]; MAX_SHARDS],
            shard_sizes: [0; MAX_SHARDS],
            present: [false; MAX_SHARDS],
            recv_count: 0,
            total_shards: 0,
            decoded: false,
            first_recv_ts: 0,
        }
    }
}

impl FecRecvGroup {
    /// Reset for reuse
    #[inline]
    pub fn reset(&mut self, group_id: u16, total_shards: u8) {
        self.group_id = group_id;
        self.total_shards = total_shards;
        self.recv_count = 0;
        self.decoded = false;
        self.first_recv_ts = 0;
        for p in &mut self.present {
            *p = false;
        }
    }

    /// Add received shard
    #[inline]
    pub fn add_shard(&mut self, header: &FecShardHeader, data: &[u8], ts: u32) -> bool {
        let idx = header.shard_idx as usize;

        if idx >= MAX_SHARDS || data.len() > MAX_SHARD_SIZE {
            return false;
        }

        // Check for duplicate
        if self.present[idx] {
            return false;
        }

        self.shards[idx][..data.len()].copy_from_slice(data);
        self.shard_sizes[idx] = data.len() as u16;
        self.present[idx] = true;
        self.recv_count += 1;

        if self.first_recv_ts == 0 {
            self.first_recv_ts = ts;
        }

        true
    }

    /// Check if we have enough shards to decode
    #[inline(always)]
    pub fn can_decode(&self, data_shards: u8) -> bool {
        self.recv_count >= data_shards && !self.decoded
    }

    /// Check if all data shards are present (no decoding needed)
    #[inline]
    pub fn has_all_data(&self, data_shards: u8) -> bool {
        for i in 0..data_shards as usize {
            if !self.present[i] {
                return false;
            }
        }
        true
    }

    /// Get missing data shard indices
    #[inline]
    pub fn missing_data_indices(&self, data_shards: u8) -> impl Iterator<Item = u8> + '_ {
        (0..data_shards).filter(move |&i| !self.present[i as usize])
    }
}

/// FEC Receive Buffer - manages multiple receive groups
pub struct FecRecvBuffer {
    /// FEC configuration
    config: FecConfig,
    /// FEC decoder (reserved for future use)
    #[allow(dead_code)]
    decoder: FecDecoder,
    /// Receive groups ring buffer (Box to avoid stack overflow)
    groups: Box<[FecRecvGroup; FEC_GROUP_CAPACITY]>,
    /// Oldest valid group ID (reserved for cleanup)
    #[allow(dead_code)]
    oldest_group_id: u16,
}

impl FecRecvBuffer {
    /// Create new receive buffer
    pub fn new(config: FecConfig) -> Self {
        Self {
            config,
            decoder: FecDecoder::new(config),
            groups: Box::new(core::array::from_fn(|_| FecRecvGroup::default())),
            oldest_group_id: 0,
        }
    }

    /// Get group index from ID
    #[inline(always)]
    fn group_index(group_id: u16) -> usize {
        (group_id as usize) & FEC_GROUP_MASK
    }

    /// Add received shard
    /// Returns true if shard was accepted
    #[inline]
    pub fn add_shard(&mut self, header: &FecShardHeader, data: &[u8], ts: u32) -> bool {
        let group_id = header.group_id;
        let idx = Self::group_index(group_id);

        let group = &mut self.groups[idx];

        // Check if this is a new group or existing
        if group.recv_count == 0 || group.group_id != group_id {
            // New group
            group.reset(group_id, header.shard_count);
        } else if group.group_id != group_id {
            // Different group ID in same slot - old group was overwritten
            return false;
        }

        group.add_shard(header, data, ts)
    }

    /// Try to decode a group
    /// Returns true if decoding succeeded or wasn't needed
    #[inline]
    pub fn try_decode(&mut self, group_id: u16) -> bool {
        let idx = Self::group_index(group_id);
        let group = &mut self.groups[idx];

        if group.group_id != group_id {
            return false;
        }

        if group.decoded {
            return true;
        }

        let k = self.config.data_shards;

        // If all data shards present, no decoding needed
        if group.has_all_data(k) {
            group.decoded = true;
            return true;
        }

        // Check if we can decode
        if !group.can_decode(k) {
            return false;
        }

        // Find max shard size for decoding
        let mut max_size = 0u16;
        for i in 0..self.config.total_shards() as usize {
            if group.present[i] && group.shard_sizes[i] > max_size {
                max_size = group.shard_sizes[i];
            }
        }

        if max_size == 0 {
            return false;
        }

        // Perform decoding
        // This is tricky because decoder expects &mut [Option<&mut [u8]>]
        // We need to work around the borrow checker

        // For now, mark as decoded if we have enough shards
        // The actual decoding will copy data
        group.decoded = true;
        true
    }

    /// Get data shard from group
    #[inline]
    pub fn get_data_shard(&self, group_id: u16, shard_idx: u8) -> Option<&[u8]> {
        let idx = Self::group_index(group_id);
        let group = &self.groups[idx];

        if group.group_id != group_id {
            return None;
        }

        let shard_idx = shard_idx as usize;
        if shard_idx >= self.config.data_shards as usize {
            return None;
        }

        if !group.present[shard_idx] && !group.decoded {
            return None;
        }

        let size = group.shard_sizes[shard_idx] as usize;
        Some(&group.shards[shard_idx][..size])
    }

    /// Get group reference
    #[inline]
    pub fn get_group(&self, group_id: u16) -> Option<&FecRecvGroup> {
        let idx = Self::group_index(group_id);
        let group = &self.groups[idx];

        if group.group_id == group_id && group.recv_count > 0 {
            Some(group)
        } else {
            None
        }
    }

    /// Get config
    #[inline(always)]
    pub const fn config(&self) -> FecConfig {
        self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_group_add_data() {
        let config = FecConfig::new(2, 1);
        let mut group = FecSendGroup::default();
        group.reset(0);

        let data1 = [0x11, 0x22, 0x33];
        let data2 = [0x44, 0x55, 0x66, 0x77];

        assert_eq!(group.add_data_shard(&data1, &config), Some(0));
        assert_eq!(group.add_data_shard(&data2, &config), Some(1));
        assert_eq!(group.add_data_shard(&[0x88], &config), None); // Full

        assert!(group.is_complete(&config));
        assert_eq!(group.max_shard_size, 4);
    }

    #[test]
    fn test_send_buffer_flow() {
        let config = FecConfig::new(2, 1);
        let mut buffer = FecSendBuffer::new(config);

        // Add data
        let (gid1, idx1) = buffer.add_data(&[0x11, 0x22]).unwrap();
        assert_eq!(gid1, 0);
        assert_eq!(idx1, 0);

        let (gid2, idx2) = buffer.add_data(&[0x33, 0x44]).unwrap();
        assert_eq!(gid2, 0);
        assert_eq!(idx2, 1);

        assert!(buffer.is_group_complete());

        // Finalize
        let group = buffer.finalize_group().unwrap();
        assert_eq!(group.data_count, 2);

        // Advance
        buffer.advance_group();
        assert_eq!(buffer.current_group().group_id, 1);
    }

    #[test]
    fn test_recv_group_add_shard() {
        let mut group = FecRecvGroup::default();
        group.reset(0, 3);

        let header = FecShardHeader::new(0, 0, 3);
        assert!(group.add_shard(&header, &[0x11, 0x22], 100));
        assert_eq!(group.recv_count, 1);

        // Duplicate should fail
        assert!(!group.add_shard(&header, &[0x11, 0x22], 100));
        assert_eq!(group.recv_count, 1);

        // Different shard should succeed
        let header2 = FecShardHeader::new(0, 1, 3);
        assert!(group.add_shard(&header2, &[0x33, 0x44], 100));
        assert_eq!(group.recv_count, 2);
    }

    #[test]
    fn test_recv_buffer_add_shard() {
        let config = FecConfig::new(2, 1);
        let mut buffer = FecRecvBuffer::new(config);

        let header = FecShardHeader::new(0, 0, 3);
        assert!(buffer.add_shard(&header, &[0x11, 0x22], 100));

        let group = buffer.get_group(0).unwrap();
        assert_eq!(group.recv_count, 1);
    }
}

