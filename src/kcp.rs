//! KCP Protocol Implementation
//!
//! Design principles:
//! - Zero allocation in hot path
//! - Lock-free, single writer
//! - Deterministic latency
//! - All buffers preallocated

use crate::constants::*;
use crate::sequence::Sequence;
use crate::segment::{SegmentHeader, SegmentState, SendSegment};
use crate::ring_buffer::{SendBuffer, RecvBuffer, AckList};
use crate::codec::{encode_segment, decode_segment};
use crate::time::{time_diff, clamp_u32};

/// KCP Error types - marked cold as errors are not expected in normal operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum KcpError {
    /// Output buffer too small
    BufferTooSmall,
    /// Send/receive buffer full
    BufferFull,
    /// Invalid packet format
    InvalidPacket,
    /// Conversation ID mismatch
    ConvMismatch,
    /// Data exceeds maximum size
    DataTooLarge,
    /// No data available (non-blocking)
    WouldBlock,
    /// Connection considered dead
    DeadLink,
}

impl KcpError {
    /// Returns true if this error indicates the connection should be closed
    #[cold]
    #[inline(never)]
    pub const fn is_fatal(&self) -> bool {
        matches!(self, KcpError::DeadLink | KcpError::ConvMismatch)
    }
}

/// KCP Result type
pub type KcpResult<T> = Result<T, KcpError>;

/// Output callback trait for sending packets
/// 
/// Implementation MUST be allocation-free in hot path.
pub trait KcpOutput {
    /// Send data to the network
    /// 
    /// # Returns
    /// - `Ok(n)` - number of bytes sent
    /// - `Err(e)` - error occurred
    fn output(&mut self, data: &[u8]) -> KcpResult<usize>;
}

/// KCP Configuration
#[derive(Clone, Copy)]
#[repr(C)]
pub struct KcpConfig {
    /// Maximum transmission unit
    pub mtu: u32,
    /// Update interval in milliseconds
    pub interval: u32,
    /// Enable nodelay mode for lower latency
    pub nodelay: bool,
    /// Fast resend trigger count (0 = disable)
    pub resend: u32,
    /// Disable congestion control
    pub nc: bool,
    /// Send window size
    pub snd_wnd: u16,
    /// Receive window size
    pub rcv_wnd: u16,
    /// Enable stream mode (no message boundaries)
    pub stream: bool,
}

impl Default for KcpConfig {
    fn default() -> Self {
        Self {
            mtu: MTU_DEFAULT as u32,
            interval: INTERVAL_DEFAULT,
            nodelay: false,
            resend: 0,
            nc: false,
            snd_wnd: WND_SND_DEFAULT,
            rcv_wnd: WND_RCV_DEFAULT,
            stream: false,
        }
    }
}

impl KcpConfig {
    /// Fast mode configuration
    pub const fn fast() -> Self {
        Self {
            mtu: MTU_DEFAULT as u32,
            interval: 20,
            nodelay: true,
            resend: 2,
            nc: true,
            snd_wnd: 128,
            rcv_wnd: 128,
            stream: false,
        }
    }
}

/// KCP Control Block
/// All buffers preallocated, no heap allocation during operation
#[repr(C)]
pub struct Kcp<O: KcpOutput> {
    // === Hot fields (accessed every update) ===
    conv: u32,
    current: u32,

    // Sequence numbers
    snd_una: Sequence,   // oldest unacked
    snd_nxt: Sequence,   // next to send
    rcv_nxt: Sequence,   // next expected

    // Window
    snd_wnd: u16,
    rcv_wnd: u16,
    rmt_wnd: u16,
    cwnd: u16,

    // RTO
    rx_rto: u32,
    rx_srtt: i32,
    rx_rttval: i32,
    rx_minrto: u32,

    // State
    state: u32,
    updated: bool,

    // === Medium hot ===
    ts_flush: u32,
    interval: u32,

    ssthresh: u32,
    incr: u32,

    probe: u8,
    ts_probe: u32,
    probe_wait: u32,

    dead_link: u32,

    // Config
    mtu: u32,
    mss: u32,
    nodelay: bool,
    fastresend: u32,
    fastlimit: u32,
    nocwnd: bool,
    stream: bool,

