use anyhow::Result;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::BinaryHeap;
use std::sync::Arc;
use parking_lot::Mutex;

use crate::hash_utils::Hash;
use crate::types::Transaction;

/// Optimized transaction pool with smart eviction and priority management
pub struct TransactionPool {
    pool: Arc<DashMap<Hash, PooledTransaction>>,
    priority_queue: Arc<Mutex<BinaryHeap<PooledTransaction>>>,
    nonce_tracker: Arc<DashMap<String, u64>>,
    max_pool_size: usize,
    max_per_account: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct PooledTransaction {
    pub tx_hash: Hash,
    pub tx: Transaction,
    pub priority_score: u64,
    pub added_at: u64,
    pub gas_price: u64,
}

impl Ord for PooledTransaction {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority_score.cmp(&other.priority_score)
    }
}

impl PartialOrd for PooledTransaction {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl TransactionPool {
    pub fn new(max_pool_size: usize, max_per_account: usize) -> Self {
        Self {
            pool: Arc::new(DashMap::new()),
            priority_queue: Arc::new(Mutex::new(BinaryHeap::new())),
            nonce_tracker: Arc::new(DashMap::new()),
            max_pool_size,
            max_per_account,
        }
    }

    /// Add transaction with optimizations
    pub fn add(&self, tx: Transaction) -> Result<Hash> {
        let tx_hash = Hash::new(&bincode::serialize(&tx).unwrap_or_default());

        // Check pool capacity
        if self.pool.len() >= self.max_pool_size {
            self.evict_lowest_priority()?;
        }

        // Check per-account limit
        let account_count = self.pool
            .iter()
            .filter(|e| e.value().tx.from == tx.from)
            .count();

        if account_count >= self.max_per_account {
            return Err(anyhow::anyhow!("Account transaction limit exceeded"));
        }

        // Calculate priority score
        let priority_score = self.calculate_priority(&tx);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let pooled = PooledTransaction {
            tx_hash,
            tx: tx.clone(),
            priority_score,
            added_at: now,
            gas_price: tx.max_fee_per_gas,
        };

        self.pool.insert(tx_hash, pooled.clone());
        self.priority_queue.lock().push(pooled);
        self.nonce_tracker.insert(tx.from.clone(), tx.nonce);

        Ok(tx_hash)
    }

    /// Calculate dynamic priority score
    fn calculate_priority(&self, tx: &Transaction) -> u64 {
        let base_priority = tx.max_fee_per_gas * 1000;
        let size_penalty = (tx.data.len() as u64) / 10;
        let nonce_bonus = if self.is_next_nonce(&tx.from, tx.nonce) {
            10000
        } else {
            0
        };

        base_priority + nonce_bonus - size_penalty
    }

    /// Check if transaction has the next expected nonce
    fn is_next_nonce(&self, address: &str, nonce: u64) -> bool {
        self.nonce_tracker
            .get(address)
            .map(|n| *n + 1 == nonce)
            .unwrap_or(nonce == 0)
    }

    /// Evict lowest priority transaction
    fn evict_lowest_priority(&self) -> Result<()> {
        let to_remove = self.pool
            .iter()
            .min_by_key(|e| e.value().priority_score)
            .map(|e| *e.key());

        if let Some(hash) = to_remove {
            self.pool.remove(&hash);
            log::debug!("Evicted transaction {:?} (low priority)", hash);
        }

        Ok(())
    }

    /// Get top N transactions
    pub fn pop_top(&self, n: usize) -> Vec<Transaction> {
        let mut queue = self.priority_queue.lock();
        let mut result = Vec::new();

        for _ in 0..n.min(queue.len()) {
            if let Some(pooled) = queue.pop() {
                if self.pool.contains_key(&pooled.tx_hash) {
                    result.push(pooled.tx);
                    self.pool.remove(&pooled.tx_hash);
                }
            }
        }

        result
    }

    /// Get pool size
    pub fn size(&self) -> usize {
        self.pool.len()
    }

