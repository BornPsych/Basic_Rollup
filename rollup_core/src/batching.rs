use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::hash_utils::Hash;
use crate::types::Transaction;

/// Advanced batching strategies for optimal batch creation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatchingStrategy {
    /// Fixed size batches
    FixedSize { size: usize },

    /// Time-based batching
    TimeBased { interval_ms: u64 },

    /// Adaptive batching based on network conditions
    Adaptive {
        min_size: usize,
        max_size: usize,
        max_wait_ms: u64,
    },

    /// Gas-based batching
    GasBased { target_gas: u64 },

    /// Hybrid strategy combining multiple factors
    Hybrid {
        min_size: usize,
        max_size: usize,
        max_wait_ms: u64,
        target_gas: u64,
    },
}

/// Batch builder with advanced strategies
pub struct BatchBuilder {
    strategy: BatchingStrategy,
    pending_transactions: VecDeque<Transaction>,
    current_batch: Vec<Transaction>,
    batch_start_time: Option<Instant>,
    total_batches_created: u64,
    stats: BatchingStats,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BatchingStats {
    pub total_batches: u64,
    pub total_transactions: u64,
    pub avg_batch_size: f64,
    pub avg_batch_time_ms: f64,
    pub avg_gas_per_batch: u64,
    pub min_batch_size: usize,
    pub max_batch_size: usize,
}

impl BatchBuilder {
    pub fn new(strategy: BatchingStrategy) -> Self {
        Self {
            strategy,
            pending_transactions: VecDeque::new(),
            current_batch: Vec::new(),
            batch_start_time: None,
            total_batches_created: 0,
            stats: BatchingStats::default(),
        }
    }

    /// Add transaction to pending pool
    pub fn add_transaction(&mut self, tx: Transaction) {
        self.pending_transactions.push_back(tx);

        if self.batch_start_time.is_none() {
            self.batch_start_time = Some(Instant::now());
        }
    }

    /// Add multiple transactions
    pub fn add_transactions(&mut self, txs: Vec<Transaction>) {
        for tx in txs {
            self.add_transaction(tx);
        }
    }

    /// Check if batch should be created based on strategy
    pub fn should_create_batch(&self) -> bool {
        if self.pending_transactions.is_empty() {
            return false;
        }

        match self.strategy {
            BatchingStrategy::FixedSize { size } => self.pending_transactions.len() >= size,

            BatchingStrategy::TimeBased { interval_ms } => {
                if let Some(start) = self.batch_start_time {
                    start.elapsed() >= Duration::from_millis(interval_ms)
                } else {
                    false
                }
            }

            BatchingStrategy::Adaptive {
                min_size,
                max_size,
                max_wait_ms,
            } => {
                let size = self.pending_transactions.len();
                let elapsed = self
                    .batch_start_time
                    .map(|t| t.elapsed())
                    .unwrap_or(Duration::ZERO);

                // Create batch if:
                // 1. Reached max size
                // 2. Reached min size and max wait time
                size >= max_size || (size >= min_size && elapsed >= Duration::from_millis(max_wait_ms))
            }

            BatchingStrategy::GasBased { target_gas } => {
                let total_gas: u64 = self.pending_transactions.iter().map(|tx| tx.gas_limit).sum();
                total_gas >= target_gas
            }

            BatchingStrategy::Hybrid {
                min_size,
                max_size,
                max_wait_ms,
                target_gas,
            } => {
                let size = self.pending_transactions.len();
                let total_gas: u64 = self.pending_transactions.iter().map(|tx| tx.gas_limit).sum();
                let elapsed = self
                    .batch_start_time
                    .map(|t| t.elapsed())
                    .unwrap_or(Duration::ZERO);

                // Create batch if any condition is met:
                size >= max_size
                    || total_gas >= target_gas
                    || (size >= min_size && elapsed >= Duration::from_millis(max_wait_ms))
            }
        }
    }

