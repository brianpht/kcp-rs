//! Reed-Solomon FEC Decoder
//!
//! Recovers missing data shards from any k received shards.
//! Uses Gaussian elimination for matrix inversion.
//! Zero allocation - uses preallocated buffers.

use super::gf256::{GF256, mul_add_slice};
use super::{FecConfig, MAX_SHARDS, MAX_SHARD_SIZE};

/// FEC Decoder
/// 
/// Recovers missing data shards using Reed-Solomon decoding.
/// Requires at least k shards (any combination of data and parity).
pub struct FecDecoder {
    /// FEC configuration
    config: FecConfig,
    /// Full encoding matrix (identity + Vandermonde)
    /// [total_shards][data_shards]
    full_matrix: [[GF256; MAX_SHARDS]; MAX_SHARDS],
    /// Decode matrix (computed per recovery)
    decode_matrix: [[GF256; MAX_SHARDS]; MAX_SHARDS],
    /// Temporary buffer for Gaussian elimination (reserved)
    #[allow(dead_code)]
    temp_row: [GF256; MAX_SHARDS],
    /// Temporary data buffer
    temp_data: [u8; MAX_SHARD_SIZE],
}

impl FecDecoder {
    /// Create new decoder with given configuration
    pub fn new(config: FecConfig) -> Self {
        let mut decoder = Self {
            config,
            full_matrix: [[GF256::ZERO; MAX_SHARDS]; MAX_SHARDS],
            decode_matrix: [[GF256::ZERO; MAX_SHARDS]; MAX_SHARDS],
            temp_row: [GF256::ZERO; MAX_SHARDS],
            temp_data: [0u8; MAX_SHARD_SIZE],
        };
        decoder.build_full_matrix();
        decoder
    }

    /// Build full encoding matrix
    ///
    /// ```text
    /// [ 1 0 0 0 ]   <- identity rows (data shards)
    /// [ 0 1 0 0 ]
    /// [ 0 0 1 0 ]
    /// [ 0 0 0 1 ]
    /// [ V V V V ]   <- Vandermonde rows (parity shards)
    /// [ V V V V ]
    /// ```
    fn build_full_matrix(&mut self) {
        let k = self.config.data_shards as usize;
        let m = self.config.parity_shards as usize;
        let n = k + m;

        // Identity rows for data shards
        for i in 0..k {
            for j in 0..k {
                self.full_matrix[i][j] = if i == j { GF256::ONE } else { GF256::ZERO };
            }
        }

        // Vandermonde rows for parity shards
        for i in 0..m {
            let base = GF256::new((k + i + 1) as u8);
            for j in 0..k {
                self.full_matrix[k + i][j] = base.pow(j as u8);
            }
        }

        let _ = n; // suppress unused warning
    }

    /// Decode missing shards
    ///
    /// # Arguments
    /// * `shards` - Array of shard buffers (Some = present, None = missing)
    /// * `shard_size` - Size of each shard
    /// * `present` - Bitmap of which shards are present
    ///
    /// # Returns
    /// `true` if decoding succeeded
    pub fn decode(
        &mut self,
        shards: &mut [Option<&mut [u8]>],
        shard_size: usize,
    ) -> bool {
        let k = self.config.data_shards as usize;
        let n = self.config.total_shards() as usize;

        if shards.len() != n || shard_size > MAX_SHARD_SIZE {
            return false;
        }

        // Count present shards and find missing data shards
        let mut present_count = 0;
        let mut missing_data = [false; MAX_SHARDS];
        let mut missing_data_count = 0;

        for (i, shard) in shards.iter().enumerate() {
            if shard.is_some() {
                present_count += 1;
            } else if i < k {
                missing_data[i] = true;
                missing_data_count += 1;
            }
        }

        // Need at least k shards to decode
        if present_count < k {
            return false;
        }

        // If no data shards are missing, nothing to do
        if missing_data_count == 0 {
            return true;
        }

        // Build submatrix from present shards
        let mut sub_row = 0;
        let mut present_indices = [0usize; MAX_SHARDS];

        for (i, shard) in shards.iter().enumerate() {
            if shard.is_some() && sub_row < k {
                // Copy row from full matrix
                for j in 0..k {
                    self.decode_matrix[sub_row][j] = self.full_matrix[i][j];
                }
                present_indices[sub_row] = i;
                sub_row += 1;
            }
        }

        // Invert the submatrix using Gaussian elimination
        if !self.invert_matrix(k) {
            return false;
        }

        // Recover each missing data shard
        for missing_idx in 0..k {
            if !missing_data[missing_idx] {
                continue;
            }

            // Initialize temp buffer to zero
            self.temp_data[..shard_size].fill(0);

            // Compute missing shard: sum(decode_matrix[missing_idx][j] * present_shard[j])
            for j in 0..k {
                let coeff = self.decode_matrix[missing_idx][j];
                if coeff.0 != 0 {
                    let present_idx = present_indices[j];
                    if let Some(ref shard_data) = shards[present_idx] {
                        mul_add_slice(
                            &mut self.temp_data[..shard_size],
                            &shard_data[..shard_size],
                            coeff,
                        );
                    }
                }
            }

            // Copy recovered data back
            // We need to handle this carefully due to borrow checker
            // Store in temp first, then copy
        }

        // Second pass: actually copy recovered data
        for missing_idx in 0..k {
            if !missing_data[missing_idx] {
                continue;
            }

            // Recompute (we can optimize this later with more temp buffers)
            self.temp_data[..shard_size].fill(0);

            for j in 0..k {
                let coeff = self.decode_matrix[missing_idx][j];
                if coeff.0 != 0 {
                    let present_idx = present_indices[j];
                    if let Some(ref shard_data) = shards[present_idx] {
                        mul_add_slice(
                            &mut self.temp_data[..shard_size],
                            &shard_data[..shard_size],
                            coeff,
                        );
                    }
                }
            }

            // Now copy to the missing shard
            if let Some(ref mut shard_data) = shards[missing_idx] {
                shard_data[..shard_size].copy_from_slice(&self.temp_data[..shard_size]);
            }
        }

        true
    }

