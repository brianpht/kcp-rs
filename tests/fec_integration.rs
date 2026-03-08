//! FEC Integration tests

#![cfg(feature = "fec")]

use kcp_rs::fec::{
    FecConfig, FecShardHeader, FecEncoder, FecDecoder,
    FecSendBuffer, FecRecvBuffer,
};

#[test]
fn test_fec_encode_parity_generation() {
    let config = FecConfig::new(2, 1); // k=2, m=1 for simpler test

    let mut encoder = FecEncoder::new(config);

    // Create 2 data shards
    let data0 = [0x11, 0x22, 0x33, 0x44];
    let data1 = [0xAA, 0xBB, 0xCC, 0xDD];

    // Encode to generate parity
    let data_refs: [&[u8]; 2] = [&data0, &data1];
    let mut parity0 = [0u8; 4];
    {
        let mut parity_refs: [&mut [u8]; 1] = [&mut parity0];
        assert!(encoder.encode(&data_refs, &mut parity_refs, 4));
    }

    // Verify parity is non-zero (unless data happens to XOR to zero)
    // With Vandermonde encoding, parity should generally be non-trivial
    assert!(parity0.iter().any(|&x| x != 0));
}

#[test]
fn test_fec_send_buffer() {
    let config = FecConfig::new(2, 1);
    let mut buffer = FecSendBuffer::new(config);

    // Add first data shard
    let result = buffer.add_data(&[0x11, 0x22, 0x33]);
    assert!(result.is_some());
    let (group_id, shard_idx) = result.unwrap();
    assert_eq!(group_id, 0);
    assert_eq!(shard_idx, 0);

    // Not complete yet
    assert!(!buffer.is_group_complete());

    // Add second data shard
    let result = buffer.add_data(&[0x44, 0x55, 0x66]);
    assert!(result.is_some());
    let (group_id, shard_idx) = result.unwrap();
    assert_eq!(group_id, 0);
    assert_eq!(shard_idx, 1);

    // Now complete
    assert!(buffer.is_group_complete());

    // Finalize
    let group = buffer.finalize_group();
    assert!(group.is_some());

    // Advance to next group
    buffer.advance_group();
    assert_eq!(buffer.current_group().group_id, 1);
}

#[test]
fn test_fec_recv_buffer() {
    let config = FecConfig::new(2, 1);
    let mut buffer = FecRecvBuffer::new(config);

    // Receive first shard
    let header1 = FecShardHeader::new(0, 0, 3);
    assert!(buffer.add_shard(&header1, &[0x11, 0x22], 100));

    // Receive second shard
    let header2 = FecShardHeader::new(0, 1, 3);
    assert!(buffer.add_shard(&header2, &[0x33, 0x44], 100));

    // Get group
    let group = buffer.get_group(0);
    assert!(group.is_some());
    assert_eq!(group.unwrap().recv_count, 2);

    // Get data shards
    let shard0 = buffer.get_data_shard(0, 0);
    assert!(shard0.is_some());
    assert_eq!(shard0.unwrap(), &[0x11, 0x22]);
}

#[test]
fn test_fec_header_roundtrip() {
    let header = FecShardHeader::new(0x1234, 5, 6);
    let mut buf = [0u8; 4];

    header.encode(&mut buf).unwrap();

    let decoded = FecShardHeader::decode(&buf).unwrap();
    assert_eq!(decoded.group_id, 0x1234);
    assert_eq!(decoded.shard_idx, 5);
    assert_eq!(decoded.shard_count, 6);
}

#[test]
fn test_fec_config_overhead() {
    // k=4, m=2 -> 50% overhead
    let balanced = FecConfig::balanced();
    assert_eq!(balanced.overhead_percent(), 50);

    // k=10, m=2 -> 20% overhead
    let efficient = FecConfig::bandwidth_efficient();
    assert_eq!(efficient.overhead_percent(), 20);

    // k=2, m=1 -> 50% overhead
    let low_latency = FecConfig::low_latency();
    assert_eq!(low_latency.overhead_percent(), 50);
}

#[test]
fn test_fec_send_group_get_shard() {
    let config = FecConfig::new(2, 1);
    let mut buffer = FecSendBuffer::new(config);

    buffer.add_data(&[0x11, 0x22, 0x33]).unwrap();
    buffer.add_data(&[0x44, 0x55, 0x66, 0x77]).unwrap();

    // Finalize to generate parity
    buffer.finalize_group().unwrap();

    let group = buffer.current_group();

    // Check data shard 0 - size is original size (3), but buffer is padded for encoding
    let (data, header) = group.get_shard(0, &config).unwrap();
    assert_eq!(data.len(), 3); // Original size preserved
    assert_eq!(data, &[0x11, 0x22, 0x33]);
    assert_eq!(header.shard_idx, 0);
    assert!(header.is_data_shard(config.data_shards));

    // Check data shard 1
    let (data1, header1) = group.get_shard(1, &config).unwrap();
    assert_eq!(data1.len(), 4);
    assert_eq!(data1, &[0x44, 0x55, 0x66, 0x77]);
    assert_eq!(header1.shard_idx, 1);

    // Check parity shard (index 2)
    let (parity_data, parity_header) = group.get_shard(2, &config).unwrap();
    assert!(!parity_data.is_empty());
    assert_eq!(parity_header.shard_idx, 2);
    assert!(!parity_header.is_data_shard(config.data_shards));
}

