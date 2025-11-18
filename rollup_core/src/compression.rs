use anyhow::{anyhow, Result};
use flate2::write::{GzEncoder, ZlibEncoder};
use flate2::read::{GzDecoder, ZlibDecoder};
use flate2::Compression;
use std::io::{Write, Read};
use serde::{Deserialize, Serialize};

/// Compression algorithms supported
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionAlgorithm {
    None,
    Gzip,
    Zlib,
}

/// Compressed data with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressedData {
    pub algorithm: CompressionAlgorithm,
    pub original_size: usize,
    pub compressed_size: usize,
    pub data: Vec<u8>,
    pub compression_ratio: f64,
}

impl CompressedData {
    pub fn new(data: Vec<u8>, original_size: usize, algorithm: CompressionAlgorithm) -> Self {
        let compressed_size = data.len();
        let compression_ratio = if original_size > 0 {
            compressed_size as f64 / original_size as f64
        } else {
            1.0
        };

        Self {
            algorithm,
            original_size,
            compressed_size,
            data,
            compression_ratio,
        }
    }
}

/// Compression utilities for batch data
pub struct CompressionEngine {
    default_algorithm: CompressionAlgorithm,
    compression_level: u32,
}

impl CompressionEngine {
    pub fn new(algorithm: CompressionAlgorithm, level: u32) -> Self {
        Self {
            default_algorithm: algorithm,
            compression_level: level.clamp(0, 9),
        }
    }

    /// Compress data using the default algorithm
    pub fn compress(&self, data: &[u8]) -> Result<CompressedData> {
        self.compress_with_algorithm(data, self.default_algorithm)
    }

    /// Compress data with a specific algorithm
    pub fn compress_with_algorithm(
        &self,
        data: &[u8],
        algorithm: CompressionAlgorithm,
    ) -> Result<CompressedData> {
        let original_size = data.len();

        let compressed = match algorithm {
            CompressionAlgorithm::None => data.to_vec(),
            CompressionAlgorithm::Gzip => {
                let mut encoder = GzEncoder::new(Vec::new(), Compression::new(self.compression_level));
                encoder.write_all(data)?;
                encoder.finish()?
            }
            CompressionAlgorithm::Zlib => {
                let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(self.compression_level));
                encoder.write_all(data)?;
                encoder.finish()?
            }
        };

        Ok(CompressedData::new(compressed, original_size, algorithm))
    }

    /// Decompress data
    pub fn decompress(&self, compressed: &CompressedData) -> Result<Vec<u8>> {
        match compressed.algorithm {
            CompressionAlgorithm::None => Ok(compressed.data.clone()),
            CompressionAlgorithm::Gzip => {
                let mut decoder = GzDecoder::new(compressed.data.as_slice());
                let mut decompressed = Vec::new();
                decoder.read_to_end(&mut decompressed)?;

                // Verify size
                if decompressed.len() != compressed.original_size {
                    return Err(anyhow!(
                        "Decompressed size mismatch: expected {}, got {}",
                        compressed.original_size,
                        decompressed.len()
                    ));
                }

                Ok(decompressed)
            }
            CompressionAlgorithm::Zlib => {
                let mut decoder = ZlibDecoder::new(compressed.data.as_slice());
                let mut decompressed = Vec::new();
                decoder.read_to_end(&mut decompressed)?;

                // Verify size
                if decompressed.len() != compressed.original_size {
                    return Err(anyhow!(
                        "Decompressed size mismatch: expected {}, got {}",
                        compressed.original_size,
                        decompressed.len()
                    ));
                }

                Ok(decompressed)
            }
        }
    }

    /// Find the best compression algorithm for given data
    pub fn find_best_compression(&self, data: &[u8]) -> Result<CompressedData> {
        let algorithms = [
            CompressionAlgorithm::Gzip,
            CompressionAlgorithm::Zlib,
        ];

        let mut best = CompressedData::new(data.to_vec(), data.len(), CompressionAlgorithm::None);
        let mut best_ratio = 1.0;

        for algorithm in algorithms {
            let compressed = self.compress_with_algorithm(data, algorithm)?;
            if compressed.compression_ratio < best_ratio {
                best_ratio = compressed.compression_ratio;
                best = compressed;
            }
        }

        Ok(best)
    }

    /// Compress transaction batch
    pub fn compress_batch(&self, transactions: &[Vec<u8>]) -> Result<CompressedData> {
        // Concatenate all transaction data
        let mut combined = Vec::new();
        for tx_data in transactions {
            combined.extend_from_slice(tx_data);
        }

        self.compress(&combined)
    }

    /// Get compression statistics
    pub fn get_compression_stats(&self, data: &[u8]) -> Result<CompressionStats> {
        let original_size = data.len();
        let mut stats = CompressionStats {
            original_size,
            algorithms: Vec::new(),
        };

        for algorithm in [CompressionAlgorithm::Gzip, CompressionAlgorithm::Zlib] {
            let compressed = self.compress_with_algorithm(data, algorithm)?;
            stats.algorithms.push(AlgorithmStats {
                algorithm,
                compressed_size: compressed.compressed_size,
                compression_ratio: compressed.compression_ratio,
                space_saved: original_size.saturating_sub(compressed.compressed_size),
            });
        }

        Ok(stats)
    }
}