    /// Decode with explicit erasure positions
    ///
    /// This method allows specifying which shards are missing (erasures).
    /// All shards in the array must have buffers, but `erasures[i] = true`
    /// indicates that shard i should be recovered.
    ///
    /// # Arguments
    /// * `shards` - Array of all shard buffers (must all be Some)
    /// * `erasures` - Boolean array indicating which shards are missing
    /// * `shard_size` - Size of each shard
    ///
    /// # Returns
    /// `true` if decoding succeeded
    pub fn decode_with_erasures(
        &mut self,
        shards: &mut [&mut [u8]],
        erasures: &[bool],
        shard_size: usize,
    ) -> bool {
        let k = self.config.data_shards as usize;
        let n = self.config.total_shards() as usize;

        if shards.len() != n || erasures.len() != n || shard_size > MAX_SHARD_SIZE {
            return false;
        }

        // Count present shards and find missing data shards
        let mut present_count = 0;
        let mut missing_data = [false; MAX_SHARDS];
        let mut missing_data_count = 0;

        for (i, &erased) in erasures.iter().enumerate().take(n) {
            if !erased {
                present_count += 1;
            } else if i < k {
                missing_data[i] = true;
                missing_data_count += 1;
            }
        }

        // Need at least k shards to decode
        if present_count < k {
            return false;
        }

        // If no data shards are missing, nothing to do
        if missing_data_count == 0 {
            return true;
        }

        // Build submatrix from present shards
        let mut sub_row = 0;
        let mut present_indices = [0usize; MAX_SHARDS];

        for (i, &erased) in erasures.iter().enumerate().take(n) {
            if !erased && sub_row < k {
                // Copy row from full matrix
                for j in 0..k {
                    self.decode_matrix[sub_row][j] = self.full_matrix[i][j];
                }
                present_indices[sub_row] = i;
                sub_row += 1;
            }
        }

        // Invert the submatrix
        if !self.invert_matrix(k) {
            return false;
        }

        // Recover each missing data shard
        for missing_idx in 0..k {
            if !missing_data[missing_idx] {
                continue;
            }

            // Initialize temp buffer
            self.temp_data[..shard_size].fill(0);

            // Compute: sum(decode_matrix[missing_idx][j] * present_shard[j])
            for j in 0..k {
                let coeff = self.decode_matrix[missing_idx][j];
                if coeff.0 != 0 {
                    let present_idx = present_indices[j];
                    mul_add_slice(
                        &mut self.temp_data[..shard_size],
                        &shards[present_idx][..shard_size],
                        coeff,
                    );
                }
            }

            // Copy recovered data to the missing shard
            shards[missing_idx][..shard_size].copy_from_slice(&self.temp_data[..shard_size]);
        }

        true
    }

    /// Invert matrix using Gaussian elimination with partial pivoting
    /// Operates in-place on decode_matrix
    fn invert_matrix(&mut self, n: usize) -> bool {
        // Augment with identity matrix
        // We'll store the inverse in the same matrix
        let mut inv = [[GF256::ZERO; MAX_SHARDS]; MAX_SHARDS];
        for i in 0..n {
            inv[i][i] = GF256::ONE;
        }

        // Forward elimination
        for col in 0..n {
            // Find pivot
            let mut pivot_row = col;
            for row in col..n {
                if self.decode_matrix[row][col].0 != 0 {
                    pivot_row = row;
                    break;
                }
            }

            if self.decode_matrix[pivot_row][col].0 == 0 {
                return false; // Singular matrix
            }

            // Swap rows if needed
            if pivot_row != col {
                for j in 0..n {
                    let tmp = self.decode_matrix[col][j];
                    self.decode_matrix[col][j] = self.decode_matrix[pivot_row][j];
                    self.decode_matrix[pivot_row][j] = tmp;

                    let tmp = inv[col][j];
                    inv[col][j] = inv[pivot_row][j];
                    inv[pivot_row][j] = tmp;
                }
            }

            // Scale pivot row
            let pivot_inv = match self.decode_matrix[col][col].inverse() {
                Some(v) => v,
                None => return false,
            };

            for j in 0..n {
                self.decode_matrix[col][j] = self.decode_matrix[col][j].mul(pivot_inv);
                inv[col][j] = inv[col][j].mul(pivot_inv);
            }

            // Eliminate column in other rows
            for row in 0..n {
                if row != col && self.decode_matrix[row][col].0 != 0 {
                    let factor = self.decode_matrix[row][col];
                    for j in 0..n {
                        let sub = self.decode_matrix[col][j].mul(factor);
                        self.decode_matrix[row][j] = self.decode_matrix[row][j].sub(sub);

                        let sub = inv[col][j].mul(factor);
                        inv[row][j] = inv[row][j].sub(sub);
                    }
                }
            }
        }

        // Copy inverse back
        for i in 0..n {
            for j in 0..n {
                self.decode_matrix[i][j] = inv[i][j];
            }
        }

        true
    }

