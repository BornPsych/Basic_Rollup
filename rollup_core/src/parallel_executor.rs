use anyhow::{anyhow, Result};
use dashmap::DashMap;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::hash_utils::Hash;
use crate::types::Transaction;

/// Parallel transaction executor with dependency analysis
pub struct ParallelExecutor {
    max_threads: usize,
    execution_stats: Arc<DashMap<u64, ExecutionBatch>>,
    batch_counter: AtomicU64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionBatch {
    pub batch_id: u64,
    pub total_transactions: usize,
    pub parallel_groups: usize,
    pub execution_time_ms: u64,
    pub speedup_factor: f64,
}

#[derive(Debug, Clone)]
pub struct TransactionWithDeps {
    pub tx: Transaction,
    pub tx_hash: Hash,
    pub reads: HashSet<String>,
    pub writes: HashSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub tx_hash: Hash,
    pub success: bool,
    pub gas_used: u64,
    pub error: Option<String>,
    pub state_changes: HashMap<String, Vec<u8>>,
}

impl ParallelExecutor {
    pub fn new(max_threads: usize) -> Self {
        Self {
            max_threads,
            execution_stats: Arc::new(DashMap::new()),
            batch_counter: AtomicU64::new(0),
        }
    }

    /// Analyze transaction dependencies (read/write sets)
    pub fn analyze_dependencies(&self, tx: &Transaction) -> (HashSet<String>, HashSet<String>) {
        let mut reads = HashSet::new();
        let mut writes = HashSet::new();

        // Sender always writes (nonce + balance update)
        writes.insert(tx.from.clone());
        reads.insert(tx.from.clone());

        // Receiver writes (balance update)
        if let Some(ref to) = tx.to {
            writes.insert(to.clone());
        }

        // Add contract storage dependencies if applicable
        // This is simplified - in reality would analyze the transaction data

        (reads, writes)
    }

    /// Build dependency graph and identify independent transaction groups
    pub fn build_execution_groups(
        &self,
        transactions: Vec<TransactionWithDeps>,
    ) -> Vec<Vec<TransactionWithDeps>> {
        let mut groups: Vec<Vec<TransactionWithDeps>> = Vec::new();
        let mut remaining = transactions;

        while !remaining.is_empty() {
            let mut current_group = Vec::new();
            let mut group_writes = HashSet::new();
            let mut group_reads = HashSet::new();
            let mut i = 0;

            while i < remaining.len() {
                let tx = &remaining[i];

                // Check if this transaction conflicts with the current group
                let has_conflict = tx.writes.iter().any(|w| {
                    group_writes.contains(w) || group_reads.contains(w)
                }) || tx.reads.iter().any(|r| group_writes.contains(r));

                if !has_conflict {
                    // No conflict - add to current group
                    for write in &tx.writes {
                        group_writes.insert(write.clone());
                    }
                    for read in &tx.reads {
                        group_reads.insert(read.clone());
                    }

                    current_group.push(remaining.remove(i));
                } else {
                    i += 1;
                }
            }

            if !current_group.is_empty() {
                groups.push(current_group);
            } else if !remaining.is_empty() {
                // Deadlock prevention - take first transaction
                groups.push(vec![remaining.remove(0)]);
            }
        }

        log::info!(
            "Built {} parallel execution groups from {} transactions",
            groups.len(),
            groups.iter().map(|g| g.len()).sum::<usize>()
        );

        groups
    }

    /// Execute transactions in parallel groups
    pub fn execute_parallel(
        &self,
        transactions: Vec<Transaction>,
        state: Arc<DashMap<String, Vec<u8>>>,
    ) -> Result<Vec<ExecutionResult>> {
        let start = Instant::now();
        let total_txs = transactions.len();

        // Analyze dependencies
        let tx_with_deps: Vec<TransactionWithDeps> = transactions
            .into_iter()
            .map(|tx| {
                let tx_hash = Hash::new(&bincode::serialize(&tx).unwrap_or_default());
                let (reads, writes) = self.analyze_dependencies(&tx);
                TransactionWithDeps {
                    tx,
                    tx_hash,
                    reads,
                    writes,
                }
            })
            .collect();

        // Build execution groups
        let groups = self.build_execution_groups(tx_with_deps);
        let group_count = groups.len();

        // Execute each group in parallel
        let mut all_results = Vec::new();

        for group in groups {
            // Execute transactions in this group in parallel using rayon
            let group_results: Vec<ExecutionResult> = group
                .par_iter()
                .map(|tx_deps| self.execute_transaction(&tx_deps.tx, tx_deps.tx_hash, &state))
                .collect();

            all_results.extend(group_results);
        }

        let execution_time = start.elapsed().as_millis() as u64;

        // Calculate speedup (estimated)
        let sequential_time_estimate = total_txs as u64 * 10; // Assume 10ms per tx sequentially
        let speedup = sequential_time_estimate as f64 / execution_time.max(1) as f64;

        // Store stats
        let batch_id = self.batch_counter.fetch_add(1, Ordering::SeqCst);
        self.execution_stats.insert(
            batch_id,
            ExecutionBatch {
                batch_id,
                total_transactions: total_txs,
                parallel_groups: group_count,
                execution_time_ms: execution_time,
                speedup_factor: speedup,
            },
        );

        log::info!(
            "Executed {} transactions in {} groups ({} ms, {:.2}x speedup)",
            total_txs,
            group_count,
            execution_time,
            speedup
        );

        Ok(all_results)
    }

