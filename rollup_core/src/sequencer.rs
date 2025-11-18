use anyhow::{anyhow, Result};
use crossbeam::channel::{Receiver as CBReceiver, Sender as CBSender};
use solana_client::rpc_client::RpcClient;
use solana_compute_budget::compute_budget::ComputeBudget;
use solana_program_runtime::{
    invoke_context::EnvironmentConfig,
    invoke_context::InvokeContext,
    loaded_programs::{ProgramCacheForTxBatch, ProgramRuntimeEnvironments},
    sysvar_cache,
    timings::ExecuteTimings,
};
use solana_bpf_loader_program::syscalls::create_program_runtime_environment_v1;
use solana_sdk::{
    account::AccountSharedData,
    clock::{Epoch, Slot},
    feature_set::FeatureSet,
    hash::Hash as SolHash,
    pubkey::Pubkey,
    rent::Rent,
    transaction::{SanitizedTransaction, Transaction},
    transaction_context::TransactionContext,
};
use solana_svm::{
    message_processor::MessageProcessor,
    transaction_result::TransactionExecutionResult,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use crate::{
    rollupdb::{ProcessedTransaction, RollupDBMessage},
    settle::settle_state,
    state::ExecutionResult,
};

/// Configuration for the sequencer
pub struct SequencerConfig {
    pub max_batch_size: u32,
    pub rpc_url: String,
    pub enable_settlement: bool,
}

impl Default for SequencerConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 10,
            rpc_url: "https://api.devnet.solana.com".to_string(),
            enable_settlement: false,
        }
    }
}

/// Main sequencer function - receives transactions and processes them
pub fn run(
    sequencer_receiver_channel: CBReceiver<Transaction>,
    rollupdb_sender: CBSender<RollupDBMessage>,
) -> Result<()> {
    let config = SequencerConfig::default();
    run_with_config(sequencer_receiver_channel, rollupdb_sender, config)
}

/// Sequencer with custom configuration
pub fn run_with_config(
    sequencer_receiver_channel: CBReceiver<Transaction>,
    rollupdb_sender: CBSender<RollupDBMessage>,
    config: SequencerConfig,
) -> Result<()> {
    log::info!("Sequencer started with config: max_batch_size={}, enable_settlement={}",
        config.max_batch_size, config.enable_settlement);

    let mut tx_counter = 0u32;
    let rpc_client = RpcClient::new(config.rpc_url.clone());

    // Account cache to avoid refetching from RPC
    let mut account_cache: HashMap<Pubkey, AccountSharedData> = HashMap::new();

    while let Ok(transaction) = sequencer_receiver_channel.recv() {
        log::info!("Processing transaction #{}", tx_counter + 1);

        // Extract accounts to lock
        let accounts_to_lock = transaction.message.account_keys.clone();

        // Lock accounts in rollupdb
        if let Err(e) = rollupdb_sender.send(RollupDBMessage {
            lock_accounts: Some(accounts_to_lock.clone()),
            unlock_accounts: None,
            frontend_get_tx: None,
            add_settle_proof: None,
            add_processed_transaction: None,
            get_account: None,
            get_batch_for_settlement: false,
        }) {
            log::error!("Failed to lock accounts: {}", e);
            continue;
        }

        // Verify transaction signatures
        if let Err(e) = transaction.verify() {
            log::error!("Transaction signature verification failed: {}", e);

            // Unlock accounts
            let _ = rollupdb_sender.send(RollupDBMessage {
                lock_accounts: None,
                unlock_accounts: Some(accounts_to_lock),
                frontend_get_tx: None,
                add_settle_proof: None,
                add_processed_transaction: None,
                get_account: None,
                get_batch_for_settlement: false,
            });
            continue;
        }

        // Process transaction with SVM
        match process_transaction(&transaction, &rpc_client, &mut account_cache) {
            Ok(processed_tx) => {
                tx_counter += 1;

                // Send processed transaction to database
                if let Err(e) = rollupdb_sender.send(RollupDBMessage {
                    lock_accounts: None,
                    unlock_accounts: None,
                    add_processed_transaction: Some(processed_tx),
                    frontend_get_tx: None,
                    add_settle_proof: None,
                    get_account: None,
                    get_batch_for_settlement: false,
                }) {
                    log::error!("Failed to send processed transaction to DB: {}", e);
                }

                log::info!(
                    "Transaction processed successfully. Total transactions: {}",
                    tx_counter
                );

                // Check if we should settle
                if config.enable_settlement && tx_counter >= config.max_batch_size {
                    log::info!("Batch size reached, initiating settlement...");

                    // Request batch for settlement
                    if let Err(e) = rollupdb_sender.send(RollupDBMessage {
                        lock_accounts: None,
                        unlock_accounts: None,
                        add_processed_transaction: None,
                        frontend_get_tx: None,
                        add_settle_proof: None,
                        get_account: None,
                        get_batch_for_settlement: true,
                    }) {
                        log::error!("Failed to request settlement batch: {}", e);
                    }

                    tx_counter = 0;
                }
            }
            Err(e) => {
                log::error!("Transaction processing failed: {}", e);

                // Unlock accounts on error
                let _ = rollupdb_sender.send(RollupDBMessage {
                    lock_accounts: None,
                    unlock_accounts: Some(accounts_to_lock),
                    frontend_get_tx: None,
                    add_settle_proof: None,
                    add_processed_transaction: None,
                    get_account: None,
                    get_batch_for_settlement: false,
                });
            }
        }
    }

    log::info!("Sequencer shutting down");
    Ok(())
}