    /// Create batch from pending transactions
    pub fn create_batch(&mut self) -> Option<Batch> {
        if self.pending_transactions.is_empty() {
            return None;
        }

        let mut transactions = Vec::new();
        let batch_size = self.get_batch_size();

        for _ in 0..batch_size.min(self.pending_transactions.len()) {
            if let Some(tx) = self.pending_transactions.pop_front() {
                transactions.push(tx);
            }
        }

        if transactions.is_empty() {
            return None;
        }

        let batch_time = self
            .batch_start_time
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0);

        let batch = Batch {
            batch_id: self.total_batches_created,
            transactions: transactions.clone(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            strategy: self.strategy,
            total_gas: transactions.iter().map(|tx| tx.gas_limit).sum(),
            batch_time_ms: batch_time,
        };

        // Update stats
        self.update_stats(&batch);

        self.total_batches_created += 1;
        self.batch_start_time = if self.pending_transactions.is_empty() {
            None
        } else {
            Some(Instant::now())
        };

        log::info!(
            "Created batch {} with {} transactions ({} gas, {} ms)",
            batch.batch_id,
            batch.transactions.len(),
            batch.total_gas,
            batch_time
        );

        Some(batch)
    }

    /// Get batch size based on strategy
    fn get_batch_size(&self) -> usize {
        match self.strategy {
            BatchingStrategy::FixedSize { size } => size,
            BatchingStrategy::TimeBased { .. } => self.pending_transactions.len(),
            BatchingStrategy::Adaptive { max_size, .. } => max_size.min(self.pending_transactions.len()),
            BatchingStrategy::GasBased { target_gas } => {
                let mut size = 0;
                let mut total_gas = 0u64;

                for tx in &self.pending_transactions {
                    if total_gas + tx.gas_limit > target_gas && size > 0 {
                        break;
                    }
                    total_gas += tx.gas_limit;
                    size += 1;
                }

                size
            }
            BatchingStrategy::Hybrid { max_size, .. } => max_size.min(self.pending_transactions.len()),
        }
    }

    /// Update batching statistics
    fn update_stats(&mut self, batch: &Batch) {
        let batch_size = batch.transactions.len();

        self.stats.total_batches += 1;
        self.stats.total_transactions += batch_size as u64;

        // Update average batch size
        self.stats.avg_batch_size = self.stats.total_transactions as f64 / self.stats.total_batches as f64;

        // Update average batch time
        self.stats.avg_batch_time_ms = (self.stats.avg_batch_time_ms * (self.stats.total_batches - 1) as f64
            + batch.batch_time_ms as f64)
            / self.stats.total_batches as f64;

        // Update min/max batch size
        if self.stats.min_batch_size == 0 || batch_size < self.stats.min_batch_size {
            self.stats.min_batch_size = batch_size;
        }
        if batch_size > self.stats.max_batch_size {
            self.stats.max_batch_size = batch_size;
        }

        // Update average gas
        self.stats.avg_gas_per_batch = (self.stats.avg_gas_per_batch * (self.stats.total_batches - 1)
            + batch.total_gas)
            / self.stats.total_batches;
    }

    /// Get batching statistics
    pub fn get_stats(&self) -> BatchingStats {
        self.stats.clone()
    }

    /// Get pending transaction count
    pub fn pending_count(&self) -> usize {
        self.pending_transactions.len()
    }

    /// Clear all pending transactions
    pub fn clear(&mut self) {
        self.pending_transactions.clear();
        self.current_batch.clear();
        self.batch_start_time = None;
    }
}

/// A batch of transactions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Batch {
    pub batch_id: u64,
    pub transactions: Vec<Transaction>,
    pub created_at: u64,
    pub strategy: BatchingStrategy,
    pub total_gas: u64,
    pub batch_time_ms: u64,
}

impl Batch {
    /// Calculate batch hash
    pub fn hash(&self) -> Hash {
        let serialized = bincode::serialize(self).unwrap_or_default();
        Hash::new(&serialized)
    }

    /// Get batch size
    pub fn size(&self) -> usize {
        self.transactions.len()
    }