    // === Buffers (preallocated) ===
    snd_buf: SendBuffer,
    rcv_buf: RecvBuffer,
    ack_list: AckList,

    // Send queue (pending fragmentation)
    snd_queue: SendBuffer,

    // Output buffer (preallocated)
    output_buf: [u8; MTU_DEFAULT + HEADER_SIZE],

    // Output callback
    output: O,
}

impl<O: KcpOutput> Kcp<O> {
    /// Create new KCP instance
    pub fn new(conv: u32, output: O) -> Self {
        Self::with_config(conv, output, KcpConfig::default())
    }

    /// Create with configuration
    pub fn with_config(conv: u32, output: O, config: KcpConfig) -> Self {
        let mss = config.mtu - HEADER_SIZE as u32;

        Self {
            conv,
            current: 0,

            snd_una: Sequence::new(0),
            snd_nxt: Sequence::new(0),
            rcv_nxt: Sequence::new(0),

            snd_wnd: config.snd_wnd,
            rcv_wnd: config.rcv_wnd,
            rmt_wnd: config.rcv_wnd,
            cwnd: 0,

            rx_rto: RTO_DEFAULT,
            rx_srtt: 0,
            rx_rttval: 0,
            rx_minrto: if config.nodelay { RTO_NDL } else { RTO_MIN },

            state: 0,
            updated: false,

            ts_flush: INTERVAL_DEFAULT,
            interval: config.interval,

            ssthresh: THRESH_INIT,
            incr: 0,

            probe: 0,
            ts_probe: 0,
            probe_wait: 0,

            dead_link: DEAD_LINK,

            mtu: config.mtu,
            mss,
            nodelay: config.nodelay,
            fastresend: config.resend,
            fastlimit: 5,
            nocwnd: config.nc,
            stream: config.stream,

            snd_buf: SendBuffer::new(),
            rcv_buf: RecvBuffer::new(),
            ack_list: AckList::new(),
            snd_queue: SendBuffer::new(),

            output_buf: [0u8; MTU_DEFAULT + HEADER_SIZE],

            output,
        }
    }

    /// Calculate unused receive window
    #[inline(always)]
    fn wnd_unused(&self) -> u16 {
        let queue_len = self.rcv_buf.rcv_nxt().diff(self.rcv_nxt) as u32;
        if queue_len < self.rcv_wnd as u32 {
            (self.rcv_wnd as u32 - queue_len) as u16
        } else {
            0
        }
    }

    /// Send data (may fragment into multiple segments)
    ///
    /// # Hot path: NO allocation
    #[inline]
    pub fn send(&mut self, data: &[u8]) -> KcpResult<usize> {
        if data.is_empty() {
            return Ok(0);
        }

        let mss = self.mss as usize;
        let count = if data.len() <= mss {
            1
        } else {
            data.len().div_ceil(mss)
        };

        if count > FRAGMENT_CAPACITY {
            return Err(KcpError::DataTooLarge);
        }

        // Get current snd_nxt from queue
        let base_sn = self.snd_queue.snd_nxt();

        let mut offset = 0;
        for i in 0..count {
            let size = (data.len() - offset).min(mss);
            let frg = if self.stream { 0 } else { (count - i - 1) as u8 };
            let sn = base_sn.add(i as u32);

            if !self.snd_queue.insert(sn, frg, &data[offset..offset + size]) {
                return Err(KcpError::BufferFull);
            }

            offset += size;
        }

        // Update snd_nxt in queue
        self.snd_queue.set_snd_nxt(base_sn.add(count as u32));

        Ok(data.len())
    }

    /// Receive data
    ///
    /// # Hot path: NO allocation
    #[inline]
    pub fn recv(&mut self, buf: &mut [u8]) -> KcpResult<usize> {
        let n = self.rcv_buf.read(buf);
        if n == 0 {
            return Err(KcpError::WouldBlock);
        }
        Ok(n)
    }

