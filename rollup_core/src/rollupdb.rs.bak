use anyhow::{anyhow, Result};
use async_channel::Sender;
use crossbeam::channel::{Receiver as CBReceiver, Sender as CBSender};
use serde::{Deserialize, Serialize};
use solana_sdk::{
    account::AccountSharedData, keccak::Hash, pubkey::Pubkey, transaction::Transaction,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
};

use crate::{
    frontend::FrontendMessage,
    state::{ExecutionResult, StateManager, StateTransition},
};

/// Messages that can be sent to the RollupDB
#[derive(Serialize, Deserialize, Clone)]
pub struct RollupDBMessage {
    pub lock_accounts: Option<Vec<Pubkey>>,
    pub unlock_accounts: Option<Vec<Pubkey>>,
    pub add_processed_transaction: Option<ProcessedTransaction>,
    pub frontend_get_tx: Option<Hash>,
    pub get_account: Option<Pubkey>,
    pub add_settle_proof: Option<String>,
    pub get_batch_for_settlement: bool,
}

/// Processed transaction with execution result
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProcessedTransaction {
    pub transaction: Transaction,
    pub execution_result: ExecutionResult,
    pub updated_accounts: HashMap<Pubkey, AccountSharedData>,
}

/// Response message from RollupDB
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RollupDBResponse {
    pub transaction: Option<StateTransition>,
    pub account: Option<AccountSharedData>,
    pub success: bool,
    pub error: Option<String>,
}

/// Main RollupDB structure managing all rollup state
pub struct RollupDB {
    /// State manager for accounts and batches
    state_manager: Arc<RwLock<StateManager>>,
    /// Currently locked accounts (for concurrent transaction processing)
    locked_accounts: HashSet<Pubkey>,
    /// Settlement proofs stored
    settlement_proofs: Vec<String>,
}

impl RollupDB {
    pub fn new(max_batch_size: usize) -> Self {
        Self {
            state_manager: Arc::new(RwLock::new(StateManager::new(max_batch_size))),
            locked_accounts: HashSet::new(),
            settlement_proofs: Vec::new(),
        }
    }

    /// Main event loop for RollupDB
    pub async fn run(
        rollup_db_receiver: CBReceiver<RollupDBMessage>,
        frontend_sender: Sender<FrontendMessage>,
    ) {
        let mut db = RollupDB::new(10); // Max 10 transactions per batch

        log::info!("RollupDB started");

        while let Ok(message) = rollup_db_receiver.recv() {
            if let Err(e) = db.process_message(message, &frontend_sender).await {
                log::error!("Error processing RollupDB message: {:?}", e);
            }
        }

        log::info!("RollupDB shutting down");
    }

    /// Process incoming messages
    async fn process_message(
        &mut self,
        message: RollupDBMessage,
        frontend_sender: &Sender<FrontendMessage>,
    ) -> Result<()> {
        // Handle account locking
        if let Some(accounts_to_lock) = message.lock_accounts {
            self.lock_accounts(accounts_to_lock)?;
        }

        // Handle account unlocking
        if let Some(accounts_to_unlock) = message.unlock_accounts {
            self.unlock_accounts(accounts_to_unlock);
        }

        // Handle transaction retrieval
        if let Some(tx_hash) = message.frontend_get_tx {
            let transition = self.get_transaction(&tx_hash);

            frontend_sender
                .send(FrontendMessage {
                    transaction: transition,
                    get_tx: None,
                    account: None,
                    state_root: None,
                    batch_info: None,
                })
                .await
                .map_err(|e| anyhow!("Failed to send to frontend: {}", e))?;
        }

        // Handle account retrieval
        if let Some(pubkey) = message.get_account {
            let account = self.get_account(&pubkey).cloned();

            frontend_sender
                .send(FrontendMessage {
                    transaction: None,
                    get_tx: None,
                    account,
                    state_root: None,
                    batch_info: None,
                })
                .await
                .map_err(|e| anyhow!("Failed to send to frontend: {}", e))?;
        }

        // Handle processed transaction storage
        if let Some(processed_tx) = message.add_processed_transaction {
            self.add_processed_transaction(processed_tx)?;
        }

        // Handle settlement proof storage
        if let Some(proof) = message.add_settle_proof {
            self.settlement_proofs.push(proof);
            log::info!("Stored settlement proof #{}", self.settlement_proofs.len());
        }

        // Handle batch retrieval for settlement
        if message.get_batch_for_settlement {
            let batch = self
                .state_manager
                .write()
                .unwrap()
                .get_next_settlement_batch();

            if let Some(batch) = batch {
                log::info!("Batch {} ready for settlement", batch.batch_id);
                frontend_sender
                    .send(FrontendMessage {
                        transaction: None,
                        get_tx: None,
                        account: None,
                        state_root: Some(batch.post_state_root),
                        batch_info: Some(batch),
                    })
                    .await
                    .map_err(|e| anyhow!("Failed to send batch to frontend: {}", e))?;
            }
        }

        Ok(())
    }

