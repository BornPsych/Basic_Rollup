use anyhow::{anyhow, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::hash_utils::Hash;
use crate::types::Transaction;

/// Transaction simulator for gas estimation and pre-execution testing
pub struct TransactionSimulator {
    // State snapshot for simulation
    state_snapshot: Arc<DashMap<String, Vec<u8>>>,

    // Gas estimation models
    base_gas: u64,
    gas_per_byte: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationResult {
    pub success: bool,
    pub gas_used: u64,
    pub gas_estimate: GasEstimate,
    pub state_changes: HashMap<String, StateChange>,
    pub logs: Vec<SimulationLog>,
    pub error: Option<String>,
    pub revert_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasEstimate {
    pub base_fee: u64,
    pub execution_fee: u64,
    pub data_fee: u64,
    pub total_gas: u64,
    pub estimated_cost: u64,
    pub recommended_gas_limit: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateChange {
    pub before: Vec<u8>,
    pub after: Vec<u8>,
    pub change_type: StateChangeType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StateChangeType {
    BalanceUpdate,
    NonceUpdate,
    StorageWrite,
    CodeDeployment,
    AccountCreation,
    AccountDeletion,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationLog {
    pub level: LogLevel,
    pub message: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

impl TransactionSimulator {
    pub fn new() -> Self {
        Self {
            state_snapshot: Arc::new(DashMap::new()),
            base_gas: 21000,
            gas_per_byte: 16,
        }
    }

    /// Load state for simulation
    pub fn load_state(&self, state: HashMap<String, Vec<u8>>) {
        for (key, value) in state {
            self.state_snapshot.insert(key, value);
        }
    }

    /// Simulate transaction execution
    pub fn simulate(&self, tx: &Transaction) -> SimulationResult {
        let mut logs = Vec::new();
        let mut state_changes = HashMap::new();

        // Step 1: Validate transaction
        logs.push(SimulationLog {
            level: LogLevel::Info,
            message: "Validating transaction".to_string(),
            data: vec![],
        });

        if let Err(e) = self.validate_transaction(tx) {
            return SimulationResult {
                success: false,
                gas_used: self.base_gas,
                gas_estimate: self.estimate_gas(tx, false),
                state_changes,
                logs,
                error: Some(e.to_string()),
                revert_reason: None,
            };
        }

        // Step 2: Check sender balance
        logs.push(SimulationLog {
            level: LogLevel::Info,
            message: "Checking sender balance".to_string(),
            data: vec![],
        });

        let sender_balance = self.get_balance(&tx.from);

        if sender_balance < tx.value {
            return SimulationResult {
                success: false,
                gas_used: self.base_gas,
                gas_estimate: self.estimate_gas(tx, false),
                state_changes,
                logs,
                error: Some("Insufficient balance".to_string()),
                revert_reason: Some(format!(
                    "Balance {} < required {}",
                    sender_balance, tx.value
                )),
            };
        }

        // Step 3: Simulate execution
        logs.push(SimulationLog {
            level: LogLevel::Info,
            message: "Executing transaction".to_string(),
            data: vec![],
        });

        // Update sender balance
        let new_sender_balance = sender_balance - tx.value;
        state_changes.insert(
            tx.from.clone(),
            StateChange {
                before: sender_balance.to_le_bytes().to_vec(),
                after: new_sender_balance.to_le_bytes().to_vec(),
                change_type: StateChangeType::BalanceUpdate,
            },
        );

        // Update receiver balance
        if let Some(ref to) = tx.to {
            let receiver_balance = self.get_balance(to);
            let new_receiver_balance = receiver_balance + tx.value;

            state_changes.insert(
                to.clone(),
                StateChange {
                    before: receiver_balance.to_le_bytes().to_vec(),
                    after: new_receiver_balance.to_le_bytes().to_vec(),
                    change_type: StateChangeType::BalanceUpdate,
                },
            );
        } else {
            // Contract deployment
            logs.push(SimulationLog {
                level: LogLevel::Info,
                message: "Contract deployment detected".to_string(),
                data: vec![],
            });
        }

        // Calculate gas
        let gas_estimate = self.estimate_gas(tx, true);

        logs.push(SimulationLog {
            level: LogLevel::Info,
            message: format!("Simulation successful, gas used: {}", gas_estimate.total_gas),
            data: vec![],
        });

        SimulationResult {
            success: true,
            gas_used: gas_estimate.total_gas,
            gas_estimate,
            state_changes,
            logs,
            error: None,
            revert_reason: None,
        }
    }

    /// Estimate gas for transaction
    pub fn estimate_gas(&self, tx: &Transaction, successful: bool) -> GasEstimate {
        let base_fee = self.base_gas;

        // Data fee (cost of calldata)
        let data_fee = tx.data.len() as u64 * self.gas_per_byte;

        // Execution fee (simplified model)
        let execution_fee = if successful {
            // Transfer: 21000
            // Contract call: 21000 + extra
            if tx.to.is_none() {
                // Contract deployment
                50000 + data_fee
            } else if !tx.data.is_empty() {
                // Contract call
                30000
            } else {
                // Simple transfer
                0
            }
        } else {
            0
        };

        let total_gas = base_fee + execution_fee + data_fee;

        // Add 20% safety margin
        let recommended_gas_limit = (total_gas as f64 * 1.2) as u64;

        // Estimated cost (assuming 1 gwei per gas)
        let estimated_cost = total_gas * 1_000_000_000;

        GasEstimate {
            base_fee,
            execution_fee,
            data_fee,
            total_gas,
            estimated_cost,
            recommended_gas_limit,
        }
    }

    /// Batch simulate multiple transactions
    pub fn simulate_batch(&self, transactions: &[Transaction]) -> Vec<SimulationResult> {
        let mut results = Vec::new();
        let mut cumulative_state = HashMap::new();

        for tx in transactions {
            // Load previous state changes
            for (key, value) in &cumulative_state {
                self.state_snapshot.insert(key.clone(), value.clone());
            }

            // Simulate transaction
            let result = self.simulate(tx);

            // Apply state changes for next simulation
            if result.success {
                for (key, change) in &result.state_changes {
                    cumulative_state.insert(key.clone(), change.after.clone());
                }
            }

            results.push(result);
        }

        results
    }

    /// Validate transaction format and basic checks
    fn validate_transaction(&self, tx: &Transaction) -> Result<()> {
        if tx.from.is_empty() {
            return Err(anyhow!("Sender address is empty"));
        }

        if tx.gas_limit < self.base_gas {
            return Err(anyhow!(
                "Gas limit {} is below minimum {}",
                tx.gas_limit,
                self.base_gas
            ));
        }

        if tx.max_fee_per_gas == 0 {
            return Err(anyhow!("Max fee per gas cannot be zero"));
        }

        Ok(())
    }

    /// Get balance from state snapshot
    fn get_balance(&self, address: &str) -> u64 {
        self.state_snapshot
            .get(address)
            .map(|data| {
                if data.len() >= 8 {
                    u64::from_le_bytes(data[..8].try_into().unwrap_or([0u8; 8]))
                } else {
                    0
                }
            })
            .unwrap_or(0)
    }

    /// Clear state snapshot
    pub fn clear_state(&self) {
        self.state_snapshot.clear();
    }
}

impl Default for TransactionSimulator {
    fn default() -> Self {
        Self::new()
    }
}

/// Gas estimator for quick gas calculations
pub struct GasEstimator {
    base_gas: u64,
    gas_per_byte: u64,
    transfer_gas: u64,
    contract_call_gas: u64,
    contract_creation_gas: u64,
}

impl GasEstimator {
    pub fn new() -> Self {
        Self {
            base_gas: 21000,
            gas_per_byte: 16,
            transfer_gas: 21000,
            contract_call_gas: 30000,
            contract_creation_gas: 50000,
        }
    }

    /// Quick gas estimate without simulation
    pub fn quick_estimate(&self, tx: &Transaction) -> u64 {
        let base = self.base_gas;
        let data_cost = tx.data.len() as u64 * self.gas_per_byte;

        let execution_cost = if tx.to.is_none() {
            self.contract_creation_gas + data_cost
        } else if !tx.data.is_empty() {
            self.contract_call_gas
        } else {
            0
        };

        base + execution_cost + data_cost
    }

    /// Estimate gas for batch
    pub fn estimate_batch(&self, transactions: &[Transaction]) -> u64 {
        transactions.iter().map(|tx| self.quick_estimate(tx)).sum()
    }

    /// Get recommended gas limit with safety margin
    pub fn recommend_gas_limit(&self, tx: &Transaction, safety_margin: f64) -> u64 {
        let estimate = self.quick_estimate(tx);
        (estimate as f64 * (1.0 + safety_margin)) as u64
    }
}

impl Default for GasEstimator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_tx(from: &str, to: Option<&str>, value: u64) -> Transaction {
        Transaction {
            from: from.to_string(),
            to: to.map(|s| s.to_string()),
            value,
            data: vec![],
            nonce: 0,
            gas_limit: 21000,
            max_fee_per_gas: 1,
        }
    }

    #[test]
    fn test_simulation_success() {
        let simulator = TransactionSimulator::new();

        // Set up state
        let mut state = HashMap::new();
        state.insert("addr1".to_string(), 1000u64.to_le_bytes().to_vec());
        simulator.load_state(state);

        let tx = create_test_tx("addr1", Some("addr2"), 100);
        let result = simulator.simulate(&tx);

        assert!(result.success);
        assert_eq!(result.state_changes.len(), 2); // sender and receiver
    }

    #[test]
    fn test_simulation_insufficient_balance() {
        let simulator = TransactionSimulator::new();

        // Set up state with insufficient balance
        let mut state = HashMap::new();
        state.insert("addr1".to_string(), 50u64.to_le_bytes().to_vec());
        simulator.load_state(state);

        let tx = create_test_tx("addr1", Some("addr2"), 100);
        let result = simulator.simulate(&tx);

        assert!(!result.success);
        assert!(result.error.is_some());
        assert!(result.revert_reason.is_some());
    }

    #[test]
    fn test_gas_estimation() {
        let simulator = TransactionSimulator::new();

        let tx = create_test_tx("addr1", Some("addr2"), 100);
        let estimate = simulator.estimate_gas(&tx, true);

        assert_eq!(estimate.base_fee, 21000);
        assert!(estimate.total_gas >= 21000);
        assert!(estimate.recommended_gas_limit > estimate.total_gas);
    }

    #[test]
    fn test_batch_simulation() {
        let simulator = TransactionSimulator::new();

        // Set up state
        let mut state = HashMap::new();
        state.insert("addr1".to_string(), 1000u64.to_le_bytes().to_vec());
        state.insert("addr2".to_string(), 500u64.to_le_bytes().to_vec());
        simulator.load_state(state);

        let txs = vec![
            create_test_tx("addr1", Some("addr2"), 100),
            create_test_tx("addr2", Some("addr3"), 50),
        ];

        let results = simulator.simulate_batch(&txs);

        assert_eq!(results.len(), 2);
        assert!(results[0].success);
        assert!(results[1].success);
    }

    #[test]
    fn test_gas_estimator() {
        let estimator = GasEstimator::new();

        let tx = create_test_tx("addr1", Some("addr2"), 100);
        let estimate = estimator.quick_estimate(&tx);

        assert_eq!(estimate, 21000);

        let recommended = estimator.recommend_gas_limit(&tx, 0.2);
        assert!(recommended > estimate);
    }
}