    /// Process incoming packet
    ///
    /// # Hot path: NO allocation
    #[inline]
    pub fn input(&mut self, data: &[u8]) -> KcpResult<()> {
        if data.len() < HEADER_SIZE {
            return Err(KcpError::InvalidPacket);
        }

        let mut offset = 0;
        let mut max_ack: Option<(Sequence, u32)> = None;

        while offset + HEADER_SIZE <= data.len() {
            let result = match decode_segment(&data[offset..]) {
                Some(r) => r,
                None => break,
            };

            let header = result.header;

            // Validate conv
            if header.conv != self.conv {
                return Err(KcpError::ConvMismatch);
            }

            // Update remote window
            self.rmt_wnd = header.wnd;

            // Process UNA (all segments before this are acked)
            self.parse_una(Sequence::new(header.una));

            // Shrink send buffer
            self.shrink_buf();

            match header.cmd {
                CMD_ACK => {
                    self.process_ack(&header, &mut max_ack);
                }
                CMD_PUSH => {
                    self.process_push(&header, &data[offset + HEADER_SIZE..offset + result.total_len]);
                }
                CMD_WASK => {
                    self.probe |= 0x02; // Tell window size
                }
                CMD_WINS => {
                    // Window size notification, already processed rmt_wnd
                }
                _ => {
                    return Err(KcpError::InvalidPacket);
                }
            }

            offset += result.total_len;
        }

        // Process fast ack
        if let Some((sn, _ts)) = max_ack {
            self.parse_fastack(sn);
        }

        // Update congestion window
        self.update_cwnd();

        Ok(())
    }

    /// Process ACK packet
    #[inline]
    fn process_ack(&mut self, header: &SegmentHeader, max_ack: &mut Option<(Sequence, u32)>) {
        let sn = Sequence::new(header.sn);

        // Update RTT
        if time_diff(self.current, header.ts) >= 0 {
            self.update_rtt(time_diff(self.current, header.ts) as u32);
        }

        // Mark segment as acked
        self.snd_buf.ack(sn);
        self.shrink_buf();

        // Track max ack for fastack
        match max_ack {
            Some((max_sn, _)) if sn.is_after(*max_sn) => {
                *max_ack = Some((sn, header.ts));
            }
            None => {
                *max_ack = Some((sn, header.ts));
            }
            _ => {}
        }
    }

    /// Process PUSH packet
    #[inline]
    fn process_push(&mut self, header: &SegmentHeader, data: &[u8]) {
        let sn = Sequence::new(header.sn);
        let wnd_end = self.rcv_nxt.add(self.rcv_wnd as u32);

        // Check if within window
        if !sn.is_in_range(self.rcv_nxt, wnd_end) {
            return;
        }

        // Queue ACK
        self.ack_list.push(header.sn, header.ts);

        // Insert into receive buffer
        if sn.diff(self.rcv_nxt) >= 0 {
            self.rcv_buf.insert(sn, header.frg, data);
        }
    }

    /// Update RTT estimation (RFC 6298)
    #[inline]
    fn update_rtt(&mut self, rtt: u32) {
        let rtt = rtt as i32;

        if self.rx_srtt == 0 {
            self.rx_srtt = rtt;
            self.rx_rttval = rtt / 2;
        } else {
            let delta = (rtt - self.rx_srtt).abs();
            self.rx_rttval = (3 * self.rx_rttval + delta) / 4;
            self.rx_srtt = (7 * self.rx_srtt + rtt) / 8;
            if self.rx_srtt < 1 {
                self.rx_srtt = 1;
            }
        }

        let rto = self.rx_srtt + (4 * self.rx_rttval).max(self.interval as i32);
        self.rx_rto = clamp_u32(rto as u32, self.rx_minrto, RTO_MAX);
    }

    /// Process UNA - remove all acked segments
    #[inline]
    fn parse_una(&mut self, una: Sequence) {
        self.snd_buf.shrink(una);
    }

    /// Shrink send buffer - update snd_una to oldest unacked
    #[inline]
    fn shrink_buf(&mut self) {
        // Update snd_una from send buffer's tracking
        self.snd_una = self.snd_buf.snd_una();

        // If buffer is empty, snd_una = snd_nxt
        if self.snd_buf.is_empty() {
            self.snd_una = self.snd_nxt;
        }
    }

