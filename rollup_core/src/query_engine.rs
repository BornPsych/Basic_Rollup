use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::hash_utils::Hash;

/// Historical query engine with indexing
pub struct QueryEngine {
    // Account index: address -> account data
    account_index: Arc<DashMap<String, AccountRecord>>,

    // Transaction index: hash -> transaction data
    tx_index: Arc<DashMap<Hash, TransactionRecord>>,

    // Block/batch index: batch_id -> batch data
    batch_index: Arc<DashMap<u64, BatchRecord>>,

    // Account history: address -> list of transactions
    account_history: Arc<DashMap<String, Vec<Hash>>>,

    // Balance tracker
    balances: Arc<DashMap<String, u64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountRecord {
    pub address: String,
    pub balance: u64,
    pub nonce: u64,
    pub created_at: u64,
    pub last_updated: u64,
    pub tx_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionRecord {
    pub hash: Hash,
    pub from: String,
    pub to: Option<String>,
    pub value: u64,
    pub gas_used: u64,
    pub fee: u64,
    pub status: TransactionStatus,
    pub batch_id: u64,
    pub timestamp: u64,
    pub logs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransactionStatus {
    Pending,
    Success,
    Failed,
    Reverted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchRecord {
    pub batch_id: u64,
    pub tx_count: usize,
    pub state_root: Hash,
    pub timestamp: u64,
    pub gas_used: u64,
    pub fees_collected: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionReceipt {
    pub tx_hash: Hash,
    pub status: TransactionStatus,
    pub batch_id: u64,
    pub gas_used: u64,
    pub fee_paid: u64,
    pub logs: Vec<EventLog>,
    pub contract_address: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventLog {
    pub address: String,
    pub topics: Vec<Hash>,
    pub data: Vec<u8>,
}

impl QueryEngine {
    pub fn new() -> Self {
        Self {
            account_index: Arc::new(DashMap::new()),
            tx_index: Arc::new(DashMap::new()),
            batch_index: Arc::new(DashMap::new()),
            account_history: Arc::new(DashMap::new()),
            balances: Arc::new(DashMap::new()),
        }
    }

    // ============ Account Operations ============

    /// Get account by address
    pub fn get_account(&self, address: &str) -> Option<AccountRecord> {
        self.account_index.get(address).map(|r| r.clone())
    }

    /// Update account
    pub fn update_account(&self, record: AccountRecord) {
        self.account_index.insert(record.address.clone(), record);
    }

    /// Get account balance
    pub fn get_balance(&self, address: &str) -> u64 {
        self.balances.get(address).map(|b| *b).unwrap_or(0)
    }

    /// Update balance
    pub fn update_balance(&self, address: &str, balance: u64) {
        self.balances.insert(address.to_string(), balance);
    }

    /// Get account history
    pub fn get_account_history(&self, address: &str, limit: usize) -> Vec<Hash> {
        self.account_history
            .get(address)
            .map(|history| {
                let len = history.len();
                if len > limit {
                    history[len - limit..].to_vec()
                } else {
                    history.clone()
                }
            })
            .unwrap_or_default()
    }

    // ============ Transaction Operations ============

    /// Index transaction
    pub fn index_transaction(&self, record: TransactionRecord) {
        let hash = record.hash;
        let from = record.from.clone();
        let to = record.to.clone();

        // Add to transaction index
        self.tx_index.insert(hash, record);

        // Add to account history
        self.account_history
            .entry(from.clone())
            .or_insert_with(Vec::new)
            .push(hash);

        if let Some(to_addr) = to {
            self.account_history
                .entry(to_addr)
                .or_insert_with(Vec::new)
                .push(hash);
        }

        // Update account tx counts
        if let Some(mut acc) = self.account_index.get_mut(&from) {
            acc.tx_count += 1;
        }
    }

    /// Get transaction by hash
    pub fn get_transaction(&self, hash: &Hash) -> Option<TransactionRecord> {
        self.tx_index.get(hash).map(|r| r.clone())
    }

    /// Get transaction receipt
    pub fn get_receipt(&self, hash: &Hash) -> Option<TransactionReceipt> {
        self.get_transaction(hash).map(|tx| TransactionReceipt {
            tx_hash: tx.hash,
            status: tx.status,
            batch_id: tx.batch_id,
            gas_used: tx.gas_used,
            fee_paid: tx.fee,
            logs: vec![], // Would be populated from actual logs
            contract_address: None,
        })
    }

    // ============ Batch Operations ============

    /// Index batch
    pub fn index_batch(&self, record: BatchRecord) {
        self.batch_index.insert(record.batch_id, record);
    }

    /// Get batch by ID
    pub fn get_batch(&self, batch_id: u64) -> Option<BatchRecord> {
        self.batch_index.get(&batch_id).map(|r| r.clone())
    }

    /// Get latest batches
    pub fn get_latest_batches(&self, limit: usize) -> Vec<BatchRecord> {
        let mut batches: Vec<_> = self.batch_index
            .iter()
            .map(|entry| entry.value().clone())
            .collect();

        batches.sort_by(|a, b| b.batch_id.cmp(&a.batch_id));
        batches.truncate(limit);
        batches
    }

    // ============ Query Operations ============

    /// Query transactions by filter
    pub fn query_transactions(&self, filter: TransactionFilter) -> Vec<TransactionRecord> {
        self.tx_index
            .iter()
            .filter(|entry| filter.matches(entry.value()))
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Search accounts
    pub fn search_accounts(&self, query: &str) -> Vec<AccountRecord> {
        self.account_index
            .iter()
            .filter(|entry| entry.key().contains(query))
            .map(|entry| entry.value().clone())
            .take(100)
            .collect()
    }

    /// Get statistics
    pub fn get_stats(&self) -> QueryEngineStats {
        QueryEngineStats {
            total_accounts: self.account_index.len(),
            total_transactions: self.tx_index.len(),
            total_batches: self.batch_index.len(),
        }
    }
}

impl Default for QueryEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionFilter {
    pub from: Option<String>,
    pub to: Option<String>,
    pub min_value: Option<u64>,
    pub max_value: Option<u64>,
    pub status: Option<TransactionStatus>,
    pub min_timestamp: Option<u64>,
    pub max_timestamp: Option<u64>,
}

impl TransactionFilter {
    pub fn matches(&self, tx: &TransactionRecord) -> bool {
        if let Some(ref from) = self.from {
            if &tx.from != from {
                return false;
            }
        }

        if let Some(ref to) = self.to {
            if tx.to.as_ref() != Some(to) {
                return false;
            }
        }

        if let Some(min) = self.min_value {
            if tx.value < min {
                return false;
            }
        }

        if let Some(max) = self.max_value {
            if tx.value > max {
                return false;
            }
        }

        true
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryEngineStats {
    pub total_accounts: usize,
    pub total_transactions: usize,
    pub total_batches: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_account_operations() {
        let engine = QueryEngine::new();

        let record = AccountRecord {
            address: "addr1".to_string(),
            balance: 1000,
            nonce: 0,
            created_at: 0,
            last_updated: 0,
            tx_count: 0,
        };

        engine.update_account(record.clone());
        let retrieved = engine.get_account("addr1").unwrap();
        assert_eq!(retrieved.balance, 1000);
    }

    #[test]
    fn test_balance_tracking() {
        let engine = QueryEngine::new();

        engine.update_balance("addr1", 5000);
        assert_eq!(engine.get_balance("addr1"), 5000);

        engine.update_balance("addr1", 3000);
        assert_eq!(engine.get_balance("addr1"), 3000);
    }

    #[test]
    fn test_transaction_indexing() {
        let engine = QueryEngine::new();

        let tx = TransactionRecord {
            hash: Hash::new(b"tx1"),
            from: "addr1".to_string(),
            to: Some("addr2".to_string()),
            value: 100,
            gas_used: 21000,
            fee: 100,
            status: TransactionStatus::Success,
            batch_id: 1,
            timestamp: 0,
            logs: vec![],
        };

        engine.index_transaction(tx.clone());
        let retrieved = engine.get_transaction(&tx.hash).unwrap();
        assert_eq!(retrieved.value, 100);

        // Check history
        let history = engine.get_account_history("addr1", 10);
        assert_eq!(history.len(), 1);
    }
}