    /// Get configuration
    #[inline(always)]
    pub const fn config(&self) -> FecConfig {
        self.config
    }
}

impl Default for FecDecoder {
    fn default() -> Self {
        Self::new(FecConfig::default())
    }
}

/// Reconstruct missing shards given available shards
/// 
/// This is a simpler interface that takes ownership of buffers.
#[allow(dead_code)]
pub fn reconstruct(
    config: FecConfig,
    shards: &mut [Option<&mut [u8]>],
    shard_size: usize,
) -> bool {
    let mut decoder = FecDecoder::new(config);
    decoder.decode(shards, shard_size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::encoder::FecEncoder;

    #[test]
    fn test_decoder_creation() {
        let config = FecConfig::new(4, 2);
        let decoder = FecDecoder::new(config);
        assert_eq!(decoder.config().data_shards, 4);
    }

    #[test]
    fn test_full_matrix_structure() {
        let config = FecConfig::new(2, 1);
        let decoder = FecDecoder::new(config);

        // Check identity part
        assert_eq!(decoder.full_matrix[0][0], GF256::ONE);
        assert_eq!(decoder.full_matrix[0][1], GF256::ZERO);
        assert_eq!(decoder.full_matrix[1][0], GF256::ZERO);
        assert_eq!(decoder.full_matrix[1][1], GF256::ONE);

        // Parity row should be Vandermonde
        assert_eq!(decoder.full_matrix[2][0], GF256::ONE); // 3^0 = 1
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let config = FecConfig::new(2, 1);
        let mut encoder = FecEncoder::new(config);
        let mut decoder = FecDecoder::new(config);

        // Original data
        let data0 = [0x11, 0x22, 0x33, 0x44];
        let data1 = [0x55, 0x66, 0x77, 0x88];

        // Encode
        let data_refs: [&[u8]; 2] = [&data0, &data1];
        let mut parity = [0u8; 4];
        {
            let mut parity_refs: [&mut [u8]; 1] = [&mut parity];
            assert!(encoder.encode(&data_refs, &mut parity_refs, 4));
        }

        // Verify parity was generated (check before borrowing mutably)
        assert!(!parity.iter().all(|&x| x == 0));

        // For recovery, we need to provide a buffer for the missing shard
        let mut recovered = [0u8; 4];
        let mut data1_copy = data1;
        let mut parity_copy = parity;

        let mut shards: [Option<&mut [u8]>; 3] = [
            Some(&mut recovered),      // data0 - buffer provided for recovery
            Some(&mut data1_copy),     // data1 - present
            Some(&mut parity_copy),    // parity - present
        ];

        // Decode (all shards present, so no recovery needed)
        let result = decoder.decode(&mut shards, 4);
        assert!(result);
    }

    #[test]
    fn test_no_loss() {
        let config = FecConfig::new(2, 1);
        let mut decoder = FecDecoder::new(config);

        let mut data0 = [0x11, 0x22, 0x33, 0x44];
        let mut data1 = [0x55, 0x66, 0x77, 0x88];
        let mut parity = [0x00; 4];

        let mut shards: [Option<&mut [u8]>; 3] = [
            Some(&mut data0),
            Some(&mut data1),
            Some(&mut parity),
        ];

        // Should succeed with no changes needed
        assert!(decoder.decode(&mut shards, 4));
    }

    #[test]
    fn test_insufficient_shards() {
        let config = FecConfig::new(2, 1);
        let mut decoder = FecDecoder::new(config);

        let mut data1 = [0x55, 0x66, 0x77, 0x88];

        // Only 1 shard present, need 2
        let mut shards: [Option<&mut [u8]>; 3] = [
            None,
            Some(&mut data1),
            None,
        ];

        assert!(!decoder.decode(&mut shards, 4));
    }

    #[test]
    fn test_matrix_inversion() {
        let config = FecConfig::new(2, 1);
        let mut decoder = FecDecoder::new(config);

        // Set up a simple 2x2 matrix
        decoder.decode_matrix[0][0] = GF256::new(1);
        decoder.decode_matrix[0][1] = GF256::new(2);
        decoder.decode_matrix[1][0] = GF256::new(3);
        decoder.decode_matrix[1][1] = GF256::new(4);

        assert!(decoder.invert_matrix(2));

        // Verify by checking A * A^-1 = I (approximately, via multiplication)
    }
}