    /// Process fast ack - increment fastack counter for segments before sn
    #[inline]
    fn parse_fastack(&mut self, sn: Sequence) {
        if sn.is_before(self.snd_una) || !sn.is_before(self.snd_nxt) {
            return;
        }

        // Increment fastack counter for all sent segments with sn < max_ack
        self.snd_buf.increment_fastack(sn, self.snd_una);
    }

    /// Update congestion window
    #[inline]
    fn update_cwnd(&mut self) {
        if self.nocwnd {
            return;
        }

        // Simplified cwnd update
        if self.cwnd < self.rmt_wnd {
            let mss = self.mss;
            if (self.cwnd as u32) < self.ssthresh {
                // Slow start
                self.cwnd = self.cwnd.saturating_add(1);
                self.incr = self.incr.wrapping_add(mss);
            } else {
                // Congestion avoidance
                if self.incr < mss {
                    self.incr = mss;
                }
                self.incr = self.incr.wrapping_add((mss * mss) / self.incr + mss / 16);
                if (self.cwnd as u32 + 1) * mss <= self.incr {
                    self.cwnd = self.cwnd.saturating_add(1);
                }
            }
            if self.cwnd > self.rmt_wnd {
                self.cwnd = self.rmt_wnd;
                self.incr = self.rmt_wnd as u32 * mss;
            }
        }
    }

    /// Main update function - call periodically
    ///
    /// # Hot path: minimal allocation
    #[inline]
    pub fn update(&mut self, current: u32) -> KcpResult<()> {
        self.current = current;

        if !self.updated {
            self.updated = true;
            self.ts_flush = current;
        }

        let mut slap = time_diff(current, self.ts_flush);

        // Handle clock drift
        if !(-10000..10000).contains(&slap) {
            self.ts_flush = current;
            slap = 0;
        }

        if slap >= 0 {
            self.ts_flush = self.ts_flush.wrapping_add(self.interval);
            if time_diff(current, self.ts_flush) >= 0 {
                self.ts_flush = current.wrapping_add(self.interval);
            }
            self.flush()?;
        }

        Ok(())
    }

    /// Check next update time
    #[inline]
    pub fn check(&self, current: u32) -> u32 {
        if !self.updated {
            return current;
        }

        let mut ts_flush = self.ts_flush;
        if time_diff(current, ts_flush) >= 10000 || time_diff(current, ts_flush) < -10000 {
            ts_flush = current;
        }

        if time_diff(current, ts_flush) >= 0 {
            return current;
        }

        let tm_flush = time_diff(ts_flush, current) as u32;

        // Check pending segments for earliest resend
        let mut tm_packet = u32::MAX;
        for (_, seg) in self.snd_buf.iter_pending() {
            let diff = time_diff(seg.resend_ts, current);
            if diff <= 0 {
                return current;
            }
            if (diff as u32) < tm_packet {
                tm_packet = diff as u32;
            }
        }

        current.wrapping_add(tm_packet.min(tm_flush).min(self.interval))
    }

    /// Flush pending data
    ///
    /// # Hot path: uses preallocated buffer
    #[inline]
    pub fn flush(&mut self) -> KcpResult<()> {
        if !self.updated {
            return Ok(());
        }

        let current = self.current;
        let wnd = self.wnd_unused();

        // Flush ACKs
        self.flush_acks(wnd)?;

        // Probe window if remote window is 0
        self.handle_probe(current)?;

        // Calculate send window
        let cwnd = self.calc_cwnd();

        // Move from queue to buffer
        self.move_to_snd_buf(cwnd)?;

        // Flush data segments
        self.flush_data(current, wnd)?;

        Ok(())
    }

    /// Flush pending ACKs
    #[inline]
    fn flush_acks(&mut self, wnd: u16) -> KcpResult<()> {
        while let Some(ack) = self.ack_list.pop() {
            let header = SegmentHeader {
                conv: self.conv,
                cmd: CMD_ACK,
                frg: 0,
                wnd,
                ts: ack.ts,
                sn: ack.sn,
                una: self.rcv_nxt.value(),
                len: 0,
            };

            if let Some(result) = encode_segment(&mut self.output_buf, &header, &[]) {
                self.output.output(&self.output_buf[..result.bytes_written])?;
            }
        }
        Ok(())
    }

