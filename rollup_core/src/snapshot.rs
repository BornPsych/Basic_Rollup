use anyhow::{anyhow, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::hash_utils::Hash;

/// State snapshot for fast synchronization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub snapshot_id: u64,
    pub batch_id: u64,
    pub state_root: Hash,
    pub timestamp: u64,
    pub account_count: usize,
    pub total_balance: u64,
    pub snapshot_type: SnapshotType,
    pub compressed_size: usize,
    pub uncompressed_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SnapshotType {
    Full,
    Incremental { base_snapshot: u64 },
    Archive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountSnapshot {
    pub address: String,
    pub balance: u64,
    pub nonce: u64,
    pub state_data: Vec<u8>,
    pub code_hash: Option<Hash>,
}

/// Snapshot manager for creating and managing state snapshots
pub struct SnapshotManager {
    snapshot_dir: PathBuf,
    snapshot_counter: AtomicU64,
    snapshots: Arc<DashMap<u64, StateSnapshot>>,
    max_snapshots: usize,
}

impl SnapshotManager {
    pub fn new<P: AsRef<Path>>(snapshot_dir: P, max_snapshots: usize) -> Result<Self> {
        let snapshot_dir = snapshot_dir.as_ref().to_path_buf();
        fs::create_dir_all(&snapshot_dir)?;

        Ok(Self {
            snapshot_dir,
            snapshot_counter: AtomicU64::new(0),
            snapshots: Arc::new(DashMap::new()),
            max_snapshots,
        })
    }

    /// Create a full state snapshot
    pub fn create_full_snapshot(
        &self,
        batch_id: u64,
        state_root: Hash,
        accounts: &HashMap<String, AccountSnapshot>,
    ) -> Result<StateSnapshot> {
        let snapshot_id = self.snapshot_counter.fetch_add(1, Ordering::SeqCst);

        // Serialize accounts
        let serialized = bincode::serialize(accounts)?;
        let uncompressed_size = serialized.len();

        // Compress snapshot data
        let compressed = self.compress_data(&serialized)?;
        let compressed_size = compressed.len();

        let snapshot = StateSnapshot {
            snapshot_id,
            batch_id,
            state_root,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_secs(),
            account_count: accounts.len(),
            total_balance: accounts.values().map(|a| a.balance).sum(),
            snapshot_type: SnapshotType::Full,
            compressed_size,
            uncompressed_size,
        };

        // Save to disk
        self.save_snapshot_data(snapshot_id, &compressed)?;
        self.save_snapshot_metadata(&snapshot)?;

        // Store in memory
        self.snapshots.insert(snapshot_id, snapshot.clone());

        log::info!(
            "Created full snapshot {} at batch {} ({} accounts, compression: {:.1}%)",
            snapshot_id,
            batch_id,
            accounts.len(),
            (1.0 - compressed_size as f64 / uncompressed_size as f64) * 100.0
        );

        // Cleanup old snapshots
        self.cleanup_old_snapshots()?;

        Ok(snapshot)
    }

    /// Create an incremental snapshot
    pub fn create_incremental_snapshot(
        &self,
        base_snapshot_id: u64,
        batch_id: u64,
        state_root: Hash,
        changed_accounts: &HashMap<String, AccountSnapshot>,
    ) -> Result<StateSnapshot> {
        let snapshot_id = self.snapshot_counter.fetch_add(1, Ordering::SeqCst);

        // Serialize only changed accounts
        let serialized = bincode::serialize(changed_accounts)?;
        let uncompressed_size = serialized.len();

        // Compress
        let compressed = self.compress_data(&serialized)?;
        let compressed_size = compressed.len();

        let snapshot = StateSnapshot {
            snapshot_id,
            batch_id,
            state_root,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_secs(),
            account_count: changed_accounts.len(),
            total_balance: changed_accounts.values().map(|a| a.balance).sum(),
            snapshot_type: SnapshotType::Incremental {
                base_snapshot: base_snapshot_id,
            },
            compressed_size,
            uncompressed_size,
        };

        // Save to disk
        self.save_snapshot_data(snapshot_id, &compressed)?;
        self.save_snapshot_metadata(&snapshot)?;

        // Store in memory
        self.snapshots.insert(snapshot_id, snapshot.clone());

        log::info!(
            "Created incremental snapshot {} (base: {}) at batch {} ({} changed accounts)",
            snapshot_id,
            base_snapshot_id,
            batch_id,
            changed_accounts.len()
        );

        Ok(snapshot)
    }

    /// Load snapshot from disk
    pub fn load_snapshot(&self, snapshot_id: u64) -> Result<HashMap<String, AccountSnapshot>> {
        // Load metadata
        let snapshot = self
            .snapshots
            .get(&snapshot_id)
            .map(|s| s.clone())
            .ok_or_else(|| anyhow!("Snapshot {} not found", snapshot_id))?;

        // Load data
        let compressed = self.load_snapshot_data(snapshot_id)?;
        let serialized = self.decompress_data(&compressed)?;
        let accounts: HashMap<String, AccountSnapshot> = bincode::deserialize(&serialized)?;

        log::info!(
            "Loaded snapshot {} ({} accounts)",
            snapshot_id,
            accounts.len()
        );

        Ok(accounts)
    }

    /// Load latest snapshot
    pub fn load_latest_snapshot(&self) -> Result<(StateSnapshot, HashMap<String, AccountSnapshot>)> {
        let latest = self.get_latest_snapshot()?;
        let accounts = self.load_snapshot(latest.snapshot_id)?;
        Ok((latest, accounts))
    }

    /// Get snapshot metadata
    pub fn get_snapshot(&self, snapshot_id: u64) -> Option<StateSnapshot> {
        self.snapshots.get(&snapshot_id).map(|s| s.clone())
    }

    /// Get latest snapshot metadata
    pub fn get_latest_snapshot(&self) -> Result<StateSnapshot> {
        self.snapshots
            .iter()
            .max_by_key(|entry| entry.value().snapshot_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| anyhow!("No snapshots available"))
    }

    /// List all snapshots
    pub fn list_snapshots(&self) -> Vec<StateSnapshot> {
        let mut snapshots: Vec<_> = self.snapshots.iter().map(|e| e.value().clone()).collect();
        snapshots.sort_by_key(|s| s.snapshot_id);
        snapshots
    }

    /// Delete snapshot
    pub fn delete_snapshot(&self, snapshot_id: u64) -> Result<()> {
        // Remove from memory
        self.snapshots.remove(&snapshot_id);

        // Delete files
        let data_path = self.get_snapshot_data_path(snapshot_id);
        let meta_path = self.get_snapshot_meta_path(snapshot_id);

        if data_path.exists() {
            fs::remove_file(data_path)?;
        }

        if meta_path.exists() {
            fs::remove_file(meta_path)?;
        }

        log::info!("Deleted snapshot {}", snapshot_id);
        Ok(())
    }

    /// Archive old snapshot (mark as archive, don't delete)
    pub fn archive_snapshot(&self, snapshot_id: u64) -> Result<()> {
        if let Some(mut snapshot) = self.snapshots.get_mut(&snapshot_id) {
            snapshot.snapshot_type = SnapshotType::Archive;
            self.save_snapshot_metadata(&snapshot)?;
            log::info!("Archived snapshot {}", snapshot_id);
        }
        Ok(())
    }

    /// Cleanup old snapshots (keep only max_snapshots)
    fn cleanup_old_snapshots(&self) -> Result<()> {
        let mut snapshots: Vec<_> = self.snapshots.iter().map(|e| e.value().clone()).collect();
        snapshots.sort_by_key(|s| s.snapshot_id);

        // Keep archive snapshots and latest max_snapshots
        let to_delete: Vec<_> = snapshots
            .iter()
            .filter(|s| s.snapshot_type != SnapshotType::Archive)
            .rev()
            .skip(self.max_snapshots)
            .map(|s| s.snapshot_id)
            .collect();

        for snapshot_id in to_delete {
            self.delete_snapshot(snapshot_id)?;
        }

        Ok(())
    }

    /// Compress data using gzip
    fn compress_data(&self, data: &[u8]) -> Result<Vec<u8>> {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(data)?;
        Ok(encoder.finish()?)
    }

    /// Decompress data
    fn decompress_data(&self, data: &[u8]) -> Result<Vec<u8>> {
        use flate2::read::GzDecoder;
        use std::io::Read;

        let mut decoder = GzDecoder::new(data);
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed)?;
        Ok(decompressed)
    }

    /// Save snapshot data to disk
    fn save_snapshot_data(&self, snapshot_id: u64, data: &[u8]) -> Result<()> {
        let path = self.get_snapshot_data_path(snapshot_id);
        fs::write(path, data)?;
        Ok(())
    }

    /// Load snapshot data from disk
    fn load_snapshot_data(&self, snapshot_id: u64) -> Result<Vec<u8>> {
        let path = self.get_snapshot_data_path(snapshot_id);
        Ok(fs::read(path)?)
    }

    /// Save snapshot metadata
    fn save_snapshot_metadata(&self, snapshot: &StateSnapshot) -> Result<()> {
        let path = self.get_snapshot_meta_path(snapshot.snapshot_id);
        let json = serde_json::to_string_pretty(snapshot)?;
        fs::write(path, json)?;
        Ok(())
    }

    fn get_snapshot_data_path(&self, snapshot_id: u64) -> PathBuf {
        self.snapshot_dir.join(format!("snapshot_{}.dat", snapshot_id))
    }

    fn get_snapshot_meta_path(&self, snapshot_id: u64) -> PathBuf {
        self.snapshot_dir.join(format!("snapshot_{}.json", snapshot_id))
    }

    /// Get snapshot statistics
    pub fn get_stats(&self) -> SnapshotStats {
        let snapshots: Vec<_> = self.snapshots.iter().map(|e| e.value().clone()).collect();

        SnapshotStats {
            total_snapshots: snapshots.len(),
            full_snapshots: snapshots
                .iter()
                .filter(|s| matches!(s.snapshot_type, SnapshotType::Full))
                .count(),
            incremental_snapshots: snapshots
                .iter()
                .filter(|s| matches!(s.snapshot_type, SnapshotType::Incremental { .. }))
                .count(),
            archive_snapshots: snapshots
                .iter()
                .filter(|s| matches!(s.snapshot_type, SnapshotType::Archive))
                .count(),
            total_compressed_size: snapshots.iter().map(|s| s.compressed_size).sum(),
            total_uncompressed_size: snapshots.iter().map(|s| s.uncompressed_size).sum(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotStats {
    pub total_snapshots: usize,
    pub full_snapshots: usize,
    pub incremental_snapshots: usize,
    pub archive_snapshots: usize,
    pub total_compressed_size: usize,
    pub total_uncompressed_size: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_creation() {
        let temp_dir = std::env::temp_dir().join("rollup_snapshots_test");
        let manager = SnapshotManager::new(&temp_dir, 10).unwrap();

        let mut accounts = HashMap::new();
        accounts.insert(
            "addr1".to_string(),
            AccountSnapshot {
                address: "addr1".to_string(),
                balance: 1000,
                nonce: 1,
                state_data: vec![1, 2, 3],
                code_hash: None,
            },
        );

        let snapshot = manager
            .create_full_snapshot(1, Hash::new(b"state_root"), &accounts)
            .unwrap();

        assert_eq!(snapshot.account_count, 1);
        assert_eq!(snapshot.batch_id, 1);

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_snapshot_load() {
        let temp_dir = std::env::temp_dir().join("rollup_snapshots_load_test");
        let manager = SnapshotManager::new(&temp_dir, 10).unwrap();

        let mut accounts = HashMap::new();
        accounts.insert(
            "addr1".to_string(),
            AccountSnapshot {
                address: "addr1".to_string(),
                balance: 1000,
                nonce: 1,
                state_data: vec![1, 2, 3],
                code_hash: None,
            },
        );

        let snapshot = manager
            .create_full_snapshot(1, Hash::new(b"state_root"), &accounts)
            .unwrap();

        let loaded = manager.load_snapshot(snapshot.snapshot_id).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.get("addr1").unwrap().balance, 1000);

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_incremental_snapshot() {
        let temp_dir = std::env::temp_dir().join("rollup_snapshots_inc_test");
        let manager = SnapshotManager::new(&temp_dir, 10).unwrap();

        let mut accounts = HashMap::new();
        accounts.insert(
            "addr1".to_string(),
            AccountSnapshot {
                address: "addr1".to_string(),
                balance: 1000,
                nonce: 1,
                state_data: vec![1, 2, 3],
                code_hash: None,
            },
        );

        let base_snapshot = manager
            .create_full_snapshot(1, Hash::new(b"state_root"), &accounts)
            .unwrap();

        let mut changed_accounts = HashMap::new();
        changed_accounts.insert(
            "addr1".to_string(),
            AccountSnapshot {
                address: "addr1".to_string(),
                balance: 2000,
                nonce: 2,
                state_data: vec![4, 5, 6],
                code_hash: None,
            },
        );

        let inc_snapshot = manager
            .create_incremental_snapshot(
                base_snapshot.snapshot_id,
                2,
                Hash::new(b"new_state_root"),
                &changed_accounts,
            )
            .unwrap();

        assert!(matches!(
            inc_snapshot.snapshot_type,
            SnapshotType::Incremental { .. }
        ));

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
