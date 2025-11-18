use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use solana_sdk::{
    account::{AccountSharedData, ReadableAccount},
    keccak::{Hash, Hasher},
    pubkey::Pubkey,
    transaction::Transaction,
};
use std::collections::{HashMap, VecDeque};

use crate::merkle::MerkleTree;

/// Represents a single state transition in the rollup
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateTransition {
    pub transaction: Transaction,
    pub pre_state_root: Hash,
    pub post_state_root: Hash,
    pub timestamp: u64,
    pub execution_result: ExecutionResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub success: bool,
    pub error_message: Option<String>,
    pub compute_units_used: u64,
    pub logs: Vec<String>,
}

/// Batch of transactions with state proof
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionBatch {
    pub batch_id: u64,
    pub transactions: Vec<StateTransition>,
    pub pre_state_root: Hash,
    pub post_state_root: Hash,
    pub timestamp: u64,
}

impl TransactionBatch {
    pub fn new(batch_id: u64, pre_state_root: Hash) -> Self {
        Self {
            batch_id,
            transactions: Vec::new(),
            pre_state_root,
            post_state_root: pre_state_root,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        }
    }

    pub fn add_transition(&mut self, transition: StateTransition) {
        self.post_state_root = transition.post_state_root;
        self.transactions.push(transition);
    }

    pub fn is_full(&self, max_batch_size: usize) -> bool {
        self.transactions.len() >= max_batch_size
    }
}

/// Account state with versioning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountState {
    pub account: AccountSharedData,
    pub last_modified: u64,
    pub version: u64,
}

impl AccountState {
    pub fn new(account: AccountSharedData) -> Self {
        Self {
            account,
            last_modified: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            version: 0,
        }
    }

    pub fn update(&mut self, account: AccountSharedData) {
        self.account = account;
        self.last_modified = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.version += 1;
    }
}

/// Complete rollup state manager
#[derive(Debug)]
pub struct StateManager {
    /// Current account states
    accounts: HashMap<Pubkey, AccountState>,
    /// Transaction history
    transactions: HashMap<Hash, StateTransition>,
    /// Batches ready for settlement
    pending_batches: VecDeque<TransactionBatch>,
    /// Current batch being built
    current_batch: Option<TransactionBatch>,
    /// Batch counter
    batch_counter: u64,
    /// Maximum transactions per batch
    max_batch_size: usize,
    /// Current state root
    current_state_root: Hash,
}

impl StateManager {
    pub fn new(max_batch_size: usize) -> Self {
        let initial_root = Hash::default();
        Self {
            accounts: HashMap::new(),
            transactions: HashMap::new(),
            pending_batches: VecDeque::new(),
            current_batch: Some(TransactionBatch::new(0, initial_root)),
            batch_counter: 0,
            max_batch_size,
            current_state_root: initial_root,
        }
    }

    /// Get account by pubkey
    pub fn get_account(&self, pubkey: &Pubkey) -> Option<&AccountSharedData> {
        self.accounts.get(pubkey).map(|state| &state.account)
    }

    /// Get mutable account by pubkey
    pub fn get_account_mut(&mut self, pubkey: &Pubkey) -> Option<&mut AccountSharedData> {
        self.accounts.get_mut(pubkey).map(|state| &mut state.account)
    }

    /// Insert or update an account
    pub fn upsert_account(&mut self, pubkey: Pubkey, account: AccountSharedData) {
        if let Some(state) = self.accounts.get_mut(&pubkey) {
            state.update(account);
        } else {
            self.accounts.insert(pubkey, AccountState::new(account));
        }
    }

