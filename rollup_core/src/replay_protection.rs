use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::hash_utils::Hash;

/// Nonce manager for replay protection
pub struct NonceManager {
    /// Account nonces
    nonces: Arc<DashMap<String, u64>>,
    /// Transaction history (hash -> timestamp)
    tx_history: Arc<DashMap<Hash, u64>>,
    /// History cleanup threshold (keep last N transactions)
    max_history: usize,
    /// Total transactions processed
    total_txs: AtomicU64,
}

impl NonceManager {
    pub fn new(max_history: usize) -> Self {
        Self {
            nonces: Arc::new(DashMap::new()),
            tx_history: Arc::new(DashMap::new()),
            max_history,
            total_txs: AtomicU64::new(0),
        }
    }

    /// Get current nonce for account
    pub fn get_nonce(&self, account: &str) -> u64 {
        self.nonces.get(account).map(|n| *n).unwrap_or(0)
    }

    /// Increment nonce for account
    pub fn increment_nonce(&self, account: &str) -> u64 {
        let mut entry = self.nonces.entry(account.to_string()).or_insert(0);
        *entry += 1;
        *entry
    }

    /// Verify nonce is correct (should be current_nonce + 1)
    pub fn verify_nonce(&self, account: &str, nonce: u64) -> bool {
        let current = self.get_nonce(account);
        nonce == current + 1
    }

    /// Check if transaction was already processed
    pub fn is_duplicate(&self, tx_hash: &Hash) -> bool {
        self.tx_history.contains_key(tx_hash)
    }

    /// Record transaction
    pub fn record_transaction(&self, account: &str, tx_hash: Hash, nonce: u64) -> Result<(), String> {
        // Check duplicate
        if self.is_duplicate(&tx_hash) {
            return Err("Transaction already processed".to_string());
        }

        // Verify nonce
        if !self.verify_nonce(account, nonce) {
            return Err(format!(
                "Invalid nonce. Expected {}, got {}",
                self.get_nonce(account) + 1,
                nonce
            ));
        }

        // Record transaction
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        self.tx_history.insert(tx_hash, timestamp);
        self.increment_nonce(account);
        self.total_txs.fetch_add(1, Ordering::Relaxed);

        // Cleanup old history if needed
        if self.tx_history.len() > self.max_history {
            self.cleanup_old_history();
        }

        Ok(())
    }

    /// Cleanup old transaction history
    fn cleanup_old_history(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Remove transactions older than 1 hour
        let cutoff = now - 3600;

        let to_remove: Vec<_> = self.tx_history
            .iter()
            .filter(|entry| *entry.value() < cutoff)
            .map(|entry| *entry.key())
            .collect();

        for hash in to_remove {
            self.tx_history.remove(&hash);
        }
    }

    /// Get statistics
    pub fn get_stats(&self) -> NonceStats {
        NonceStats {
            total_accounts: self.nonces.len(),
            total_transactions: self.total_txs.load(Ordering::Relaxed),
            history_size: self.tx_history.len(),
        }
    }

    /// Reset nonce for account (admin function)
    pub fn reset_nonce(&self, account: &str) {
        self.nonces.remove(account);
    }
}

impl Default for NonceManager {
    fn default() -> Self {
        Self::new(100_000) // Keep last 100k transactions
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NonceStats {
    pub total_accounts: usize,
    pub total_transactions: u64,
    pub history_size: usize,
}

/// Transaction with nonce
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NonceTransaction {
    pub from: String,
    pub nonce: u64,
    pub data: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nonce_increment() {
        let manager = NonceManager::default();

        assert_eq!(manager.get_nonce("account1"), 0);
        manager.increment_nonce("account1");
        assert_eq!(manager.get_nonce("account1"), 1);
    }

    #[test]
    fn test_nonce_verification() {
        let manager = NonceManager::default();

        assert!(manager.verify_nonce("account1", 1)); // First nonce should be 1
        assert!(!manager.verify_nonce("account1", 2)); // Can't skip
    }

    #[test]
    fn test_duplicate_detection() {
        let manager = NonceManager::default();
        let tx_hash = Hash::new(b"test_tx");

        let result1 = manager.record_transaction("account1", tx_hash, 1);
        assert!(result1.is_ok());

        let result2 = manager.record_transaction("account1", tx_hash, 2);
        assert!(result2.is_err());
    }

    #[test]
    fn test_invalid_nonce() {
        let manager = NonceManager::default();
        let tx_hash = Hash::new(b"test_tx");

        // Try to use nonce 5 when current is 0
        let result = manager.record_transaction("account1", tx_hash, 5);
        assert!(result.is_err());
    }
}
