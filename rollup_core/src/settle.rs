use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    instruction::{AccountMeta, Instruction},
    keccak::Hash,
    pubkey::Pubkey,
    signature::{Keypair, Signature, Signer},
    system_program,
    transaction::Transaction,
};

use crate::state::TransactionBatch;

/// Settlement configuration
#[derive(Debug, Clone)]
pub struct SettlementConfig {
    /// RPC endpoint for the L1 chain (Solana)
    pub rpc_url: String,
    /// Settlement contract program ID
    pub program_id: Pubkey,
    /// Authority keypair for signing settlement transactions
    pub authority: Option<Keypair>,
    /// Enable or disable actual settlement (for testing)
    pub enabled: bool,
}

impl Default for SettlementConfig {
    fn default() -> Self {
        Self {
            rpc_url: "https://api.devnet.solana.com".to_string(),
            program_id: Pubkey::default(), // Would be actual program ID in production
            authority: None,
            enabled: false,
        }
    }
}

/// Settlement proof containing batch information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettlementProof {
    pub batch_id: u64,
    pub pre_state_root: Hash,
    pub post_state_root: Hash,
    pub transaction_count: usize,
    pub timestamp: u64,
    pub merkle_root: Hash,
}

impl SettlementProof {
    pub fn from_batch(batch: &TransactionBatch) -> Self {
        Self {
            batch_id: batch.batch_id,
            pre_state_root: batch.pre_state_root,
            post_state_root: batch.post_state_root,
            transaction_count: batch.transactions.len(),
            timestamp: batch.timestamp,
            merkle_root: batch.post_state_root, // In production, this would be a separate calculation
        }
    }

    /// Serialize proof to bytes for on-chain storage
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        bincode::serialize(self).map_err(|e| anyhow!("Failed to serialize proof: {}", e))
    }

    /// Deserialize proof from bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        bincode::deserialize(data).map_err(|e| anyhow!("Failed to deserialize proof: {}", e))
    }
}

/// Settle a batch on the L1 chain
pub async fn settle_batch(
    batch: &TransactionBatch,
    config: &SettlementConfig,
) -> Result<Option<Signature>> {
    if !config.enabled {
        log::info!("Settlement disabled, skipping batch {}", batch.batch_id);
        return Ok(None);
    }

    log::info!(
        "Settling batch {} with {} transactions",
        batch.batch_id,
        batch.transactions.len()
    );

    let proof = SettlementProof::from_batch(batch);
    settle_state_with_proof(proof, config).await.map(Some)
}

/// Settle the state on Solana with a proof
pub async fn settle_state_with_proof(
    proof: SettlementProof,
    config: &SettlementConfig,
) -> Result<Signature> {
    let rpc_client = RpcClient::new(config.rpc_url.clone());

    log::info!(
        "Submitting settlement proof for batch {} to L1",
        proof.batch_id
    );
    log::debug!("Proof details: {:?}", proof);

    // In a real implementation, you would:
    // 1. Create an instruction that calls your settlement contract
    // 2. Include the proof data as instruction data
    // 3. Sign and send the transaction

    // For now, we'll create a placeholder transaction
    let authority = config
        .authority
        .as_ref()
        .ok_or_else(|| anyhow!("No authority keypair configured"))?;

    // Create settlement instruction
    let instruction = create_settlement_instruction(
        &proof,
        &config.program_id,
        &authority.pubkey(),
    )?;

    // Create and send transaction
    let recent_blockhash = rpc_client
        .get_latest_blockhash()
        .await
        .map_err(|e| anyhow!("Failed to get blockhash: {}", e))?;

    let transaction = Transaction::new_signed_with_payer(
        &[instruction],
        Some(&authority.pubkey()),
        &[authority],
        recent_blockhash,
    );

    // Send transaction
    let signature = rpc_client
        .send_and_confirm_transaction_with_spinner(&transaction)
        .await
        .map_err(|e| anyhow!("Failed to send settlement transaction: {}", e))?;

    log::info!("Settlement transaction confirmed: {}", signature);

    Ok(signature)
}

/// Create a settlement instruction for the L1 contract
fn create_settlement_instruction(
    proof: &SettlementProof,
    program_id: &Pubkey,
    authority: &Pubkey,
) -> Result<Instruction> {
    // Serialize proof as instruction data
    let proof_data = proof.to_bytes()?;

    // Create instruction
    // In production, this would interact with your actual settlement contract
    Ok(Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*authority, true),          // Authority (signer)
            AccountMeta::new(Pubkey::new_unique(), false), // State account
            AccountMeta::new_readonly(system_program::ID, false), // System program
        ],
        data: proof_data,
    })
}

/// Legacy function for compatibility
pub async fn settle_state(state_root: Hash) -> Result<String> {
    log::info!("Settlement called with state root: {:?}", state_root);

    let config = SettlementConfig::default();
    let rpc_client = RpcClient::new(config.rpc_url);

    // Create a minimal proof
    let proof = SettlementProof {
        batch_id: 0,
        pre_state_root: Hash::default(),
        post_state_root: state_root,
        transaction_count: 0,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        merkle_root: state_root,
    };

    log::info!("Settlement proof created: {:?}", proof);

    // In production, this would actually submit to L1
    // For now, we just return a placeholder signature
    Ok(format!("settlement_{}", proof.batch_id))
}

/// Verify a settlement proof (used by validators)
pub fn verify_settlement_proof(proof: &SettlementProof) -> Result<bool> {
    // In production, this would:
    // 1. Verify the Merkle root
    // 2. Check the state transition is valid
    // 3. Verify signatures
    // 4. Check batch sequencing

    log::debug!("Verifying settlement proof for batch {}", proof.batch_id);

    // Basic validation
    if proof.transaction_count == 0 {
        log::warn!("Proof has no transactions");
        return Ok(false);
    }

    if proof.pre_state_root == proof.post_state_root {
        log::warn!("State root unchanged");
        return Ok(false);
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ExecutionResult, StateTransition};
    use solana_sdk::transaction::Transaction;

    #[test]
    fn test_settlement_proof_serialization() {
        let proof = SettlementProof {
            batch_id: 1,
            pre_state_root: Hash::default(),
            post_state_root: Hash::new(&[1u8; 32]),
            transaction_count: 10,
            timestamp: 1234567890,
            merkle_root: Hash::new(&[2u8; 32]),
        };

        let bytes = proof.to_bytes().unwrap();
        let deserialized = SettlementProof::from_bytes(&bytes).unwrap();

        assert_eq!(proof.batch_id, deserialized.batch_id);
        assert_eq!(proof.transaction_count, deserialized.transaction_count);
    }

    #[test]
    fn test_verify_settlement_proof() {
        let proof = SettlementProof {
            batch_id: 1,
            pre_state_root: Hash::default(),
            post_state_root: Hash::new(&[1u8; 32]),
            transaction_count: 10,
            timestamp: 1234567890,
            merkle_root: Hash::new(&[2u8; 32]),
        };

        assert!(verify_settlement_proof(&proof).unwrap());
    }

    #[test]
    fn test_empty_batch_verification_fails() {
        let proof = SettlementProof {
            batch_id: 1,
            pre_state_root: Hash::default(),
            post_state_root: Hash::new(&[1u8; 32]),
            transaction_count: 0, // Empty batch
            timestamp: 1234567890,
            merkle_root: Hash::new(&[2u8; 32]),
        };

        assert!(!verify_settlement_proof(&proof).unwrap());
    }
}
