use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::hash_utils::{Hash, Hasher};
use crate::merkle::MerkleTree;
use crate::state::StateTransition;

/// Fraud proof for challenging invalid state transitions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FraudProof {
    pub proof_id: u64,
    pub batch_id: u64,
    pub challenged_tx_index: usize,
    pub claim: FraudClaim,
    pub evidence: FraudEvidence,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FraudClaim {
    InvalidStateTransition {
        expected_post_state: Hash,
        actual_post_state: Hash,
    },
    InvalidExecution {
        reason: String,
    },
    InvalidSignature,
    DoubleSpend,
    InvalidNonce {
        expected: u64,
        actual: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FraudEvidence {
    pub pre_state_proof: Vec<Hash>,
    pub post_state_proof: Vec<Hash>,
    pub transaction_data: Vec<u8>,
    pub witnesses: Vec<String>,
}

/// Fraud proof manager
pub struct FraudProofManager {
    proofs: HashMap<u64, FraudProof>,
    proof_counter: std::sync::atomic::AtomicU64,
    challenge_period: u64, // seconds
}

impl FraudProofManager {
    pub fn new(challenge_period: u64) -> Self {
        Self {
            proofs: HashMap::new(),
            proof_counter: std::sync::atomic::AtomicU64::new(0),
            challenge_period,
        }
    }

    /// Submit a fraud proof
    pub fn submit_fraud_proof(
        &mut self,
        batch_id: u64,
        tx_index: usize,
        claim: FraudClaim,
        evidence: FraudEvidence,
    ) -> u64 {
        let proof_id = self.proof_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let proof = FraudProof {
            proof_id,
            batch_id,
            challenged_tx_index: tx_index,
            claim,
            evidence,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };

        log::warn!("Fraud proof submitted: ID {}, Batch {}", proof_id, batch_id);

        self.proofs.insert(proof_id, proof);
        proof_id
    }

    /// Verify fraud proof
    pub fn verify_proof(&self, proof_id: u64) -> Result<bool, String> {
        let proof = self.proofs.get(&proof_id)
            .ok_or_else(|| "Proof not found".to_string())?;

        match &proof.claim {
            FraudClaim::InvalidStateTransition { expected_post_state, actual_post_state } => {
                // Verify Merkle proofs
                let pre_state_valid = self.verify_merkle_proofs(&proof.evidence.pre_state_proof);
                let post_state_valid = self.verify_merkle_proofs(&proof.evidence.post_state_proof);

                if !pre_state_valid || !post_state_valid {
                    return Ok(false);
                }

                // Check if states differ
                Ok(expected_post_state != actual_post_state)
            }
            FraudClaim::InvalidExecution { .. } => {
                // Re-execute transaction and compare result
                Ok(true) // Simplified for now
            }
            FraudClaim::InvalidSignature => {
                // Verify signature
                Ok(true) // Simplified
            }
            FraudClaim::DoubleSpend => {
                // Check for double spend
                Ok(true) // Simplified
            }
            FraudClaim::InvalidNonce { expected, actual } => {
                Ok(expected != actual)
            }
        }
    }

    /// Verify Merkle proofs
    fn verify_merkle_proofs(&self, proofs: &[Hash]) -> bool {
        // Simplified verification
        !proofs.is_empty()
    }

    /// Get proof by ID
    pub fn get_proof(&self, proof_id: u64) -> Option<&FraudProof> {
        self.proofs.get(&proof_id)
    }

    /// Get all active proofs
    pub fn get_active_proofs(&self) -> Vec<&FraudProof> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        self.proofs
            .values()
            .filter(|p| now - p.timestamp < self.challenge_period)
            .collect()
    }

    /// Resolve proof (accept or reject)
    pub fn resolve_proof(&mut self, proof_id: u64, accepted: bool) {
        if accepted {
            log::warn!("Fraud proof {} ACCEPTED - rollback required", proof_id);
            // Trigger rollback to pre-fraud state
        } else {
            log::info!("Fraud proof {} rejected", proof_id);
        }
        self.proofs.remove(&proof_id);
    }
}

impl Default for FraudProofManager {
    fn default() -> Self {
        Self::new(7 * 24 * 3600) // 7 day challenge period
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fraud_proof_submission() {
        let mut manager = FraudProofManager::default();

        let claim = FraudClaim::InvalidNonce {
            expected: 1,
            actual: 5,
        };

        let evidence = FraudEvidence {
            pre_state_proof: vec![Hash::new(b"test")],
            post_state_proof: vec![Hash::new(b"test2")],
            transaction_data: vec![1, 2, 3],
            witnesses: vec!["witness1".to_string()],
        };

        let proof_id = manager.submit_fraud_proof(1, 0, claim, evidence);
        assert_eq!(proof_id, 0);
        assert!(manager.get_proof(proof_id).is_some());
    }

    #[test]
    fn test_fraud_proof_verification() {
        let mut manager = FraudProofManager::default();

        let claim = FraudClaim::InvalidNonce {
            expected: 1,
            actual: 5,
        };

        let evidence = FraudEvidence {
            pre_state_proof: vec![Hash::new(b"test")],
            post_state_proof: vec![Hash::new(b"test2")],
            transaction_data: vec![1, 2, 3],
            witnesses: vec![],
        };

        let proof_id = manager.submit_fraud_proof(1, 0, claim, evidence);
        let result = manager.verify_proof(proof_id);
        assert!(result.is_ok());
    }
}
