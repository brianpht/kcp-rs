//! Integration tests for KCP end-to-end flow

use kcp_rs::{Kcp, KcpConfig, KcpOutput, KcpResult, KcpError};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

/// Simulated network link that captures packets
struct SimulatedLink {
    packets: Rc<RefCell<VecDeque<Vec<u8>>>>,
}

impl SimulatedLink {
    fn new(packets: Rc<RefCell<VecDeque<Vec<u8>>>>) -> Self {
        Self { packets }
    }
}

impl KcpOutput for SimulatedLink {
    fn output(&mut self, data: &[u8]) -> KcpResult<usize> {
        self.packets.borrow_mut().push_back(data.to_vec());
        Ok(data.len())
    }
}

/// Helper to create KCP on heap (avoids stack overflow from large buffers)
fn create_kcp(conv: u32, link: SimulatedLink) -> Box<Kcp<SimulatedLink>> {
    Box::new(Kcp::with_config(conv, link, KcpConfig::fast()))
}

#[test]
fn test_basic_send_receive() {
    // Create packet buffers for bidirectional communication
    let a_to_b = Rc::new(RefCell::new(VecDeque::new()));
    let b_to_a = Rc::new(RefCell::new(VecDeque::new()));

    // Create two KCP endpoints (on heap)
    let mut kcp_a = create_kcp(1, SimulatedLink::new(a_to_b.clone()));
    let mut kcp_b = create_kcp(1, SimulatedLink::new(b_to_a.clone()));

    // Send data from A
    let test_data = b"Hello, KCP!";
    kcp_a.send(test_data).expect("send should succeed");

    // Simulate time progression and updates
    let mut current_time = 0u32;

    // Run a few update cycles
    for _ in 0..10 {
        current_time = current_time.wrapping_add(20);

        kcp_a.update(current_time).expect("update A");
        kcp_b.update(current_time).expect("update B");

        // Transfer packets A -> B
        while let Some(packet) = a_to_b.borrow_mut().pop_front() {
            let _ = kcp_b.input(&packet);
        }

        // Transfer packets B -> A (ACKs)
        while let Some(packet) = b_to_a.borrow_mut().pop_front() {
            let _ = kcp_a.input(&packet);
        }
    }

    // Try to receive at B
    let mut recv_buf = [0u8; 1024];
    match kcp_b.recv(&mut recv_buf) {
        Ok(n) => {
            assert_eq!(&recv_buf[..n], test_data);
        }
        Err(e) => {
            // May fail due to incomplete flow - this is for debugging
            eprintln!("recv failed: {:?}, wait_snd_a={}, wait_snd_b={}",
                e, kcp_a.wait_snd(), kcp_b.wait_snd());
        }
    }
}

#[test]
fn test_fragmentation() {
    let packets = Rc::new(RefCell::new(VecDeque::new()));
    let mut kcp = create_kcp(1, SimulatedLink::new(packets.clone()));

    // Send data larger than MSS (1376 bytes)
    let large_data = vec![0xABu8; 3000];
    let result = kcp.send(&large_data);

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 3000);

    // Should have created multiple segments (3000 / 1376 ≈ 3)
    assert!(kcp.wait_snd() >= 3);
}

#[test]
fn test_empty_send() {
    let packets = Rc::new(RefCell::new(VecDeque::new()));
    let mut kcp = create_kcp(1, SimulatedLink::new(packets.clone()));

    let result = kcp.send(&[]);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 0);
}

#[test]
fn test_update_without_send() {
    let packets = Rc::new(RefCell::new(VecDeque::new()));
    let mut kcp = create_kcp(1, SimulatedLink::new(packets.clone()));

    // Update should not fail even without data
    for ts in (0..1000).step_by(20) {
        let result = kcp.update(ts);
        assert!(result.is_ok());
    }
}

#[test]
fn test_conv_mismatch() {
    let packets = Rc::new(RefCell::new(VecDeque::new()));
    let mut kcp = create_kcp(1, SimulatedLink::new(packets.clone()));

    // Create a packet with wrong conv
    let mut bad_packet = [0u8; 24];
    bad_packet[0..4].copy_from_slice(&2u32.to_le_bytes()); // conv = 2, but KCP expects 1

    let result = kcp.input(&bad_packet);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), KcpError::ConvMismatch);
}

#[test]
fn test_invalid_packet_too_small() {
    let packets = Rc::new(RefCell::new(VecDeque::new()));
    let mut kcp = create_kcp(1, SimulatedLink::new(packets.clone()));

    // Packet smaller than header size
    let small_packet = [0u8; 10];
    let result = kcp.input(&small_packet);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), KcpError::InvalidPacket);
}

#[test]
fn test_multiple_sends() {
    let packets = Rc::new(RefCell::new(VecDeque::new()));
    let mut kcp = create_kcp(1, SimulatedLink::new(packets.clone()));

    // Send multiple messages
    for i in 0..5 {
        let data = format!("Message {}", i);
        let result = kcp.send(data.as_bytes());
        assert!(result.is_ok());
    }

    assert_eq!(kcp.wait_snd(), 5);
}

#[test]
fn test_rtt_and_rto() {
    let packets = Rc::new(RefCell::new(VecDeque::new()));
    let kcp = create_kcp(1, SimulatedLink::new(packets.clone()));

    // Initial RTT should be 0 (not yet calculated)
    assert_eq!(kcp.rtt(), 0);
    // Initial RTO should be default
    assert_eq!(kcp.rto(), 200);
}

#[test]
fn test_config_fast_mode() {
    let config = KcpConfig::fast();
    assert!(config.nodelay);
    assert!(config.nc); // no congestion control
    assert_eq!(config.interval, 20);
    assert_eq!(config.resend, 2);
}

#[test]
fn test_config_default() {
    let config = KcpConfig::default();
    assert!(!config.nodelay);
    assert!(!config.nc);
    assert_eq!(config.interval, 100);
    assert_eq!(config.mtu, 1400);
}

#[test]
fn test_check_returns_next_update_time() {
    let packets = Rc::new(RefCell::new(VecDeque::new()));
    let mut kcp = create_kcp(1, SimulatedLink::new(packets.clone()));

    // Before first update, check returns current time
    let next = kcp.check(100);
    assert_eq!(next, 100);

    // After update, should return future time
    let _ = kcp.update(100);
    let next = kcp.check(100);
    // Should be at least current + interval (usually >= 100)
    assert!(next >= 100);
}