impl Default for CompressionEngine {
    fn default() -> Self {
        Self::new(CompressionAlgorithm::Gzip, 6) // Default to gzip level 6
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionStats {
    pub original_size: usize,
    pub algorithms: Vec<AlgorithmStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlgorithmStats {
    pub algorithm: CompressionAlgorithm,
    pub compressed_size: usize,
    pub compression_ratio: f64,
    pub space_saved: usize,
}

/// Batch data compression wrapper
pub struct BatchCompressor {
    engine: CompressionEngine,
    use_adaptive_compression: bool,
}

impl BatchCompressor {
    pub fn new(use_adaptive_compression: bool) -> Self {
        Self {
            engine: CompressionEngine::default(),
            use_adaptive_compression,
        }
    }

    /// Compress a batch with optional adaptive algorithm selection
    pub fn compress_batch_data(&self, data: &[u8]) -> Result<CompressedData> {
        if self.use_adaptive_compression {
            // Use the best compression algorithm
            self.engine.find_best_compression(data)
        } else {
            // Use default algorithm
            self.engine.compress(data)
        }
    }

    /// Decompress batch data
    pub fn decompress_batch_data(&self, compressed: &CompressedData) -> Result<Vec<u8>> {
        self.engine.decompress(compressed)
    }

    /// Calculate potential space savings
    pub fn calculate_savings(&self, data: &[u8]) -> Result<CompressionSavings> {
        let stats = self.engine.get_compression_stats(data)?;

        let best_algorithm = stats.algorithms
            .iter()
            .min_by(|a, b| a.compressed_size.cmp(&b.compressed_size))
            .ok_or_else(|| anyhow!("No compression algorithms available"))?;

        Ok(CompressionSavings {
            original_size: stats.original_size,
            best_compressed_size: best_algorithm.compressed_size,
            best_algorithm: best_algorithm.algorithm,
            space_saved_bytes: best_algorithm.space_saved,
            space_saved_percent: (best_algorithm.space_saved as f64 / stats.original_size as f64 * 100.0) as u32,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionSavings {
    pub original_size: usize,
    pub best_compressed_size: usize,
    pub best_algorithm: CompressionAlgorithm,
    pub space_saved_bytes: usize,
    pub space_saved_percent: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gzip_compression() {
        let engine = CompressionEngine::default();
        let data = b"Hello, World! ".repeat(100);

        let compressed = engine.compress(&data).unwrap();
        assert!(compressed.compressed_size < data.len());

        let decompressed = engine.decompress(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_compression_algorithms() {
        let engine = CompressionEngine::default();
        let data = b"Test data ".repeat(50);

        for algorithm in [CompressionAlgorithm::Gzip, CompressionAlgorithm::Zlib] {
            let compressed = engine.compress_with_algorithm(&data, algorithm).unwrap();
            let decompressed = engine.decompress(&compressed).unwrap();
            assert_eq!(decompressed, data);
        }
    }

    #[test]
    fn test_best_compression() {
        let engine = CompressionEngine::default();
        let data = b"Highly compressible data ".repeat(100);

        let best = engine.find_best_compression(&data).unwrap();
        assert!(best.compression_ratio < 1.0);
    }

    #[test]
    fn test_batch_compressor() {
        let compressor = BatchCompressor::new(true);
        let data = b"Batch data ".repeat(50);

        let compressed = compressor.compress_batch_data(&data).unwrap();
        let decompressed = compressor.decompress_batch_data(&compressed).unwrap();

        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_compression_savings() {
        let compressor = BatchCompressor::new(true);
        let data = b"Compressible ".repeat(100);

        let savings = compressor.calculate_savings(&data).unwrap();
        assert!(savings.space_saved_bytes > 0);
        assert!(savings.space_saved_percent > 0);
    }
}