/// Process a single transaction using Solana SVM
fn process_transaction(
    transaction: &Transaction,
    rpc_client: &RpcClient,
    account_cache: &mut HashMap<Pubkey, AccountSharedData>,
) -> Result<ProcessedTransaction> {
    // Fetch accounts (with caching)
    let mut accounts_data = Vec::new();
    for pubkey in &transaction.message.account_keys {
        let account = if let Some(cached_account) = account_cache.get(pubkey) {
            log::debug!("Using cached account for {}", pubkey);
            cached_account.clone()
        } else {
            log::debug!("Fetching account {} from RPC", pubkey);
            let fetched_account = rpc_client
                .get_account(pubkey)
                .unwrap_or_else(|_| {
                    // Create default account if it doesn't exist
                    solana_sdk::account::Account::default()
                })
                .into();

            account_cache.insert(*pubkey, fetched_account.clone());
            fetched_account
        };
        accounts_data.push((*pubkey, account));
    }

    // Create transaction context
    let mut transaction_context =
        TransactionContext::new(accounts_data.clone(), Rent::default(), 0, 0);

    // Setup runtime environment
    let compute_budget = ComputeBudget::default();
    let feature_set = FeatureSet::all_enabled();

    let runtime_env = Arc::new(
        create_program_runtime_environment_v1(&feature_set, &compute_budget, false, false)
            .map_err(|e| anyhow!("Failed to create runtime environment: {}", e))?,
    );

    let mut prog_cache = ProgramCacheForTxBatch::new(
        Slot::default(),
        ProgramRuntimeEnvironments {
            program_runtime_v1: runtime_env.clone(),
            program_runtime_v2: runtime_env,
        },
        None,
        Epoch::default(),
    );

    // Setup environment
    let sysvar_cache = sysvar_cache::SysvarCache::default();
    let lamports_per_signature = 5000;
    let env = EnvironmentConfig::new(
        SolHash::default(),
        None,
        None,
        Arc::new(feature_set),
        lamports_per_signature,
        &sysvar_cache,
    );

    // Create invoke context
    let mut invoke_context = InvokeContext::new(
        &mut transaction_context,
        &mut prog_cache,
        env,
        None,
        compute_budget.clone(),
    );

    // Sanitize transaction
    let sanitized = SanitizedTransaction::try_from_legacy_transaction(
        transaction.clone(),
        &HashSet::new(),
    )
    .map_err(|e| anyhow!("Failed to sanitize transaction: {}", e))?;

    // Execute transaction
    let mut timings = ExecuteTimings::default();
    let mut used_cu = 0u64;

    let mut logs = Vec::new();
    let execution_result = MessageProcessor::process_message(
        &sanitized.message(),
        &vec![],
        &mut invoke_context,
        &mut timings,
        &mut used_cu,
    );

    log::debug!("Transaction execution result: {:?}", execution_result);
    log::debug!("Compute units used: {}", used_cu);

    // Extract execution result
    let (success, error_message) = match execution_result {
        Ok(_) => {
            log::info!("Transaction executed successfully");
            (true, None)
        }
        Err(e) => {
            log::warn!("Transaction execution failed: {:?}", e);
            (false, Some(format!("{:?}", e)))
        }
    };

    // Extract updated accounts from transaction context
    let mut updated_accounts = HashMap::new();

    // Get accounts from transaction context after execution
    for (index, pubkey) in transaction.message.account_keys.iter().enumerate() {
        if let Ok(account_ref) = invoke_context.transaction_context.get_account_at_index(index) {
            let account = account_ref.borrow().clone();
            updated_accounts.insert(*pubkey, account);
            log::debug!("Updated account {}: {} lamports", pubkey, account.lamports());
        }
    }

    // Create execution result
    let exec_result = ExecutionResult {
        success,
        error_message,
        compute_units_used: used_cu,
        logs,
    };

    Ok(ProcessedTransaction {
        transaction: transaction.clone(),
        execution_result: exec_result,
        updated_accounts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::{
        signature::{Keypair, Signer},
        system_instruction,
        native_token::LAMPORTS_PER_SOL,
    };

    #[test]
    fn test_sequencer_config() {
        let config = SequencerConfig::default();
        assert_eq!(config.max_batch_size, 10);
        assert!(!config.enable_settlement);
    }
}
