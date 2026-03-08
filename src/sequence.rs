//! Sequence number arithmetic with wrapping support
//!
//! Uses half-range comparison for proper sequence ordering.
//! NEVER use direct comparison (>, <) on sequence numbers.

/// Sequence number type alias
pub type SeqNum = u32;

/// Half range for sequence comparison (2^31)
const HALF_RANGE: u32 = 1 << 31;

/// Sequence number with wrapping arithmetic support.
///
/// Uses half-range comparison for proper ordering across wraparound.
/// All operations use `wrapping_*` methods to handle overflow correctly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct Sequence(pub SeqNum);

impl Sequence {
    /// Create a new sequence number
    #[inline(always)]
    pub const fn new(val: SeqNum) -> Self {
        Self(val)
    }

    /// Get the raw value
    #[inline(always)]
    pub const fn value(self) -> SeqNum {
        self.0
    }

    /// Increment sequence number (wrapping)
    #[inline(always)]
    pub const fn increment(self) -> Self {
        Self(self.0.wrapping_add(1))
    }

    /// Add offset to sequence number (wrapping)
    #[inline(always)]
    pub const fn add(self, offset: u32) -> Self {
        Self(self.0.wrapping_add(offset))
    }

    /// Compute difference between sequences (wrapping)
    /// Returns signed difference for proper ordering
    #[inline(always)]
    pub const fn diff(self, other: Self) -> i32 {
        self.0.wrapping_sub(other.0) as i32
    }

    /// Check if self is after other (using half-range comparison)
    /// Returns true if self > other in sequence space
    #[inline(always)]
    pub const fn is_after(self, other: Self) -> bool {
        let diff = self.0.wrapping_sub(other.0);
        diff > 0 && diff < HALF_RANGE
    }

    /// Check if self is before other
    #[inline(always)]
    pub const fn is_before(self, other: Self) -> bool {
        other.is_after(self)
    }

    /// Check if self is within range [start, end)
    #[inline(always)]
    pub const fn is_in_range(self, start: Self, end: Self) -> bool {
        let diff_start = self.0.wrapping_sub(start.0);
        let diff_end = end.0.wrapping_sub(start.0);
        diff_start < diff_end
    }

    /// Convert to ring buffer index using bitwise AND (NOT modulo)
    #[inline(always)]
    pub const fn to_index(self, mask: usize) -> usize {
        (self.0 as usize) & mask
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrapping() {
        let max = Sequence::new(u32::MAX);
        let next = max.increment();
        assert_eq!(next.value(), 0);
    }

    #[test]
    fn test_ordering() {
        let a = Sequence::new(100);
        let b = Sequence::new(50);
        assert!(a.is_after(b));
        assert!(b.is_before(a));
    }

    #[test]
    fn test_wrapping_ordering() {
        let a = Sequence::new(10);
        let b = Sequence::new(u32::MAX - 10);
        // a should be "after" b when b wraps around
        assert!(a.is_after(b));
    }

    #[test]
    fn test_index() {
        let seq = Sequence::new(257);
        let mask = 255usize; // capacity 256
        assert_eq!(seq.to_index(mask), 1);
    }

    #[test]
    fn test_diff() {
        let a = Sequence::new(100);
        let b = Sequence::new(90);
        assert_eq!(a.diff(b), 10);
        assert_eq!(b.diff(a), -10);
    }

    #[test]
    fn test_diff_wrapping() {
        let a = Sequence::new(5);
        let b = Sequence::new(u32::MAX - 5);
        // Wrapping diff: 5 - (MAX - 5) = 5 + 6 = 11
        assert_eq!(a.diff(b), 11);
    }

    #[test]
    fn test_add() {
        let seq = Sequence::new(100);
        assert_eq!(seq.add(50).value(), 150);

        let max = Sequence::new(u32::MAX);
        assert_eq!(max.add(10).value(), 9);
    }

    #[test]
    fn test_is_in_range() {
        let start = Sequence::new(10);
        let end = Sequence::new(20);

        assert!(Sequence::new(10).is_in_range(start, end));
        assert!(Sequence::new(15).is_in_range(start, end));
        assert!(Sequence::new(19).is_in_range(start, end));
        assert!(!Sequence::new(20).is_in_range(start, end));
        assert!(!Sequence::new(9).is_in_range(start, end));
        assert!(!Sequence::new(25).is_in_range(start, end));
    }

    #[test]
    fn test_is_in_range_wrapping() {
        // Range that wraps around
        let start = Sequence::new(u32::MAX - 5);
        let end = Sequence::new(10);

        assert!(Sequence::new(u32::MAX - 5).is_in_range(start, end));
        assert!(Sequence::new(u32::MAX).is_in_range(start, end));
        assert!(Sequence::new(0).is_in_range(start, end));
        assert!(Sequence::new(5).is_in_range(start, end));
        assert!(!Sequence::new(10).is_in_range(start, end));
        assert!(!Sequence::new(100).is_in_range(start, end));
    }

    #[test]
    fn test_equal_sequences() {
        let a = Sequence::new(100);
        let b = Sequence::new(100);
        assert!(!a.is_after(b));
        assert!(!a.is_before(b));
        assert_eq!(a.diff(b), 0);
    }

    #[test]
    fn test_half_range_boundary() {
        // Test at exactly half range
        let a = Sequence::new(0);
        let b = Sequence::new(1 << 31);
        // b is exactly half range away, should NOT be considered after
        assert!(!b.is_after(a));
    }
}