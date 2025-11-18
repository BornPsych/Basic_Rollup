use anyhow::{anyhow, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::hash_utils::Hash;

/// Smart contract deployment and management system
pub struct ContractManager {
    contracts: Arc<DashMap<String, DeployedContract>>,
    abis: Arc<DashMap<String, ContractABI>>,
    verified_contracts: Arc<DashMap<String, VerificationInfo>>,
    deployment_counter: AtomicU64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployedContract {
    pub address: String,
    pub deployer: String,
    pub code_hash: Hash,
    pub bytecode: Vec<u8>,
    pub deployed_at: u64,
    pub deployment_tx: Hash,
    pub is_verified: bool,
    pub total_calls: u64,
    pub total_gas_used: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractABI {
    pub contract_address: String,
    pub abi: String, // JSON ABI
    pub functions: Vec<FunctionSignature>,
    pub events: Vec<EventSignature>,
    pub constructor: Option<FunctionSignature>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionSignature {
    pub name: String,
    pub inputs: Vec<Parameter>,
    pub outputs: Vec<Parameter>,
    pub state_mutability: StateMutability,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSignature {
    pub name: String,
    pub inputs: Vec<Parameter>,
    pub anonymous: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub param_type: String,
    pub indexed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StateMutability {
    Pure,
    View,
    NonPayable,
    Payable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationInfo {
    pub contract_address: String,
    pub source_code: String,
    pub compiler_version: String,
    pub optimization_enabled: bool,
    pub optimization_runs: u32,
    pub verified_at: u64,
    pub verifier: String,
}

impl ContractManager {
    pub fn new() -> Self {
        Self {
            contracts: Arc::new(DashMap::new()),
            abis: Arc::new(DashMap::new()),
            verified_contracts: Arc::new(DashMap::new()),
            deployment_counter: AtomicU64::new(0),
        }
    }

    /// Deploy a new contract
    pub fn deploy_contract(
        &self,
        deployer: String,
        bytecode: Vec<u8>,
        deployment_tx: Hash,
    ) -> Result<DeployedContract> {
        let deployment_id = self.deployment_counter.fetch_add(1, Ordering::SeqCst);

        // Generate contract address (simplified)
        let address = format!("contract_{}", deployment_id);

        let code_hash = Hash::new(&bytecode);
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        let contract = DeployedContract {
            address: address.clone(),
            deployer,
            code_hash,
            bytecode,
            deployed_at: now,
            deployment_tx,
            is_verified: false,
            total_calls: 0,
            total_gas_used: 0,
        };

        self.contracts.insert(address.clone(), contract.clone());

        log::info!(
            "Deployed contract {} by {} (code hash: {:?})",
            address,
            contract.deployer,
            code_hash
        );

        Ok(contract)
    }

    /// Register ABI for a contract
    pub fn register_abi(&self, abi: ContractABI) -> Result<()> {
        if !self.contracts.contains_key(&abi.contract_address) {
            return Err(anyhow!("Contract not found"));
        }

        self.abis.insert(abi.contract_address.clone(), abi.clone());

        log::info!(
            "Registered ABI for contract {} ({} functions, {} events)",
            abi.contract_address,
            abi.functions.len(),
            abi.events.len()
        );

        Ok(())
    }

    /// Verify contract source code
    pub fn verify_contract(&self, verification: VerificationInfo) -> Result<()> {
        let mut contract = self
            .contracts
            .get_mut(&verification.contract_address)
            .ok_or_else(|| anyhow!("Contract not found"))?;

        // In production, would actually compile and verify bytecode matches
        contract.is_verified = true;

        self.verified_contracts
            .insert(verification.contract_address.clone(), verification.clone());

        log::info!(
            "Verified contract {} with compiler {}",
            verification.contract_address,
            verification.compiler_version
        );

        Ok(())
    }

    /// Record contract call
    pub fn record_call(&self, contract_address: &str, gas_used: u64) -> Result<()> {
        let mut contract = self
            .contracts
            .get_mut(contract_address)
            .ok_or_else(|| anyhow!("Contract not found"))?;

        contract.total_calls += 1;
        contract.total_gas_used += gas_used;

        Ok(())
    }

    /// Get contract by address
    pub fn get_contract(&self, address: &str) -> Option<DeployedContract> {
        self.contracts.get(address).map(|c| c.clone())
    }

    /// Get contract ABI
    pub fn get_abi(&self, address: &str) -> Option<ContractABI> {
        self.abis.get(address).map(|a| a.clone())
    }

    /// Get verification info
    pub fn get_verification(&self, address: &str) -> Option<VerificationInfo> {
        self.verified_contracts.get(address).map(|v| v.clone())
    }

    /// Get all contracts
    pub fn get_all_contracts(&self) -> Vec<DeployedContract> {
        self.contracts.iter().map(|e| e.value().clone()).collect()
    }

    /// Get verified contracts
    pub fn get_verified_contracts(&self) -> Vec<DeployedContract> {
        self.contracts
            .iter()
            .filter(|e| e.value().is_verified)
            .map(|e| e.value().clone())
            .collect()
    }

    /// Search contracts by deployer
    pub fn get_contracts_by_deployer(&self, deployer: &str) -> Vec<DeployedContract> {
        self.contracts
            .iter()
            .filter(|e| e.value().deployer == deployer)
            .map(|e| e.value().clone())
            .collect()
    }

    /// Get contract statistics
    pub fn get_stats(&self) -> ContractStats {
        let contracts: Vec<_> = self.get_all_contracts();

        ContractStats {
            total_contracts: contracts.len(),
            verified_contracts: contracts.iter().filter(|c| c.is_verified).count(),
            total_calls: contracts.iter().map(|c| c.total_calls).sum(),
            total_gas_used: contracts.iter().map(|c| c.total_gas_used).sum(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractStats {
    pub total_contracts: usize,
    pub verified_contracts: usize,
    pub total_calls: u64,
    pub total_gas_used: u64,
}

impl Default for ContractManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contract_deployment() {
        let manager = ContractManager::new();

        let contract = manager
            .deploy_contract(
                "deployer1".to_string(),
                vec![1, 2, 3, 4],
                Hash::new(b"tx1"),
            )
            .unwrap();

        assert!(contract.address.starts_with("contract_"));
        assert!(!contract.is_verified);

        let retrieved = manager.get_contract(&contract.address).unwrap();
        assert_eq!(retrieved.deployer, "deployer1");
    }

    #[test]
    fn test_abi_registration() {
        let manager = ContractManager::new();

        let contract = manager
            .deploy_contract(
                "deployer1".to_string(),
                vec![1, 2, 3],
                Hash::new(b"tx1"),
            )
            .unwrap();

        let abi = ContractABI {
            contract_address: contract.address.clone(),
            abi: "{}".to_string(),
            functions: vec![],
            events: vec![],
            constructor: None,
        };

        manager.register_abi(abi).unwrap();

        let retrieved_abi = manager.get_abi(&contract.address).unwrap();
        assert_eq!(retrieved_abi.contract_address, contract.address);
    }

    #[test]
    fn test_contract_verification() {
        let manager = ContractManager::new();

        let contract = manager
            .deploy_contract(
                "deployer1".to_string(),
                vec![1, 2, 3],
                Hash::new(b"tx1"),
            )
            .unwrap();

        let verification = VerificationInfo {
            contract_address: contract.address.clone(),
            source_code: "contract Test {}".to_string(),
            compiler_version: "0.8.0".to_string(),
            optimization_enabled: true,
            optimization_runs: 200,
            verified_at: 0,
            verifier: "admin".to_string(),
        };

        manager.verify_contract(verification).unwrap();

        let updated = manager.get_contract(&contract.address).unwrap();
        assert!(updated.is_verified);
    }

    #[test]
    fn test_contract_calls() {
        let manager = ContractManager::new();

        let contract = manager
            .deploy_contract(
                "deployer1".to_string(),
                vec![1, 2, 3],
                Hash::new(b"tx1"),
            )
            .unwrap();

        manager.record_call(&contract.address, 100).unwrap();
        manager.record_call(&contract.address, 150).unwrap();

        let updated = manager.get_contract(&contract.address).unwrap();
        assert_eq!(updated.total_calls, 2);
        assert_eq!(updated.total_gas_used, 250);
    }
}
