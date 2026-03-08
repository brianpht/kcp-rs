//! Galois Field GF(2^8) arithmetic
//!
//! Uses lookup tables for O(1) multiplication and division.
//! Primitive polynomial: x^8 + x^4 + x^3 + x^2 + 1 (0x11D)
//!
//! All tables are computed at compile time - zero runtime allocation.

/// Primitive polynomial for GF(2^8): x^8 + x^4 + x^3 + x^2 + 1
const PRIMITIVE_POLY: u16 = 0x11D;

/// GF(2^8) element
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct GF256(pub u8);

impl GF256 {
    /// Zero element
    pub const ZERO: Self = Self(0);

    /// One element (multiplicative identity)
    pub const ONE: Self = Self(1);

    /// Create new GF(256) element
    #[inline(always)]
    pub const fn new(val: u8) -> Self {
        Self(val)
    }

    /// Get raw value
    #[inline(always)]
    pub const fn value(self) -> u8 {
        self.0
    }

    /// Addition in GF(2^8) is XOR
    #[inline(always)]
    pub const fn add(self, other: Self) -> Self {
        Self(self.0 ^ other.0)
    }

    /// Subtraction in GF(2^8) is also XOR (same as addition)
    #[inline(always)]
    pub const fn sub(self, other: Self) -> Self {
        Self(self.0 ^ other.0)
    }

    /// Multiplication using lookup tables
    /// Optimized to avoid modulo using extended EXP_TABLE
    #[inline(always)]
    pub fn mul(self, other: Self) -> Self {
        if self.0 == 0 || other.0 == 0 {
            return Self::ZERO;
        }
        let log_a = LOG_TABLE[self.0 as usize] as usize;
        let log_b = LOG_TABLE[other.0 as usize] as usize;
        // Use extended EXP_TABLE (512 entries) to avoid modulo
        Self(EXP_TABLE[log_a + log_b])
    }

    /// Division using lookup tables
    /// Returns None if dividing by zero
    #[inline(always)]
    pub fn div(self, other: Self) -> Option<Self> {
        if other.0 == 0 {
            return None;
        }
        if self.0 == 0 {
            return Some(Self::ZERO);
        }
        let log_a = LOG_TABLE[self.0 as usize] as i16;
        let log_b = LOG_TABLE[other.0 as usize] as i16;
        let mut log_result = log_a - log_b;
        if log_result < 0 {
            log_result += 255;
        }
        Some(Self(EXP_TABLE[log_result as usize]))
    }

    /// Multiplicative inverse
    #[inline(always)]
    pub fn inverse(self) -> Option<Self> {
        Self::ONE.div(self)
    }

    /// Power function
    #[inline]
    pub fn pow(self, exp: u8) -> Self {
        if exp == 0 {
            return Self::ONE;
        }
        if self.0 == 0 {
            return Self::ZERO;
        }
        let log_a = LOG_TABLE[self.0 as usize] as u16;
        let log_result = (log_a * exp as u16) % 255;
        Self(EXP_TABLE[log_result as usize])
    }
}

/// Exponential table: EXP_TABLE[i] = 2^i mod P
/// Precomputed at compile time
static EXP_TABLE: [u8; 512] = {
    let mut table = [0u8; 512];
    let mut val: u16 = 1;
    let mut i = 0;
    while i < 255 {
        table[i] = val as u8;
        table[i + 255] = val as u8; // Duplicate for wraparound
        val <<= 1;
        if val & 0x100 != 0 {
            val ^= PRIMITIVE_POLY;
        }
        i += 1;
    }
    // EXP_TABLE[255] and beyond are for wraparound
    table[510] = table[0];
    table[511] = table[1];
    table
};

/// Logarithm table: LOG_TABLE[2^i] = i
/// Precomputed at compile time
static LOG_TABLE: [u8; 256] = {
    let mut table = [0u8; 256];
    let mut i = 0;
    while i < 255 {
        table[EXP_TABLE[i] as usize] = i as u8;
        i += 1;
    }
    table[0] = 0; // log(0) is undefined, but we handle it specially
    table
};

/// Inverse table: INV_TABLE[x] = x^(-1)
/// Precomputed at compile time
#[allow(dead_code)]
static INV_TABLE: [u8; 256] = {
    let mut table = [0u8; 256];
    table[0] = 0; // 0 has no inverse
    let mut i = 1;
    while i < 256 {
        // inv(x) = x^254 in GF(2^8)
        let log_x = LOG_TABLE[i];
        let log_inv = if log_x == 0 { 0 } else { 255 - log_x };
        table[i] = EXP_TABLE[log_inv as usize];
        i += 1;
    }
    table
};