    /// Execute a single transaction
    fn execute_transaction(
        &self,
        tx: &Transaction,
        tx_hash: Hash,
        state: &Arc<DashMap<String, Vec<u8>>>,
    ) -> ExecutionResult {
        // Simplified execution logic
        let mut state_changes = HashMap::new();

        // Validate sender balance (simplified)
        let sender_data = state
            .get(&tx.from)
            .map(|d| d.clone())
            .unwrap_or_else(|| vec![0u8; 8]);

        let sender_balance = u64::from_le_bytes(sender_data[..8].try_into().unwrap_or([0u8; 8]));

        if sender_balance < tx.value {
            return ExecutionResult {
                tx_hash,
                success: false,
                gas_used: 21000,
                error: Some("Insufficient balance".to_string()),
                state_changes: HashMap::new(),
            };
        }

        // Update sender balance
        let new_sender_balance = sender_balance - tx.value;
        state_changes.insert(tx.from.clone(), new_sender_balance.to_le_bytes().to_vec());

        // Update receiver balance
        if let Some(ref to) = tx.to {
            let receiver_data = state
                .get(to)
                .map(|d| d.clone())
                .unwrap_or_else(|| vec![0u8; 8]);

            let receiver_balance =
                u64::from_le_bytes(receiver_data[..8].try_into().unwrap_or([0u8; 8]));
            let new_receiver_balance = receiver_balance + tx.value;

            state_changes.insert(to.clone(), new_receiver_balance.to_le_bytes().to_vec());
        }

        // Apply state changes
        for (key, value) in &state_changes {
            state.insert(key.clone(), value.clone());
        }

        ExecutionResult {
            tx_hash,
            success: true,
            gas_used: 21000,
            error: None,
            state_changes,
        }
    }

    /// Get execution statistics
    pub fn get_stats(&self) -> Vec<ExecutionBatch> {
        self.execution_stats
            .iter()
            .map(|e| e.value().clone())
            .collect()
    }

    /// Get average speedup factor
    pub fn get_average_speedup(&self) -> f64 {
        let stats: Vec<_> = self.execution_stats.iter().map(|e| e.value().clone()).collect();

        if stats.is_empty() {
            return 1.0;
        }

        stats.iter().map(|s| s.speedup_factor).sum::<f64>() / stats.len() as f64
    }
}

impl Default for ParallelExecutor {
    fn default() -> Self {
        Self::new(num_cpus::get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dependency_analysis() {
        let executor = ParallelExecutor::default();

        let tx = Transaction {
            from: "addr1".to_string(),
            to: Some("addr2".to_string()),
            value: 100,
            data: vec![],
            nonce: 0,
            gas_limit: 21000,
            max_fee_per_gas: 1,
        };

        let (reads, writes) = executor.analyze_dependencies(&tx);

        assert!(writes.contains("addr1"));
        assert!(writes.contains("addr2"));
        assert!(reads.contains("addr1"));
    }

    #[test]
    fn test_execution_groups() {
        let executor = ParallelExecutor::default();

        let txs = vec![
            TransactionWithDeps {
                tx: Transaction {
                    from: "addr1".to_string(),
                    to: Some("addr2".to_string()),
                    value: 100,
                    data: vec![],
                    nonce: 0,
                    gas_limit: 21000,
                    max_fee_per_gas: 1,
                },
                tx_hash: Hash::new(b"tx1"),
                reads: vec!["addr1".to_string()].into_iter().collect(),
                writes: vec!["addr1".to_string(), "addr2".to_string()]
                    .into_iter()
                    .collect(),
            },
            TransactionWithDeps {
                tx: Transaction {
                    from: "addr3".to_string(),
                    to: Some("addr4".to_string()),
                    value: 100,
                    data: vec![],
                    nonce: 0,
                    gas_limit: 21000,
                    max_fee_per_gas: 1,
                },
                tx_hash: Hash::new(b"tx2"),
                reads: vec!["addr3".to_string()].into_iter().collect(),
                writes: vec!["addr3".to_string(), "addr4".to_string()]
                    .into_iter()
                    .collect(),
            },
        ];

        let groups = executor.build_execution_groups(txs);

        // These two transactions should be in the same group (no conflicts)
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 2);
    }

    #[test]
    fn test_parallel_execution() {
        let executor = ParallelExecutor::default();
        let state = Arc::new(DashMap::new());

        // Initialize balances
        state.insert("addr1".to_string(), 1000u64.to_le_bytes().to_vec());
        state.insert("addr3".to_string(), 1000u64.to_le_bytes().to_vec());

        let txs = vec![
            Transaction {
                from: "addr1".to_string(),
                to: Some("addr2".to_string()),
                value: 100,
                data: vec![],
                nonce: 0,
                gas_limit: 21000,
                max_fee_per_gas: 1,
            },
            Transaction {
                from: "addr3".to_string(),
                to: Some("addr4".to_string()),
                value: 100,
                data: vec![],
                nonce: 0,
                gas_limit: 21000,
                max_fee_per_gas: 1,
            },
        ];

        let results = executor.execute_parallel(txs, state.clone()).unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.success));
    }
}
