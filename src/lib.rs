//! KCP Protocol - High Performance Rust Implementation
//!
//! Design principles:
//! - Zero allocation in hot path
//! - Lock-free, single writer
//! - Deterministic latency
//! - All buffers preallocated
//!
//! # Example
//! ```ignore
//! use kcp_rs::{Kcp, KcpConfig, KcpOutput, KcpResult};
//!
//! struct UdpOutput { /* ... */ }
//! impl KcpOutput for UdpOutput {
//!     fn output(&mut self, data: &[u8]) -> KcpResult<usize> {
//!         // Send via UDP
//!         Ok(data.len())
//!     }
//! }
//!
//! let mut kcp = Kcp::with_config(1, UdpOutput { }, KcpConfig::fast());
//! kcp.send(b"Hello")?;
//! kcp.update(current_ms)?;
//! ```

#![no_std]
#![forbid(unsafe_code)] // Remove if unsafe is needed with justification
#![warn(missing_docs)]

pub mod constants;
pub mod sequence;
pub mod segment;
pub mod ring_buffer;
pub mod codec;
pub mod time;
pub mod kcp;

pub use constants::*;
pub use sequence::Sequence;
pub use segment::{SegmentHeader, SegmentState};
pub use kcp::{Kcp, KcpConfig, KcpError, KcpOutput, KcpResult};