use anyhow::{anyhow, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use solana_sdk::{pubkey::Pubkey, transaction::Transaction};
use std::collections::BinaryHeap;
use std::cmp::Ordering;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use crate::hash_utils::{Hash, Hasher};

/// Transaction priority level
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    Low = 0,
    Medium = 1,
    High = 2,
    Urgent = 3,
}

/// Mempool transaction with metadata
#[derive(Debug, Clone)]
pub struct MempoolTransaction {
    pub transaction: Transaction,
    pub hash: Hash,
    pub priority: Priority,
    pub fee: u64,
    pub timestamp: u64,
    pub nonce: u64,
    pub sender: Option<Pubkey>,
}

impl PartialEq for MempoolTransaction {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
    }
}

impl Eq for MempoolTransaction {}

impl PartialOrd for MempoolTransaction {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MempoolTransaction {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher priority first
        match self.priority.cmp(&other.priority) {
            Ordering::Equal => {
                // Higher fee first
                match self.fee.cmp(&other.fee) {
                    Ordering::Equal => {
                        // Earlier timestamp first
                        other.timestamp.cmp(&self.timestamp)
                    }
                    ordering => ordering,
                }
            }
            ordering => ordering,
        }
    }
}

/// Transaction mempool with prioritization
pub struct Mempool {
    /// Transactions indexed by hash
    transactions: Arc<DashMap<Hash, MempoolTransaction>>,
    /// Priority queue for transaction ordering
    queue: Arc<parking_lot::Mutex<BinaryHeap<MempoolTransaction>>>,
    /// Maximum mempool size
    max_size: usize,
    /// Current mempool size
    current_size: AtomicU64,
    /// Nonce tracker
    nonce_counter: AtomicU64,
}

impl Mempool {
    pub fn new(max_size: usize) -> Self {
        Self {
            transactions: Arc::new(DashMap::new()),
            queue: Arc::new(parking_lot::Mutex::new(BinaryHeap::new())),
            max_size,
            current_size: AtomicU64::new(0),
            nonce_counter: AtomicU64::new(0),
        }
    }

    /// Add a transaction to the mempool
    pub fn add_transaction(
        &self,
        transaction: Transaction,
        priority: Priority,
        fee: u64,
    ) -> Result<Hash> {
        // Check if mempool is full
        if self.current_size.load(AtomicOrdering::Relaxed) >= self.max_size as u64 {
            return Err(anyhow!("Mempool is full"));
        }

        // Calculate transaction hash
        let tx_bytes = bincode::serialize(&transaction)?;
        let mut hasher = Hasher::new();
        hasher.update(&tx_bytes);
        let hash = hasher.finalize();

        // Check if transaction already exists
        if self.transactions.contains_key(&hash) {
            return Err(anyhow!("Transaction already in mempool"));
        }

        // Extract sender
        let sender = transaction.message.account_keys.first().copied();

        // Create mempool transaction
        let mempool_tx = MempoolTransaction {
            transaction,
            hash,
            priority,
            fee,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
            nonce: self.nonce_counter.fetch_add(1, AtomicOrdering::SeqCst),
            sender,
        };

        // Add to storage and queue
        self.transactions.insert(hash, mempool_tx.clone());
        self.queue.lock().push(mempool_tx);
        self.current_size.fetch_add(1, AtomicOrdering::SeqCst);

        log::info!("Added transaction {} to mempool with priority {:?}", hash, priority);

        Ok(hash)
    }

    /// Get next transaction from mempool (highest priority)
    pub fn pop_transaction(&self) -> Option<MempoolTransaction> {
        let mut queue = self.queue.lock();
        let tx = queue.pop()?;
        self.transactions.remove(&tx.hash);
        self.current_size.fetch_sub(1, AtomicOrdering::SeqCst);

        log::debug!("Popped transaction {} from mempool", tx.hash);
        Some(tx)
    }

    /// Get multiple transactions from mempool
    pub fn pop_transactions(&self, count: usize) -> Vec<MempoolTransaction> {
        let mut transactions = Vec::with_capacity(count);
        for _ in 0..count {
            if let Some(tx) = self.pop_transaction() {
                transactions.push(tx);
            } else {
                break;
            }
        }
        transactions
    }

    /// Get transaction by hash
    pub fn get_transaction(&self, hash: &Hash) -> Option<MempoolTransaction> {
        self.transactions.get(hash).map(|entry| entry.clone())
    }

    /// Remove transaction by hash
    pub fn remove_transaction(&self, hash: &Hash) -> Option<MempoolTransaction> {
        let tx = self.transactions.remove(hash)?;
        self.current_size.fetch_sub(1, AtomicOrdering::SeqCst);

        // Rebuild queue without this transaction
        let mut queue = self.queue.lock();
        *queue = queue
            .drain()
            .filter(|t| t.hash != *hash)
            .collect();

        Some(tx.1)
    }

    /// Get mempool size
    pub fn size(&self) -> usize {
        self.current_size.load(AtomicOrdering::Relaxed) as usize
    }

    /// Clear all transactions
    pub fn clear(&self) {
        self.transactions.clear();
        self.queue.lock().clear();
        self.current_size.store(0, AtomicOrdering::SeqCst);
        log::info!("Cleared mempool");
    }

    /// Get pending transactions by priority
    pub fn get_by_priority(&self, priority: Priority, limit: usize) -> Vec<MempoolTransaction> {
        self.transactions
            .iter()
            .filter(|entry| entry.value().priority == priority)
            .take(limit)
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get statistics
    pub fn get_stats(&self) -> MempoolStats {
        let size = self.size();
        let mut priority_counts = [0usize; 4];

        for entry in self.transactions.iter() {
            let index = entry.value().priority as usize;
            priority_counts[index] += 1;
        }

        MempoolStats {
            total_transactions: size,
            urgent: priority_counts[3],
            high: priority_counts[2],
            medium: priority_counts[1],
            low: priority_counts[0],
            capacity_used: (size as f64 / self.max_size as f64 * 100.0) as u8,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MempoolStats {
    pub total_transactions: usize,
    pub urgent: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
    pub capacity_used: u8,
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::signature::Keypair;
    use solana_sdk::system_instruction;

    fn create_test_transaction() -> Transaction {
        Transaction::default()
    }

    #[test]
    fn test_mempool_add_remove() {
        let mempool = Mempool::new(100);
        let tx = create_test_transaction();

        let hash = mempool.add_transaction(tx.clone(), Priority::Medium, 1000).unwrap();
        assert_eq!(mempool.size(), 1);

        let removed = mempool.remove_transaction(&hash);
        assert!(removed.is_some());
        assert_eq!(mempool.size(), 0);
    }

    #[test]
    fn test_mempool_priority_ordering() {
        let mempool = Mempool::new(100);

        let tx1 = create_test_transaction();
        let tx2 = create_test_transaction();

        mempool.add_transaction(tx1, Priority::Low, 100).unwrap();
        mempool.add_transaction(tx2, Priority::High, 100).unwrap();

        let popped = mempool.pop_transaction().unwrap();
        assert_eq!(popped.priority, Priority::High);
    }

    #[test]
    fn test_mempool_capacity() {
        let mempool = Mempool::new(2);

        let tx1 = create_test_transaction();
        let tx2 = create_test_transaction();

        mempool.add_transaction(tx1, Priority::Medium, 100).unwrap();
        mempool.add_transaction(tx2, Priority::Medium, 100).unwrap();

        // Should fail - mempool is full
        let tx3 = create_test_transaction();
        let result = mempool.add_transaction(tx3, Priority::Medium, 100);
        assert!(result.is_err());
    }
}