/// Multiply vector by scalar (in-place)
/// Optimized to avoid modulo using extended EXP_TABLE
#[allow(dead_code)]
#[inline]
pub fn mul_slice(data: &mut [u8], scalar: GF256) {
    if scalar.0 == 0 {
        data.fill(0);
        return;
    }
    if scalar.0 == 1 {
        return;
    }
    let log_scalar = LOG_TABLE[scalar.0 as usize] as usize;
    for byte in data.iter_mut() {
        if *byte != 0 {
            let log_byte = LOG_TABLE[*byte as usize] as usize;
            // Use extended EXP_TABLE to avoid modulo
            *byte = EXP_TABLE[log_byte + log_scalar];
        }
    }
}

/// Add (XOR) src into dst: dst = dst + src
#[inline]
pub fn add_slice(dst: &mut [u8], src: &[u8]) {
    debug_assert_eq!(dst.len(), src.len());
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        *d ^= *s;
    }
}

/// Multiply-add: dst = dst + (src * scalar)
/// Hot path - optimized to avoid modulo
#[inline]
pub fn mul_add_slice(dst: &mut [u8], src: &[u8], scalar: GF256) {
    debug_assert_eq!(dst.len(), src.len());
    if scalar.0 == 0 {
        return;
    }
    if scalar.0 == 1 {
        add_slice(dst, src);
        return;
    }
    let log_scalar = LOG_TABLE[scalar.0 as usize] as usize;
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        if *s != 0 {
            let log_s = LOG_TABLE[*s as usize] as usize;
            // Use extended EXP_TABLE (512 entries) to avoid modulo
            // log_s + log_scalar < 255 + 255 = 510, which fits in 512-entry table
            *d ^= EXP_TABLE[log_s + log_scalar];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gf_add() {
        let a = GF256::new(0x53);
        let b = GF256::new(0xCA);
        let c = a.add(b);
        // Addition is XOR
        assert_eq!(c.value(), 0x53 ^ 0xCA);
        // a + a = 0
        assert_eq!(a.add(a), GF256::ZERO);
    }

    #[test]
    fn test_gf_mul() {
        // 1 * x = x
        assert_eq!(GF256::ONE.mul(GF256::new(0x53)), GF256::new(0x53));
        // 0 * x = 0
        assert_eq!(GF256::ZERO.mul(GF256::new(0x53)), GF256::ZERO);
        // x * 0 = 0
        assert_eq!(GF256::new(0x53).mul(GF256::ZERO), GF256::ZERO);

        // Known multiplication result
        // 0x53 * 0xCA in GF(2^8) with primitive polynomial 0x11D
        let result = GF256::new(0x53).mul(GF256::new(0xCA));
        // Verify by checking inverse
        let back = result.div(GF256::new(0xCA)).unwrap();
        assert_eq!(back, GF256::new(0x53));
    }

    #[test]
    fn test_gf_div() {
        let a = GF256::new(0x53);
        let b = GF256::new(0xCA);

        // Division by zero
        assert!(a.div(GF256::ZERO).is_none());

        // 0 / x = 0
        assert_eq!(GF256::ZERO.div(b), Some(GF256::ZERO));

        // x / 1 = x
        assert_eq!(a.div(GF256::ONE), Some(a));

        // x / x = 1
        assert_eq!(a.div(a), Some(GF256::ONE));

        // (a * b) / b = a
        let product = a.mul(b);
        assert_eq!(product.div(b), Some(a));
    }

    #[test]
    fn test_gf_inverse() {
        for i in 1..=255u8 {
            let x = GF256::new(i);
            let inv = x.inverse().unwrap();
            // x * x^(-1) = 1
            assert_eq!(x.mul(inv), GF256::ONE);
        }
    }

    #[test]
    fn test_gf_pow() {
        let x = GF256::new(2);

        // x^0 = 1
        assert_eq!(x.pow(0), GF256::ONE);

        // x^1 = x
        assert_eq!(x.pow(1), x);

        // 2^8 in GF(2^8) wraps with primitive polynomial
        let result = x.pow(8);
        // 2^8 mod (x^8 + x^4 + x^3 + x^2 + 1) = x^4 + x^3 + x^2 + 1 = 0x1D
        assert_eq!(result.value(), 0x1D);
    }

    #[test]
    fn test_slice_operations() {
        let mut data = [0x53, 0xCA, 0x00, 0xFF];
        let original = data;

        // mul by 1 doesn't change
        mul_slice(&mut data, GF256::ONE);
        assert_eq!(data, original);

        // mul by 0 zeros everything
        mul_slice(&mut data, GF256::ZERO);
        assert_eq!(data, [0, 0, 0, 0]);

        // Test add_slice
        let mut dst = [0x00, 0xFF, 0x53, 0xCA];
        let src = [0xFF, 0xFF, 0x53, 0x00];
        add_slice(&mut dst, &src);
        assert_eq!(dst, [0xFF, 0x00, 0x00, 0xCA]);
    }

    #[test]
    fn test_exp_log_tables() {
        // Verify tables are consistent
        for i in 1..255 {
            let exp_val = EXP_TABLE[i];
            let log_val = LOG_TABLE[exp_val as usize];
            assert_eq!(log_val as usize, i);
        }
    }
}

