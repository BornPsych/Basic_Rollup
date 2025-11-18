use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;
use flate2::write::GzEncoder;
use flate2::read::GzDecoder;
use flate2::Compression;
use std::io::{Write, Read};

use crate::hash_utils::Hash;

/// Checkpoint of rollup state at a specific point in time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub checkpoint_id: u64,
    pub batch_id: u64,
    pub state_root: Hash,
    pub timestamp: u64,
    pub transaction_count: u64,
    pub account_count: usize,
    pub data_size: usize,
    pub metadata: CheckpointMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointMetadata {
    pub version: String,
    pub created_at: String,
    pub checkpoint_type: CheckpointType,
    pub compression: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckpointType {
    Full,        // Complete state snapshot
    Incremental, // Only changes since last checkpoint
    Emergency,   // Emergency backup
}

/// Checkpoint manager for creating and restoring checkpoints
pub struct CheckpointManager {
    checkpoint_dir: PathBuf,
    checkpoint_interval: u64, // Create checkpoint every N batches
    last_checkpoint_batch: std::sync::atomic::AtomicU64,
    checkpoint_counter: std::sync::atomic::AtomicU64,
}

impl CheckpointManager {
    pub fn new<P: AsRef<Path>>(checkpoint_dir: P, checkpoint_interval: u64) -> Result<Self> {
        let dir = checkpoint_dir.as_ref().to_path_buf();

        // Create checkpoint directory if it doesn't exist
        if !dir.exists() {
            fs::create_dir_all(&dir)?;
        }

        Ok(Self {
            checkpoint_dir: dir,
            checkpoint_interval,
            last_checkpoint_batch: std::sync::atomic::AtomicU64::new(0),
            checkpoint_counter: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Check if a checkpoint should be created
    pub fn should_checkpoint(&self, current_batch: u64) -> bool {
        let last = self.last_checkpoint_batch.load(std::sync::atomic::Ordering::Relaxed);
        current_batch - last >= self.checkpoint_interval
    }

    /// Create a checkpoint
    pub fn create_checkpoint(
        &self,
        batch_id: u64,
        state_root: Hash,
        transaction_count: u64,
        account_count: usize,
        state_data: &[u8],
        checkpoint_type: CheckpointType,
    ) -> Result<Checkpoint> {
        let checkpoint_id = self.checkpoint_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let checkpoint = Checkpoint {
            checkpoint_id,
            batch_id,
            state_root,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
            transaction_count,
            account_count,
            data_size: state_data.len(),
            metadata: CheckpointMetadata {
                version: "1.0.0".to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                checkpoint_type,
                compression: true,
            },
        };

        // Save checkpoint metadata
        let meta_path = self.checkpoint_dir.join(format!("checkpoint_{}_meta.json", checkpoint_id));
        let meta_json = serde_json::to_string_pretty(&checkpoint)?;
        fs::write(&meta_path, meta_json)?;

        // Save checkpoint data (compressed)
        let data_path = self.checkpoint_dir.join(format!("checkpoint_{}_data.bin.gz", checkpoint_id));
        let file = fs::File::create(&data_path)?;
        let mut encoder = GzEncoder::new(file, Compression::default());
        encoder.write_all(state_data)?;
        encoder.finish()?;

        self.last_checkpoint_batch.store(batch_id, std::sync::atomic::Ordering::Relaxed);

        log::info!(
            "Created checkpoint {} at batch {} (type: {:?}, size: {} bytes compressed)",
            checkpoint_id,
            batch_id,
            checkpoint_type,
            fs::metadata(&data_path)?.len()
        );

        Ok(checkpoint)
    }

    /// Load a checkpoint
    pub fn load_checkpoint(&self, checkpoint_id: u64) -> Result<(Checkpoint, Vec<u8>)> {
        // Load metadata
        let meta_path = self.checkpoint_dir.join(format!("checkpoint_{}_meta.json", checkpoint_id));
        let meta_json = fs::read_to_string(&meta_path)?;
        let checkpoint: Checkpoint = serde_json::from_str(&meta_json)?;

        // Load data
        let data_path = self.checkpoint_dir.join(format!("checkpoint_{}_data.bin.gz", checkpoint_id));
        let file = fs::File::open(&data_path)?;
        let mut decoder = GzDecoder::new(file);
        let mut data = Vec::new();
        decoder.read_to_end(&mut data)?;

        log::info!("Loaded checkpoint {} (batch {})", checkpoint_id, checkpoint.batch_id);

        Ok((checkpoint, data))
    }

    /// Get latest checkpoint
    pub fn get_latest_checkpoint(&self) -> Result<Option<Checkpoint>> {
        let checkpoints = self.list_checkpoints()?;

        if checkpoints.is_empty() {
            return Ok(None);
        }

        // Return the most recent checkpoint
        let latest = checkpoints.into_iter()
            .max_by_key(|c| c.checkpoint_id)
            .unwrap();

        Ok(Some(latest))
    }

    /// List all checkpoints
    pub fn list_checkpoints(&self) -> Result<Vec<Checkpoint>> {
        let mut checkpoints = Vec::new();

        for entry in fs::read_dir(&self.checkpoint_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Some(filename) = path.file_name().and_then(|s| s.to_str()) {
                    if filename.ends_with("_meta.json") {
                        let json = fs::read_to_string(&path)?;
                        if let Ok(checkpoint) = serde_json::from_str::<Checkpoint>(&json) {
                            checkpoints.push(checkpoint);
                        }
                    }
                }
            }
        }

        checkpoints.sort_by_key(|c| c.checkpoint_id);
        Ok(checkpoints)
    }

    /// Delete old checkpoints, keeping only the last N
    pub fn cleanup_old_checkpoints(&self, keep_count: usize) -> Result<usize> {
        let mut checkpoints = self.list_checkpoints()?;

        if checkpoints.len() <= keep_count {
            return Ok(0);
        }

        // Sort by checkpoint_id and keep only the most recent ones
        checkpoints.sort_by_key(|c| c.checkpoint_id);
        let to_delete = checkpoints.len() - keep_count;
        let mut deleted = 0;

        for checkpoint in checkpoints.iter().take(to_delete) {
            self.delete_checkpoint(checkpoint.checkpoint_id)?;
            deleted += 1;
        }

        log::info!("Deleted {} old checkpoints", deleted);
        Ok(deleted)
    }

    /// Delete a specific checkpoint
    pub fn delete_checkpoint(&self, checkpoint_id: u64) -> Result<()> {
        let meta_path = self.checkpoint_dir.join(format!("checkpoint_{}_meta.json", checkpoint_id));
        let data_path = self.checkpoint_dir.join(format!("checkpoint_{}_data.bin.gz", checkpoint_id));

        if meta_path.exists() {
            fs::remove_file(meta_path)?;
        }
        if data_path.exists() {
            fs::remove_file(data_path)?;
        }

        log::info!("Deleted checkpoint {}", checkpoint_id);
        Ok(())
    }

    /// Get checkpoint statistics
    pub fn get_stats(&self) -> Result<CheckpointStats> {
        let checkpoints = self.list_checkpoints()?;
        let total_size: u64 = checkpoints.iter()
            .map(|c| c.data_size as u64)
            .sum();

        Ok(CheckpointStats {
            total_checkpoints: checkpoints.len(),
            total_size_bytes: total_size,
            oldest_checkpoint: checkpoints.first().map(|c| c.checkpoint_id),
            newest_checkpoint: checkpoints.last().map(|c| c.checkpoint_id),
            last_checkpoint_batch: self.last_checkpoint_batch.load(std::sync::atomic::Ordering::Relaxed),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointStats {
    pub total_checkpoints: usize,
    pub total_size_bytes: u64,
    pub oldest_checkpoint: Option<u64>,
    pub newest_checkpoint: Option<u64>,
    pub last_checkpoint_batch: u64,
}

/// Recovery manager for restoring from checkpoints
pub struct RecoveryManager {
    checkpoint_manager: CheckpointManager,
}

impl RecoveryManager {
    pub fn new(checkpoint_manager: CheckpointManager) -> Self {
        Self {
            checkpoint_manager,
        }
    }

    /// Recover from the latest checkpoint
    pub fn recover_from_latest(&self) -> Result<Option<(Checkpoint, Vec<u8>)>> {
        let checkpoint = match self.checkpoint_manager.get_latest_checkpoint()? {
            Some(cp) => cp,
            None => return Ok(None),
        };

        log::info!("Starting recovery from checkpoint {}", checkpoint.checkpoint_id);

        let (checkpoint, data) = self.checkpoint_manager.load_checkpoint(checkpoint.checkpoint_id)?;

        log::info!(
            "Successfully recovered from checkpoint {} (batch {}, {} accounts)",
            checkpoint.checkpoint_id,
            checkpoint.batch_id,
            checkpoint.account_count
        );

        Ok(Some((checkpoint, data)))
    }

    /// Recover from a specific checkpoint
    pub fn recover_from_checkpoint(&self, checkpoint_id: u64) -> Result<(Checkpoint, Vec<u8>)> {
        log::info!("Starting recovery from checkpoint {}", checkpoint_id);

        let (checkpoint, data) = self.checkpoint_manager.load_checkpoint(checkpoint_id)?;

        log::info!("Successfully recovered from checkpoint {}", checkpoint_id);

        Ok((checkpoint, data))
    }

    /// Verify checkpoint integrity
    pub fn verify_checkpoint(&self, checkpoint_id: u64) -> Result<bool> {
        let (checkpoint, data) = self.checkpoint_manager.load_checkpoint(checkpoint_id)?;

        // Basic verification - in production you'd want more thorough checks
        let verification_passed = data.len() == checkpoint.data_size;

        if verification_passed {
            log::info!("Checkpoint {} verification passed", checkpoint_id);
        } else {
            log::error!("Checkpoint {} verification failed", checkpoint_id);
        }

        Ok(verification_passed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_checkpoint_creation() {
        let temp_dir = TempDir::new().unwrap();
        let manager = CheckpointManager::new(temp_dir.path(), 10).unwrap();

        let state_data = vec![1, 2, 3, 4, 5];
        let checkpoint = manager.create_checkpoint(
            1,
            Hash::new(b"test"),
            100,
            50,
            &state_data,
            CheckpointType::Full,
        ).unwrap();

        assert_eq!(checkpoint.batch_id, 1);
        assert_eq!(checkpoint.transaction_count, 100);
    }

    #[test]
    fn test_checkpoint_load() {
        let temp_dir = TempDir::new().unwrap();
        let manager = CheckpointManager::new(temp_dir.path(), 10).unwrap();

        let state_data = vec![1, 2, 3, 4, 5];
        let created = manager.create_checkpoint(
            1,
            Hash::new(b"test"),
            100,
            50,
            &state_data,
            CheckpointType::Full,
        ).unwrap();

        let (loaded, data) = manager.load_checkpoint(created.checkpoint_id).unwrap();
        assert_eq!(loaded.checkpoint_id, created.checkpoint_id);
        assert_eq!(data, state_data);
    }

    #[test]
    fn test_checkpoint_cleanup() {
        let temp_dir = TempDir::new().unwrap();
        let manager = CheckpointManager::new(temp_dir.path(), 10).unwrap();

        // Create 5 checkpoints
        for i in 0..5 {
            manager.create_checkpoint(
                i,
                Hash::new(&[i as u8]),
                100,
                50,
                &vec![i as u8; 100],
                CheckpointType::Full,
            ).unwrap();
        }

        // Keep only 2
        let deleted = manager.cleanup_old_checkpoints(2).unwrap();
        assert_eq!(deleted, 3);

        let remaining = manager.list_checkpoints().unwrap();
        assert_eq!(remaining.len(), 2);
    }
}