#[test]
fn test_fec_encode_decode_no_loss() {
    // Test that decode succeeds when all shards are present
    let config = FecConfig::new(4, 2); // k=4, m=2

    let mut encoder = FecEncoder::new(config);
    let mut decoder = FecDecoder::new(config);

    let shard_size = 8;

    // Create 4 data shards
    let original_data = [
        [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
        [0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18],
        [0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28],
        [0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38],
    ];

    // Generate parity using contiguous encoding
    let data_flat: Vec<u8> = original_data.iter().flatten().copied().collect();
    let mut parity_flat = vec![0u8; 2 * shard_size];

    assert!(encoder.encode_contiguous(&data_flat, &mut parity_flat, shard_size));

    // Copy to separate buffers
    let mut shard0 = original_data[0];
    let mut shard1 = original_data[1];
    let mut shard2 = original_data[2];
    let mut shard3 = original_data[3];
    let mut shard4 = [0u8; 8];
    let mut shard5 = [0u8; 8];
    shard4.copy_from_slice(&parity_flat[0..8]);
    shard5.copy_from_slice(&parity_flat[8..16]);

    // Create Option array for decoder (all present)
    let mut shard_refs: [Option<&mut [u8]>; 6] = [
        Some(&mut shard0),
        Some(&mut shard1),
        Some(&mut shard2),
        Some(&mut shard3),
        Some(&mut shard4),
        Some(&mut shard5),
    ];

    // Decode should succeed (no recovery needed)
    assert!(decoder.decode(&mut shard_refs, shard_size));

    // Verify data unchanged
    assert_eq!(shard0, original_data[0]);
    assert_eq!(shard1, original_data[1]);
    assert_eq!(shard2, original_data[2]);
    assert_eq!(shard3, original_data[3]);
}

#[test]
fn test_fec_encode_decode_with_recovery() {
    // Test recovery of missing data shards using decode_with_erasures
    let config = FecConfig::new(2, 2); // k=2, m=2

    let mut encoder = FecEncoder::new(config);
    let mut decoder = FecDecoder::new(config);

    let shard_size = 4;

    // Original data shards
    let original_data = [
        [0x11u8, 0x22, 0x33, 0x44],
        [0xAAu8, 0xBB, 0xCC, 0xDD],
    ];

    // Generate parity
    let data_flat: Vec<u8> = original_data.iter().flatten().copied().collect();
    let mut parity_flat = vec![0u8; 2 * shard_size];

    assert!(encoder.encode_contiguous(&data_flat, &mut parity_flat, shard_size));

    // Copy all to separate buffers
    let mut shard0 = [0u8; 4]; // Will be recovered (erased)
    let mut shard1 = original_data[1]; // Present
    let mut shard2 = [0u8; 4]; // Parity 0 - present
    let mut shard3 = [0u8; 4]; // Parity 1 - present
    shard2.copy_from_slice(&parity_flat[0..4]);
    shard3.copy_from_slice(&parity_flat[4..8]);

    // Build shards array with all buffers
    let mut shards: [&mut [u8]; 4] = [
        &mut shard0,
        &mut shard1,
        &mut shard2,
        &mut shard3,
    ];

    // Mark shard0 as erased (missing)
    let erasures = [true, false, false, false];

    // Decode - this should recover shard0
    assert!(decoder.decode_with_erasures(&mut shards, &erasures, shard_size));

    // Verify recovered data matches original
    assert_eq!(shard0, original_data[0]);
}

#[test]
fn test_fec_insufficient_shards() {
    let config = FecConfig::new(4, 2); // k=4, m=2

    let mut decoder = FecDecoder::new(config);

    let shard_size = 8;

    // Only 3 shards present (need 4)
    let mut shard0 = [0u8; 8];
    let mut shard1 = [0u8; 8];
    let mut shard4 = [0u8; 8];

    let mut shards: [Option<&mut [u8]>; 6] = [
        Some(&mut shard0),  // Present
        Some(&mut shard1),  // Present
        None,               // Missing
        None,               // Missing
        Some(&mut shard4),  // Present (parity)
        None,               // Missing (parity)
    ];

    // Decode should fail (only 3 shards, need 4)
    assert!(!decoder.decode(&mut shards, shard_size));
}