    /// Calculate current state root from all accounts
    pub fn calculate_state_root(&self) -> Hash {
        if self.accounts.is_empty() {
            return Hash::default();
        }

        let mut account_hashes: Vec<_> = self
            .accounts
            .iter()
            .map(|(pubkey, state)| {
                let mut hasher = Hasher::default();
                hasher.hash(pubkey.as_ref());
                hasher.hash(&state.account.lamports().to_le_bytes());
                hasher.hash(&state.account.data());
                hasher.hash(state.account.owner().as_ref());
                hasher.result()
            })
            .collect();

        // Sort for deterministic ordering
        account_hashes.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));

        let tree = MerkleTree::new(account_hashes);
        tree.root()
    }

    /// Add a state transition to the current batch
    pub fn add_transition(&mut self, transition: StateTransition) -> Result<()> {
        // Store in transaction history
        let tx_hash = self.hash_transaction(&transition.transaction);
        self.transactions.insert(tx_hash, transition.clone());

        // Add to current batch
        if let Some(batch) = &mut self.current_batch {
            batch.add_transition(transition);

            // Check if batch is full
            if batch.is_full(self.max_batch_size) {
                self.finalize_batch()?;
            }
        } else {
            return Err(anyhow!("No active batch"));
        }

        Ok(())
    }

    /// Finalize current batch and create a new one
    pub fn finalize_batch(&mut self) -> Result<TransactionBatch> {
        let batch = self
            .current_batch
            .take()
            .ok_or_else(|| anyhow!("No active batch"))?;

        self.current_state_root = batch.post_state_root;
        self.pending_batches.push_back(batch.clone());

        // Create new batch
        self.batch_counter += 1;
        self.current_batch = Some(TransactionBatch::new(
            self.batch_counter,
            self.current_state_root,
        ));

        Ok(batch)
    }

    /// Get next batch ready for settlement
    pub fn get_next_settlement_batch(&mut self) -> Option<TransactionBatch> {
        self.pending_batches.pop_front()
    }

    /// Get transaction by hash
    pub fn get_transaction(&self, tx_hash: &Hash) -> Option<&StateTransition> {
        self.transactions.get(tx_hash)
    }

    /// Get current state root
    pub fn get_state_root(&self) -> Hash {
        self.current_state_root
    }

    /// Get current batch info
    pub fn get_current_batch_size(&self) -> usize {
        self.current_batch
            .as_ref()
            .map(|b| b.transactions.len())
            .unwrap_or(0)
    }

    /// Get number of pending batches
    pub fn get_pending_batch_count(&self) -> usize {
        self.pending_batches.len()
    }

    /// Hash a transaction
    fn hash_transaction(&self, tx: &Transaction) -> Hash {
        let mut hasher = Hasher::default();
        if let Ok(serialized) = bincode::serialize(tx) {
            hasher.hash(&serialized);
        }
        hasher.result()
    }

    /// Get all accounts (for debugging/inspection)
    pub fn get_all_accounts(&self) -> &HashMap<Pubkey, AccountState> {
        &self.accounts
    }

    /// Force finalize current batch even if not full (for shutdown/testing)
    pub fn force_finalize_batch(&mut self) -> Result<Option<TransactionBatch>> {
        if let Some(ref batch) = self.current_batch {
            if batch.transactions.is_empty() {
                return Ok(None);
            }
        }
        self.finalize_batch().map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::native_token::LAMPORTS_PER_SOL;

    #[test]
    fn test_state_manager_creation() {
        let manager = StateManager::new(10);
        assert_eq!(manager.get_current_batch_size(), 0);
        assert_eq!(manager.get_pending_batch_count(), 0);
    }

    #[test]
    fn test_account_operations() {
        let mut manager = StateManager::new(10);
        let pubkey = Pubkey::new_unique();
        let account = AccountSharedData::new(LAMPORTS_PER_SOL, 0, &Pubkey::default());

        manager.upsert_account(pubkey, account.clone());
        assert!(manager.get_account(&pubkey).is_some());
        assert_eq!(manager.get_account(&pubkey).unwrap().lamports(), LAMPORTS_PER_SOL);
    }

    #[test]
    fn test_state_root_calculation() {
        let mut manager = StateManager::new(10);
        let initial_root = manager.calculate_state_root();

        let pubkey = Pubkey::new_unique();
        let account = AccountSharedData::new(LAMPORTS_PER_SOL, 0, &Pubkey::default());
        manager.upsert_account(pubkey, account);

        let new_root = manager.calculate_state_root();
        assert_ne!(initial_root, new_root);
    }
}
