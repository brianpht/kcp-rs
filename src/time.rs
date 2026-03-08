//! Time utilities with wrapping support

/// Time difference with wrapping
#[inline(always)]
pub const fn time_diff(later: u32, earlier: u32) -> i32 {
    later.wrapping_sub(earlier) as i32
}

/// Check if time has passed
#[inline(always)]
pub const fn time_after(a: u32, b: u32) -> bool {
    time_diff(a, b) > 0
}

/// Clamp value between min and max
#[inline(always)]
pub const fn clamp_u32(val: u32, min: u32, max: u32) -> u32 {
    if val < min {
        min
    } else if val > max {
        max
    } else {
        val
    }
}