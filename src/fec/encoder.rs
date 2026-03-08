//! Reed-Solomon FEC Encoder
//!
//! Generates parity shards from data shards using Vandermonde matrix.
//! Zero allocation - uses preallocated buffers.

use super::gf256::{GF256, mul_add_slice};
use super::{FecConfig, MAX_SHARDS, MAX_SHARD_SIZE};

/// FEC Encoder
///
/// Generates parity shards from data shards using Reed-Solomon encoding.
/// All matrices and buffers are preallocated.
pub struct FecEncoder {
    /// FEC configuration
    config: FecConfig,
    /// Encoding matrix (Vandermonde-derived)
    /// Only stores parity rows: [parity_shards][data_shards]
    encode_matrix: [[GF256; MAX_SHARDS]; MAX_SHARDS],
    /// Temporary buffer for encoding (reserved for future optimization)
    #[allow(dead_code)]
    temp: [u8; MAX_SHARD_SIZE],
}

impl FecEncoder {
    /// Create new encoder with given configuration
    pub fn new(config: FecConfig) -> Self {
        let mut encoder = Self {
            config,
            encode_matrix: [[GF256::ZERO; MAX_SHARDS]; MAX_SHARDS],
            temp: [0u8; MAX_SHARD_SIZE],
        };
        encoder.build_encode_matrix();
        encoder
    }

    /// Build Vandermonde-derived encoding matrix
    ///
    /// The encoding matrix has the form:
    /// ```text
    /// [ I_k ]     <- identity (data shards pass through)
    /// [ V_m ]     <- Vandermonde rows (generate parity)
    /// ```
    ///
    /// We only store V_m since I_k is implicit.
    fn build_encode_matrix(&mut self) {
        let k = self.config.data_shards as usize;
        let m = self.config.parity_shards as usize;

        // Build Vandermonde matrix rows for parity
        // V[i][j] = (i+1)^j where i is parity row, j is data column
        for i in 0..m {
            let base = GF256::new((k + i + 1) as u8);
            for j in 0..k {
                self.encode_matrix[i][j] = base.pow(j as u8);
            }
        }
    }

    /// Encode data shards to generate parity shards
    ///
    /// # Arguments
    /// * `data_shards` - Slice of k data shard buffers
    /// * `parity_shards` - Mutable slice of m parity shard buffers (output)
    /// * `shard_size` - Size of each shard in bytes
    ///
    /// # Returns
    /// `true` if encoding succeeded, `false` if parameters invalid
    #[inline]
    pub fn encode(
        &mut self,
        data_shards: &[&[u8]],
        parity_shards: &mut [&mut [u8]],
        shard_size: usize,
    ) -> bool {
        let k = self.config.data_shards as usize;
        let m = self.config.parity_shards as usize;

        // Validate inputs
        if data_shards.len() != k || parity_shards.len() != m {
            return false;
        }
        if shard_size > MAX_SHARD_SIZE {
            return false;
        }

        // Clear parity shards
        for parity in parity_shards.iter_mut() {
            parity[..shard_size].fill(0);
        }

        // Generate each parity shard
        // parity[i] = sum(encode_matrix[i][j] * data[j]) for j in 0..k
        for (i, parity) in parity_shards.iter_mut().enumerate() {
            for (j, data) in data_shards.iter().enumerate() {
                let coeff = self.encode_matrix[i][j];
                if coeff.0 != 0 {
                    mul_add_slice(&mut parity[..shard_size], &data[..shard_size], coeff);
                }
            }
        }

        true
    }

    /// Encode data shards from a contiguous buffer
    ///
    /// More efficient when shards are stored contiguously.
    #[inline]
    pub fn encode_contiguous(
        &mut self,
        data: &[u8],
        parity: &mut [u8],
        shard_size: usize,
    ) -> bool {
        let k = self.config.data_shards as usize;
        let m = self.config.parity_shards as usize;

        // Validate sizes
        if data.len() < k * shard_size || parity.len() < m * shard_size {
            return false;
        }
        if shard_size > MAX_SHARD_SIZE {
            return false;
        }

        // Clear parity
        parity[..m * shard_size].fill(0);

        // Generate parity shards
        for i in 0..m {
            let parity_start = i * shard_size;
            let parity_shard = &mut parity[parity_start..parity_start + shard_size];

            for j in 0..k {
                let coeff = self.encode_matrix[i][j];
                if coeff.0 != 0 {
                    let data_start = j * shard_size;
                    let data_shard = &data[data_start..data_start + shard_size];
                    mul_add_slice(parity_shard, data_shard, coeff);
                }
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

impl Default for FecEncoder {
    fn default() -> Self {
        Self::new(FecConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encoder_creation() {
        let config = FecConfig::new(4, 2);
        let encoder = FecEncoder::new(config);
        assert_eq!(encoder.config().data_shards, 4);
        assert_eq!(encoder.config().parity_shards, 2);
    }

    #[test]
    fn test_encode_basic() {
        let config = FecConfig::new(2, 1);
        let mut encoder = FecEncoder::new(config);

        let data0 = [0x01, 0x02, 0x03, 0x04];
        let data1 = [0x05, 0x06, 0x07, 0x08];
        let data_shards: [&[u8]; 2] = [&data0, &data1];

        let mut parity0 = [0u8; 4];
        let mut parity_shards: [&mut [u8]; 1] = [&mut parity0];

        assert!(encoder.encode(&data_shards, &mut parity_shards, 4));

        // Parity should not be all zeros (unless data XORs to zero)
        // With Vandermonde encoding, parity = coeff[0]*data0 + coeff[1]*data1
        assert!(!parity0.iter().all(|&x| x == 0) ||
                (data0.iter().zip(data1.iter()).all(|(&a, &b)| a == b)));
    }

    #[test]
    fn test_encode_contiguous() {
        let config = FecConfig::new(2, 1);
        let mut encoder = FecEncoder::new(config);

        let data = [
            0x01, 0x02, 0x03, 0x04,  // shard 0
            0x05, 0x06, 0x07, 0x08,  // shard 1
        ];
        let mut parity = [0u8; 4];

        assert!(encoder.encode_contiguous(&data, &mut parity, 4));
    }

    #[test]
    fn test_encode_invalid_params() {
        let config = FecConfig::new(2, 1);
        let mut encoder = FecEncoder::new(config);

        let data0 = [0u8; 4];
        let data_shards: [&[u8]; 1] = [&data0]; // Wrong count
        let mut parity0 = [0u8; 4];
        let mut parity_shards: [&mut [u8]; 1] = [&mut parity0];

        // Should fail with wrong number of data shards
        assert!(!encoder.encode(&data_shards, &mut parity_shards, 4));
    }

    #[test]
    fn test_encode_matrix_properties() {
        let config = FecConfig::new(4, 2);
        let encoder = FecEncoder::new(config);

        // First column should be all 1s (x^0 = 1 for any x)
        for i in 0..2 {
            assert_eq!(encoder.encode_matrix[i][0], GF256::ONE);
        }
    }
}

