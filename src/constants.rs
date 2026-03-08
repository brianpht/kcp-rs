//! KCP Protocol Constants
//! All values are compile-time constants for maximum optimization

/// Command type: Push data segment
pub const CMD_PUSH: u8 = 81;
/// Command type: Acknowledgment
pub const CMD_ACK: u8 = 82;
/// Command type: Window probe (ask)
pub const CMD_WASK: u8 = 83;
/// Command type: Window size notification
pub const CMD_WINS: u8 = 84;

/// Header size: conv(4) + cmd(1) + frg(1) + wnd(2) + ts(4) + sn(4) + una(4) + len(4) = 24
pub const HEADER_SIZE: usize = 24;

/// Default MTU (Maximum Transmission Unit)
pub const MTU_DEFAULT: usize = 1400;
/// Default MSS (Maximum Segment Size) = MTU - Header
pub const MSS_DEFAULT: usize = MTU_DEFAULT - HEADER_SIZE;
/// Default send window size
pub const WND_SND_DEFAULT: u16 = 32;
/// Default receive window size
pub const WND_RCV_DEFAULT: u16 = 128;
/// Default RTO (Retransmission Timeout) in milliseconds
pub const RTO_DEFAULT: u32 = 200;
/// Minimum RTO in milliseconds
pub const RTO_MIN: u32 = 100;
/// Maximum RTO in milliseconds
pub const RTO_MAX: u32 = 60000;
/// RTO for nodelay mode
pub const RTO_NDL: u32 = 30;
/// Default update interval in milliseconds
pub const INTERVAL_DEFAULT: u32 = 100;
/// Dead link detection threshold (max retransmissions)
pub const DEAD_LINK: u32 = 20;
/// Initial slow-start threshold
pub const THRESH_INIT: u32 = 2;
/// Minimum slow-start threshold
pub const THRESH_MIN: u32 = 2;
/// Initial probe wait time in milliseconds
pub const PROBE_INIT: u32 = 7000;
/// Maximum probe wait time in milliseconds
pub const PROBE_LIMIT: u32 = 120000;

/// Send buffer capacity (MUST be power of 2)
#[cfg(feature = "small-buffers")]
pub const SND_BUF_CAPACITY: usize = 32;
/// Receive buffer capacity (MUST be power of 2)
#[cfg(feature = "small-buffers")]
pub const RCV_BUF_CAPACITY: usize = 32;

/// Send buffer capacity (MUST be power of 2)
#[cfg(not(feature = "small-buffers"))]
pub const SND_BUF_CAPACITY: usize = 256;
/// Receive buffer capacity (MUST be power of 2)
#[cfg(not(feature = "small-buffers"))]
pub const RCV_BUF_CAPACITY: usize = 256;

/// ACK list capacity (MUST be power of 2)
pub const ACK_LIST_CAPACITY: usize = 128;
/// Maximum fragments per message
pub const FRAGMENT_CAPACITY: usize = 128;

/// Compile-time assertion for power of 2
const _: () = assert!(SND_BUF_CAPACITY.is_power_of_two());
const _: () = assert!(RCV_BUF_CAPACITY.is_power_of_two());
const _: () = assert!(ACK_LIST_CAPACITY.is_power_of_two());
const _: () = assert!(FRAGMENT_CAPACITY.is_power_of_two());

/// Mask for send buffer indexing (capacity - 1)
pub const SND_BUF_MASK: usize = SND_BUF_CAPACITY - 1;
/// Mask for receive buffer indexing (capacity - 1)
pub const RCV_BUF_MASK: usize = RCV_BUF_CAPACITY - 1;
/// Mask for ACK list indexing (capacity - 1)
pub const ACK_LIST_MASK: usize = ACK_LIST_CAPACITY - 1;
/// Mask for fragment indexing (capacity - 1)
pub const FRAGMENT_MASK: usize = FRAGMENT_CAPACITY - 1;