    /// Get total value transferred in batch
    pub fn total_value(&self) -> u64 {
        self.transactions.iter().map(|tx| tx.value).sum()
    }
}

/// Batch optimizer that suggests optimal batching strategy
pub struct BatchOptimizer {
    network_load: f64, // 0.0 to 1.0
    avg_tx_rate: f64,  // transactions per second
}

impl BatchOptimizer {
    pub fn new() -> Self {
        Self {
            network_load: 0.5,
            avg_tx_rate: 10.0,
        }
    }

    /// Update network conditions
    pub fn update_conditions(&mut self, load: f64, tx_rate: f64) {
        self.network_load = load.clamp(0.0, 1.0);
        self.avg_tx_rate = tx_rate.max(0.0);
    }

    /// Suggest optimal batching strategy based on network conditions
    pub fn suggest_strategy(&self) -> BatchingStrategy {
        // High load: use larger batches with shorter wait times
        if self.network_load > 0.7 {
            BatchingStrategy::Hybrid {
                min_size: 100,
                max_size: 500,
                max_wait_ms: 1000,
                target_gas: 10_000_000,
            }
        }
        // Medium load: balanced approach
        else if self.network_load > 0.3 {
            BatchingStrategy::Hybrid {
                min_size: 50,
                max_size: 200,
                max_wait_ms: 2000,
                target_gas: 5_000_000,
            }
        }
        // Low load: smaller batches, longer wait for efficiency
        else {
            BatchingStrategy::Hybrid {
                min_size: 20,
                max_size: 100,
                max_wait_ms: 5000,
                target_gas: 2_000_000,
            }
        }
    }
}

impl Default for BatchOptimizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_tx(value: u64) -> Transaction {
        Transaction {
            from: "addr1".to_string(),
            to: Some("addr2".to_string()),
            value,
            data: vec![],
            nonce: 0,
            gas_limit: 21000,
            max_fee_per_gas: 1,
        }
    }

    #[test]
    fn test_fixed_size_batching() {
        let mut builder = BatchBuilder::new(BatchingStrategy::FixedSize { size: 10 });

        for i in 0..15 {
            builder.add_transaction(create_test_tx(i));
        }

        assert!(builder.should_create_batch());

        let batch = builder.create_batch().unwrap();
        assert_eq!(batch.transactions.len(), 10);
        assert_eq!(builder.pending_count(), 5);
    }

    #[test]
    fn test_gas_based_batching() {
        let mut builder = BatchBuilder::new(BatchingStrategy::GasBased {
            target_gas: 100_000,
        });

        // Each tx has 21000 gas, so 5 txs = 105000 gas
        for i in 0..5 {
            builder.add_transaction(create_test_tx(i));
        }

        assert!(builder.should_create_batch());

        let batch = builder.create_batch().unwrap();
        assert!(batch.total_gas >= 100_000);
    }

    #[test]
    fn test_batch_optimizer() {
        let mut optimizer = BatchOptimizer::new();

        // High load
        optimizer.update_conditions(0.9, 100.0);
        let strategy = optimizer.suggest_strategy();

        match strategy {
            BatchingStrategy::Hybrid { max_size, .. } => {
                assert!(max_size >= 200);
            }
            _ => panic!("Expected Hybrid strategy"),
        }

        // Low load
        optimizer.update_conditions(0.1, 5.0);
        let strategy = optimizer.suggest_strategy();

        match strategy {
            BatchingStrategy::Hybrid { max_size, .. } => {
                assert!(max_size <= 200);
            }
            _ => panic!("Expected Hybrid strategy"),
        }
    }

    #[test]
    fn test_batching_stats() {
        let mut builder = BatchBuilder::new(BatchingStrategy::FixedSize { size: 5 });

        for i in 0..10 {
            builder.add_transaction(create_test_tx(i));
        }

        builder.create_batch();
        builder.create_batch();

        let stats = builder.get_stats();
        assert_eq!(stats.total_batches, 2);
        assert_eq!(stats.total_transactions, 10);
        assert_eq!(stats.avg_batch_size, 5.0);
    }
}