    /// Handle window probe
    #[inline]
    fn handle_probe(&mut self, current: u32) -> KcpResult<()> {
        if self.rmt_wnd == 0 {
            if self.probe_wait == 0 {
                self.probe_wait = PROBE_INIT;
                self.ts_probe = current.wrapping_add(self.probe_wait);
            } else if time_diff(current, self.ts_probe) >= 0 {
                if self.probe_wait < PROBE_INIT {
                    self.probe_wait = PROBE_INIT;
                }
                self.probe_wait = self.probe_wait.wrapping_add(self.probe_wait / 2);
                if self.probe_wait > PROBE_LIMIT {
                    self.probe_wait = PROBE_LIMIT;
                }
                self.ts_probe = current.wrapping_add(self.probe_wait);
                self.probe |= 0x01; // Ask window
            }
        } else {
            self.ts_probe = 0;
            self.probe_wait = 0;
        }

        // Send probe packets
        let wnd = self.wnd_unused();

        if self.probe & 0x01 != 0 {
            self.send_cmd(CMD_WASK, wnd)?;
        }
        if self.probe & 0x02 != 0 {
            self.send_cmd(CMD_WINS, wnd)?;
        }

        self.probe = 0;
        Ok(())
    }

    /// Send command packet
    #[inline]
    fn send_cmd(&mut self, cmd: u8, wnd: u16) -> KcpResult<()> {
        let header = SegmentHeader {
            conv: self.conv,
            cmd,
            frg: 0,
            wnd,
            ts: 0,
            sn: 0,
            una: self.rcv_nxt.value(),
            len: 0,
        };

        if let Some(result) = encode_segment(&mut self.output_buf, &header, &[]) {
            self.output.output(&self.output_buf[..result.bytes_written])?;
        }
        Ok(())
    }

    /// Calculate effective congestion window
    #[inline(always)]
    fn calc_cwnd(&self) -> u32 {
        let mut cwnd = (self.snd_wnd as u32).min(self.rmt_wnd as u32);
        if !self.nocwnd {
            cwnd = cwnd.min(self.cwnd as u32);
        }
        cwnd
    }

    /// Move segments from queue to send buffer respecting cwnd
    #[inline]
    fn move_to_snd_buf(&mut self, cwnd: u32) -> KcpResult<()> {
        // Calculate how many new segments we can send
        // in-flight = snd_nxt - snd_una
        let in_flight = self.snd_nxt.diff(self.snd_una) as u32;

        // Available window
        let available = cwnd.saturating_sub(in_flight);

        // Move segments from queue to send buffer
        let queue_base = self.snd_queue.snd_una();
        let queue_nxt = self.snd_queue.snd_nxt();
        let queue_count = queue_nxt.diff(queue_base) as u32;

        let to_move = available.min(queue_count);

        for i in 0..to_move {
            let queue_sn = queue_base.add(i);
            let idx = queue_sn.to_index(SND_BUF_MASK);

            // Get segment data from queue
            let queue_seg = self.snd_queue.get_by_index(idx);
            if queue_seg.state != SegmentState::Pending {
                continue;
            }

            let frg = queue_seg.frg;
            let data_len = queue_seg.data_len as usize;
            let data_offset = queue_seg.data_offset as usize;

            // Copy data to send buffer
            let sn = self.snd_nxt;
            let snd_idx = sn.to_index(SND_BUF_MASK);
            let snd_data_offset = snd_idx * MSS_DEFAULT;

            // Copy data from queue to snd_buf
            // Note: Using separate indexing to avoid borrow issues
            for j in 0..data_len {
                self.snd_buf.data[snd_data_offset + j] = self.snd_queue.data[data_offset + j];
            }

            // Create segment in send buffer
            self.snd_buf.segments[snd_idx] = SendSegment {
                sn,
                resend_ts: 0,
                rto: self.rx_rto,
                fastack: 0,
                xmit: 0,
                state: SegmentState::Pending,
                frg,
                data_len: data_len as u16,
                ts: 0,
                data_offset: snd_data_offset as u32,
            };

            // Clear queue segment
            self.snd_queue.segments[idx].state = SegmentState::Empty;

            // Advance snd_nxt
            self.snd_nxt = self.snd_nxt.increment();
            self.snd_buf.set_snd_nxt(self.snd_nxt);
        }

        // Update queue's snd_una
        self.snd_queue.set_snd_una(queue_base.add(to_move));

        Ok(())
    }

