use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use solana_sdk::keccak::{Hash, Hasher};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::state::TransactionBatch;

/// Data availability layer for storing and retrieving rollup data
/// This ensures all transaction data is available for verification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataBlob {
    pub batch_id: u64,
    pub data: Vec<u8>,
    pub hash: Hash,
    pub timestamp: u64,
}

impl DataBlob {
    pub fn new(batch_id: u64, data: Vec<u8>) -> Self {
        let mut hasher = Hasher::default();
        hasher.hash(&data);
        let hash = hasher.result();

        Self {
            batch_id,
            data,
            hash,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        }
    }

    pub fn from_batch(batch: &TransactionBatch) -> Result<Self> {
        let data = bincode::serialize(batch)
            .map_err(|e| anyhow!("Failed to serialize batch: {}", e))?;
        Ok(DataBlob::new(batch.batch_id, data))
    }

    pub fn verify(&self) -> bool {
        let mut hasher = Hasher::default();
        hasher.hash(&self.data);
        hasher.result() == self.hash
    }
}

/// Data availability layer implementation
pub struct DataAvailabilityLayer {
    /// Storage for data blobs
    storage: Arc<RwLock<HashMap<u64, DataBlob>>>,
    /// Index by hash for quick lookups
    hash_index: Arc<RwLock<HashMap<Hash, u64>>>,
}

impl DataAvailabilityLayer {
    pub fn new() -> Self {
        Self {
            storage: Arc::new(RwLock::new(HashMap::new())),
            hash_index: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Store a data blob
    pub fn store(&self, blob: DataBlob) -> Result<()> {
        if !blob.verify() {
            return Err(anyhow!("Data blob verification failed"));
        }

        let batch_id = blob.batch_id;
        let hash = blob.hash;

        self.storage.write().unwrap().insert(batch_id, blob);
        self.hash_index.write().unwrap().insert(hash, batch_id);

        log::info!("Stored data blob for batch {}", batch_id);
        Ok(())
    }

    /// Retrieve a data blob by batch ID
    pub fn get_by_batch_id(&self, batch_id: u64) -> Option<DataBlob> {
        self.storage.read().unwrap().get(&batch_id).cloned()
    }

    /// Retrieve a data blob by hash
    pub fn get_by_hash(&self, hash: &Hash) -> Option<DataBlob> {
        let batch_id = self.hash_index.read().unwrap().get(hash).copied()?;
        self.get_by_batch_id(batch_id)
    }

    /// Check if data is available for a batch
    pub fn is_available(&self, batch_id: u64) -> bool {
        self.storage.read().unwrap().contains_key(&batch_id)
    }

    /// Get all stored batch IDs
    pub fn get_all_batch_ids(&self) -> Vec<u64> {
        let mut ids: Vec<_> = self.storage.read().unwrap().keys().copied().collect();
        ids.sort();
        ids
    }

    /// Get storage statistics
    pub fn get_stats(&self) -> DAStats {
        let storage = self.storage.read().unwrap();
        let total_size: usize = storage.values().map(|blob| blob.data.len()).sum();

        DAStats {
            total_blobs: storage.len(),
            total_bytes: total_size,
        }
    }

    /// Prune old data (for cleanup)
    pub fn prune_before(&self, batch_id: u64) -> usize {
        let mut storage = self.storage.write().unwrap();
        let mut hash_index = self.hash_index.write().unwrap();

        let to_remove: Vec<_> = storage
            .keys()
            .filter(|&&id| id < batch_id)
            .copied()
            .collect();

        let count = to_remove.len();
        for id in to_remove {
            if let Some(blob) = storage.remove(&id) {
                hash_index.remove(&blob.hash);
            }
        }

        log::info!("Pruned {} data blobs before batch {}", count, batch_id);
        count
    }
}

impl Default for DataAvailabilityLayer {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DAStats {
    pub total_blobs: usize,
    pub total_bytes: usize,
}

/// Data availability commitment for L1 settlement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DACommitment {
    pub batch_id: u64,
    pub data_hash: Hash,
    pub data_size: usize,
    pub availability_proof: Vec<u8>, // Could be a KZG commitment or similar
}

impl DACommitment {
    pub fn from_blob(blob: &DataBlob) -> Self {
        Self {
            batch_id: blob.batch_id,
            data_hash: blob.hash,
            data_size: blob.data.len(),
            availability_proof: Vec::new(), // In production, generate actual proof
        }
    }

    /// Verify the commitment matches the data
    pub fn verify(&self, blob: &DataBlob) -> bool {
        self.batch_id == blob.batch_id
            && self.data_hash == blob.hash
            && self.data_size == blob.data.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_blob_creation() {
        let data = vec![1, 2, 3, 4, 5];
        let blob = DataBlob::new(1, data.clone());

        assert_eq!(blob.batch_id, 1);
        assert_eq!(blob.data, data);
        assert!(blob.verify());
    }

    #[test]
    fn test_data_availability_layer() {
        let dal = DataAvailabilityLayer::new();

        let blob1 = DataBlob::new(1, vec![1, 2, 3]);
        let blob2 = DataBlob::new(2, vec![4, 5, 6]);

        dal.store(blob1.clone()).unwrap();
        dal.store(blob2.clone()).unwrap();

        assert!(dal.is_available(1));
        assert!(dal.is_available(2));
        assert!(!dal.is_available(3));

        let retrieved = dal.get_by_batch_id(1).unwrap();
        assert_eq!(retrieved.batch_id, blob1.batch_id);
        assert_eq!(retrieved.data, blob1.data);
    }

    #[test]
    fn test_dal_pruning() {
        let dal = DataAvailabilityLayer::new();

        for i in 0..10 {
            let blob = DataBlob::new(i, vec![i as u8]);
            dal.store(blob).unwrap();
        }

        assert_eq!(dal.get_all_batch_ids().len(), 10);

        let pruned = dal.prune_before(5);
        assert_eq!(pruned, 5);
        assert_eq!(dal.get_all_batch_ids().len(), 5);
        assert!(!dal.is_available(0));
        assert!(dal.is_available(5));
    }

    #[test]
    fn test_da_commitment() {
        let blob = DataBlob::new(1, vec![1, 2, 3, 4, 5]);
        let commitment = DACommitment::from_blob(&blob);

        assert!(commitment.verify(&blob));
    }
}