    /// Get statistics
    pub fn get_stats(&self) -> PoolStats {
        let txs: Vec<_> = self.pool.iter().map(|e| e.value().clone()).collect();

        PoolStats {
            total_transactions: txs.len(),
            avg_gas_price: if !txs.is_empty() {
                txs.iter().map(|t| t.gas_price).sum::<u64>() / txs.len() as u64
            } else {
                0
            },
            max_gas_price: txs.iter().map(|t| t.gas_price).max().unwrap_or(0),
            min_gas_price: txs.iter().map(|t| t.gas_price).min().unwrap_or(0),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolStats {
    pub total_transactions: usize,
    pub avg_gas_price: u64,
    pub max_gas_price: u64,
    pub min_gas_price: u64,
}

/// Priority fee suggestion engine
pub struct FeeSuggestionEngine {
    recent_blocks: Arc<Mutex<Vec<BlockFeeData>>>,
    max_history: usize,
}

#[derive(Debug, Clone)]
struct BlockFeeData {
    base_fee: u64,
    priority_fees: Vec<u64>,
    timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeSuggestion {
    pub slow: u64,
    pub standard: u64,
    pub fast: u64,
    pub instant: u64,
    pub base_fee: u64,
}

impl FeeSuggestionEngine {
    pub fn new(max_history: usize) -> Self {
        Self {
            recent_blocks: Arc::new(Mutex::new(Vec::new())),
            max_history,
        }
    }

    /// Record block fee data
    pub fn record_block(&self, base_fee: u64, priority_fees: Vec<u64>) {
        let mut blocks = self.recent_blocks.lock();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        blocks.push(BlockFeeData {
            base_fee,
            priority_fees,
            timestamp: now,
        });

        if blocks.len() > self.max_history {
            blocks.remove(0);
        }
    }

    /// Get fee suggestions
    pub fn suggest_fees(&self) -> FeeSuggestion {
        let blocks = self.recent_blocks.lock();

        if blocks.is_empty() {
            return FeeSuggestion {
                slow: 1,
                standard: 2,
                fast: 5,
                instant: 10,
                base_fee: 1,
            };
        }

        // Get latest base fee
        let base_fee = blocks.last().map(|b| b.base_fee).unwrap_or(1);

        // Collect all priority fees
        let mut all_fees: Vec<u64> = blocks
            .iter()
            .flat_map(|b| b.priority_fees.clone())
            .collect();

        all_fees.sort();

        let len = all_fees.len();
        if len == 0 {
            return FeeSuggestion {
                slow: base_fee,
                standard: base_fee * 2,
                fast: base_fee * 5,
                instant: base_fee * 10,
                base_fee,
            };
        }

        // Calculate percentiles
        let p10 = all_fees[len / 10];
        let p50 = all_fees[len / 2];
        let p75 = all_fees[len * 3 / 4];
        let p90 = all_fees[len * 9 / 10];

        FeeSuggestion {
            slow: base_fee + p10,
            standard: base_fee + p50,
            fast: base_fee + p75,
            instant: base_fee + p90,
            base_fee,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_pool() {
        let pool = TransactionPool::new(100, 10);

        let tx = Transaction {
            from: "addr1".to_string(),
            to: Some("addr2".to_string()),
            value: 100,
            data: vec![],
            nonce: 0,
            gas_limit: 21000,
            max_fee_per_gas: 10,
        };

        pool.add(tx).unwrap();

        assert_eq!(pool.size(), 1);
    }

    #[test]
    fn test_fee_suggestion() {
        let engine = FeeSuggestionEngine::new(10);

        engine.record_block(1000, vec![100, 200, 300, 400, 500]);
        engine.record_block(1100, vec![150, 250, 350, 450, 550]);

        let suggestion = engine.suggest_fees();

        assert!(suggestion.slow > 0);
        assert!(suggestion.standard > suggestion.slow);
        assert!(suggestion.fast > suggestion.standard);
    }
}