    /// Flush data segments
    #[inline]
    fn flush_data(&mut self, current: u32, wnd: u16) -> KcpResult<()> {
        let resent = if self.fastresend > 0 { self.fastresend } else { u32::MAX };
        let rtomin = if self.nodelay { 0 } else { self.rx_rto >> 3 };

        // Collect pending indices first to avoid borrow conflicts
        let pending = self.snd_buf.pending_indices();

        for maybe_idx in pending.iter() {
            let idx = match maybe_idx {
                Some(i) => *i,
                None => break,
            };

            let seg = self.snd_buf.get_by_index(idx);
            let mut needsend = false;

            // Determine if we need to send and compute new values
            let (new_xmit, new_rto, new_resend_ts, new_fastack) = if seg.xmit == 0 {
                // First transmission
                needsend = true;
                (1u16, self.rx_rto, current.wrapping_add(self.rx_rto + rtomin), seg.fastack)
            } else if time_diff(current, seg.resend_ts) >= 0 {
                // Timeout retransmission
                needsend = true;
                let new_rto = if self.nodelay {
                    seg.rto.wrapping_add(seg.rto / 2)
                } else {
                    seg.rto.wrapping_add(self.rx_rto)
                };
                (seg.xmit.saturating_add(1), new_rto, current.wrapping_add(new_rto), seg.fastack)
            } else if seg.fastack >= resent as u16 {
                // Fast retransmission
                if seg.xmit <= self.fastlimit as u16 || self.fastlimit == 0 {
                    needsend = true;
                    (seg.xmit.saturating_add(1), seg.rto, current.wrapping_add(seg.rto), 0u16)
                } else {
                    (seg.xmit, seg.rto, seg.resend_ts, seg.fastack)
                }
            } else {
                (seg.xmit, seg.rto, seg.resend_ts, seg.fastack)
            };

            // Capture segment info before mutation
            let sn = seg.sn;
            let frg = seg.frg;
            let data_len = seg.data_len;

            if needsend {
                // Update segment state
                let seg_mut = self.snd_buf.get_mut_by_index(idx);
                seg_mut.xmit = new_xmit;
                seg_mut.rto = new_rto;
                seg_mut.resend_ts = new_resend_ts;
                seg_mut.fastack = new_fastack;
                seg_mut.ts = current;
                seg_mut.state = SegmentState::Sent;

                let header = SegmentHeader {
                    conv: self.conv,
                    cmd: CMD_PUSH,
                    frg,
                    wnd,
                    ts: current,
                    sn: sn.value(),
                    una: self.rcv_nxt.value(),
                    len: data_len as u32,
                };

                let data = self.snd_buf.get_data_by_index(idx);
                if let Some(result) = encode_segment(&mut self.output_buf, &header, data) {
                    self.output.output(&self.output_buf[..result.bytes_written])?;
                }

                // Check dead link
                if new_xmit as u32 >= self.dead_link {
                    self.state = u32::MAX;
                    return Err(KcpError::DeadLink);
                }
            }
        }

        Ok(())
    }

    /// Get number of packets waiting to be sent
    #[inline(always)]
    pub fn wait_snd(&self) -> u32 {
        self.snd_buf.len() + self.snd_queue.len()
    }

    /// Check if connection is dead
    #[inline(always)]
    pub const fn is_dead(&self) -> bool {
        self.state == u32::MAX
    }

    /// Get current RTT estimate in milliseconds
    #[inline(always)]
    pub const fn rtt(&self) -> u32 {
        self.rx_srtt as u32
    }

    /// Get current RTO in milliseconds
    #[inline(always)]
    pub const fn rto(&self) -> u32 {
        self.rx_rto
    }
}