    /// Lock accounts for transaction processing
    fn lock_accounts(&mut self, accounts: Vec<Pubkey>) -> Result<()> {
        for pubkey in accounts {
            if self.locked_accounts.contains(&pubkey) {
                return Err(anyhow!("Account {} is already locked", pubkey));
            }
            self.locked_accounts.insert(pubkey);
        }
        log::debug!("Locked {} accounts", self.locked_accounts.len());
        Ok(())
    }

    /// Unlock accounts after transaction processing
    fn unlock_accounts(&mut self, accounts: Vec<Pubkey>) {
        for pubkey in accounts {
            self.locked_accounts.remove(&pubkey);
        }
        log::debug!(
            "Unlocked accounts, {} still locked",
            self.locked_accounts.len()
        );
    }

    /// Get account from state
    fn get_account(&self, pubkey: &Pubkey) -> Option<AccountSharedData> {
        self.state_manager.read().unwrap().get_account(pubkey).cloned()
    }

    /// Get transaction from state
    fn get_transaction(&self, tx_hash: &Hash) -> Option<StateTransition> {
        self.state_manager.read().unwrap().get_transaction(tx_hash).cloned()
    }

    /// Add a processed transaction to the state
    fn add_processed_transaction(&mut self, processed_tx: ProcessedTransaction) -> Result<()> {
        let mut state_mgr = self.state_manager.write().unwrap();

        // Calculate pre-state root
        let pre_state_root = state_mgr.calculate_state_root();

        // Update accounts in state
        for (pubkey, account) in processed_tx.updated_accounts {
            state_mgr.upsert_account(pubkey, account);
            // Unlock the account
            self.locked_accounts.remove(&pubkey);
        }

        // Calculate post-state root
        let post_state_root = state_mgr.calculate_state_root();

        // Create state transition
        let transition = StateTransition {
            transaction: processed_tx.transaction,
            pre_state_root,
            post_state_root,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            execution_result: processed_tx.execution_result,
        };

        // Add to state manager
        state_mgr.add_transition(transition)?;

        log::info!(
            "Added transaction to state. Current batch size: {}",
            state_mgr.get_current_batch_size()
        );

        Ok(())
    }

    /// Get current state root
    pub fn get_state_root(&self) -> Hash {
        self.state_manager.read().unwrap().get_state_root()
    }

    /// Get statistics
    pub fn get_stats(&self) -> RollupStats {
        let state_mgr = self.state_manager.read().unwrap();
        RollupStats {
            locked_accounts: self.locked_accounts.len(),
            current_batch_size: state_mgr.get_current_batch_size(),
            pending_batches: state_mgr.get_pending_batch_count(),
            settlement_proofs: self.settlement_proofs.len(),
            current_state_root: state_mgr.get_state_root(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RollupStats {
    pub locked_accounts: usize,
    pub current_batch_size: usize,
    pub pending_batches: usize,
    pub settlement_proofs: usize,
    pub current_state_root: Hash,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rollup_db_creation() {
        let db = RollupDB::new(10);
        assert_eq!(db.locked_accounts.len(), 0);
    }

    #[test]
    fn test_account_locking() {
        let mut db = RollupDB::new(10);
        let pubkey = Pubkey::new_unique();

        db.lock_accounts(vec![pubkey]).unwrap();
        assert!(db.locked_accounts.contains(&pubkey));

        // Should fail to lock again
        assert!(db.lock_accounts(vec![pubkey]).is_err());

        db.unlock_accounts(vec![pubkey]);
        assert!(!db.locked_accounts.contains(&pubkey));
    }
}
