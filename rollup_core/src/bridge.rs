use anyhow::{anyhow, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::hash_utils::Hash;

/// Cross-chain bridge management system
pub struct BridgeManager {
    bridges: Arc<DashMap<String, Bridge>>,
    transfers: Arc<DashMap<Hash, BridgeTransfer>>,
    wrapped_tokens: Arc<DashMap<String, WrappedToken>>,
    transfer_counter: AtomicU64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bridge {
    pub bridge_id: String,
    pub chain_id: u64,
    pub chain_name: String,
    pub bridge_address: String,
    pub status: BridgeStatus,
    pub total_locked: u64,
    pub total_transfers: u64,
    pub supported_tokens: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum BridgeStatus {
    Active,
    Paused,
    Deprecated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeTransfer {
    pub transfer_id: u64,
    pub transfer_hash: Hash,
    pub bridge_id: String,
    pub direction: TransferDirection,
    pub sender: String,
    pub recipient: String,
    pub token: String,
    pub amount: u64,
    pub status: TransferStatus,
    pub initiated_at: u64,
    pub completed_at: Option<u64>,
    pub proof: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransferDirection {
    Deposit,  // From external chain to rollup
    Withdrawal, // From rollup to external chain
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransferStatus {
    Pending,
    Confirmed,
    Completed,
    Failed,
    Challenged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WrappedToken {
    pub wrapped_address: String,
    pub original_chain: u64,
    pub original_address: String,
    pub total_supply: u64,
    pub holders: u64,
}

impl BridgeManager {
    pub fn new() -> Self {
        Self {
            bridges: Arc::new(DashMap::new()),
            transfers: Arc::new(DashMap::new()),
            wrapped_tokens: Arc::new(DashMap::new()),
            transfer_counter: AtomicU64::new(0),
        }
    }

    /// Register a new bridge
    pub fn register_bridge(&self, bridge: Bridge) -> Result<()> {
        if self.bridges.contains_key(&bridge.bridge_id) {
            return Err(anyhow!("Bridge already registered"));
        }

        log::info!(
            "Registered bridge {} for chain {} ({})",
            bridge.bridge_id,
            bridge.chain_id,
            bridge.chain_name
        );

        self.bridges.insert(bridge.bridge_id.clone(), bridge);
        Ok(())
    }

    /// Initiate a deposit (external chain -> rollup)
    pub fn initiate_deposit(
        &self,
        bridge_id: String,
        sender: String,
        recipient: String,
        token: String,
        amount: u64,
        proof: Vec<u8>,
    ) -> Result<BridgeTransfer> {
        let bridge = self
            .bridges
            .get(&bridge_id)
            .ok_or_else(|| anyhow!("Bridge not found"))?;

        if bridge.status != BridgeStatus::Active {
            return Err(anyhow!("Bridge is not active"));
        }

        let transfer_id = self.transfer_counter.fetch_add(1, Ordering::SeqCst);
        let transfer_hash = Hash::new(&bincode::serialize(&transfer_id).unwrap_or_default());
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        let transfer = BridgeTransfer {
            transfer_id,
            transfer_hash,
            bridge_id: bridge_id.clone(),
            direction: TransferDirection::Deposit,
            sender,
            recipient: recipient.clone(),
            token: token.clone(),
            amount,
            status: TransferStatus::Pending,
            initiated_at: now,
            completed_at: None,
            proof: Some(proof),
        };

        self.transfers.insert(transfer_hash, transfer.clone());

        log::info!(
            "Initiated deposit {} - {} {} to {}",
            transfer_id,
            amount,
            token,
            recipient
        );

        Ok(transfer)
    }

    /// Initiate a withdrawal (rollup -> external chain)
    pub fn initiate_withdrawal(
        &self,
        bridge_id: String,
        sender: String,
        recipient: String,
        token: String,
        amount: u64,
    ) -> Result<BridgeTransfer> {
        let bridge = self
            .bridges
            .get(&bridge_id)
            .ok_or_else(|| anyhow!("Bridge not found"))?;

        if bridge.status != BridgeStatus::Active {
            return Err(anyhow!("Bridge is not active"));
        }

        let transfer_id = self.transfer_counter.fetch_add(1, Ordering::SeqCst);
        let transfer_hash = Hash::new(&bincode::serialize(&transfer_id).unwrap_or_default());
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        let transfer = BridgeTransfer {
            transfer_id,
            transfer_hash,
            bridge_id: bridge_id.clone(),
            direction: TransferDirection::Withdrawal,
            sender,
            recipient: recipient.clone(),
            token: token.clone(),
            amount,
            status: TransferStatus::Pending,
            initiated_at: now,
            completed_at: None,
            proof: None,
        };

        self.transfers.insert(transfer_hash, transfer.clone());

        log::info!(
            "Initiated withdrawal {} - {} {} to {}",
            transfer_id,
            amount,
            token,
            recipient
        );

        Ok(transfer)
    }

    /// Complete a transfer
    pub fn complete_transfer(&self, transfer_hash: Hash) -> Result<()> {
        let mut transfer = self
            .transfers
            .get_mut(&transfer_hash)
            .ok_or_else(|| anyhow!("Transfer not found"))?;

        if transfer.status != TransferStatus::Confirmed {
            return Err(anyhow!("Transfer not confirmed"));
        }

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        transfer.status = TransferStatus::Completed;
        transfer.completed_at = Some(now);

        // Update bridge stats
        if let Some(mut bridge) = self.bridges.get_mut(&transfer.bridge_id) {
            bridge.total_transfers += 1;
        }

        log::info!("Completed transfer {}", transfer.transfer_id);

        Ok(())
    }

    /// Create wrapped token
    pub fn create_wrapped_token(
        &self,
        original_chain: u64,
        original_address: String,
    ) -> Result<WrappedToken> {
        let wrapped_address = format!("wrapped_{}_{}", original_chain, original_address);

        if self.wrapped_tokens.contains_key(&wrapped_address) {
            return Err(anyhow!("Wrapped token already exists"));
        }

        let token = WrappedToken {
            wrapped_address: wrapped_address.clone(),
            original_chain,
            original_address,
            total_supply: 0,
            holders: 0,
        };

        self.wrapped_tokens.insert(wrapped_address.clone(), token.clone());

        log::info!("Created wrapped token {}", wrapped_address);

        Ok(token)
    }

    /// Get bridge
    pub fn get_bridge(&self, bridge_id: &str) -> Option<Bridge> {
        self.bridges.get(bridge_id).map(|b| b.clone())
    }

    /// Get transfer
    pub fn get_transfer(&self, transfer_hash: &Hash) -> Option<BridgeTransfer> {
        self.transfers.get(transfer_hash).map(|t| t.clone())
    }

    /// Get all bridges
    pub fn get_all_bridges(&self) -> Vec<Bridge> {
        self.bridges.iter().map(|e| e.value().clone()).collect()
    }

    /// Get bridge statistics
    pub fn get_stats(&self) -> BridgeStats {
        let bridges: Vec<_> = self.get_all_bridges();
        let transfers: Vec<_> = self.transfers.iter().map(|e| e.value().clone()).collect();

        BridgeStats {
            total_bridges: bridges.len(),
            active_bridges: bridges
                .iter()
                .filter(|b| b.status == BridgeStatus::Active)
                .count(),
            total_transfers: transfers.len(),
            pending_transfers: transfers
                .iter()
                .filter(|t| t.status == TransferStatus::Pending)
                .count(),
            completed_transfers: transfers
                .iter()
                .filter(|t| t.status == TransferStatus::Completed)
                .count(),
            total_volume: transfers.iter().map(|t| t.amount).sum(),
            wrapped_tokens: self.wrapped_tokens.len(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeStats {
    pub total_bridges: usize,
    pub active_bridges: usize,
    pub total_transfers: usize,
    pub pending_transfers: usize,
    pub completed_transfers: usize,
    pub total_volume: u64,
    pub wrapped_tokens: usize,
}

impl Default for BridgeManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bridge_registration() {
        let manager = BridgeManager::new();

        let bridge = Bridge {
            bridge_id: "eth_bridge".to_string(),
            chain_id: 1,
            chain_name: "Ethereum".to_string(),
            bridge_address: "0x123".to_string(),
            status: BridgeStatus::Active,
            total_locked: 0,
            total_transfers: 0,
            supported_tokens: vec!["ETH".to_string()],
        };

        manager.register_bridge(bridge).unwrap();

        let retrieved = manager.get_bridge("eth_bridge").unwrap();
        assert_eq!(retrieved.chain_name, "Ethereum");
    }

    #[test]
    fn test_deposit() {
        let manager = BridgeManager::new();

        let bridge = Bridge {
            bridge_id: "eth_bridge".to_string(),
            chain_id: 1,
            chain_name: "Ethereum".to_string(),
            bridge_address: "0x123".to_string(),
            status: BridgeStatus::Active,
            total_locked: 0,
            total_transfers: 0,
            supported_tokens: vec!["ETH".to_string()],
        };

        manager.register_bridge(bridge).unwrap();

        let transfer = manager
            .initiate_deposit(
                "eth_bridge".to_string(),
                "sender1".to_string(),
                "recipient1".to_string(),
                "ETH".to_string(),
                1000,
                vec![1, 2, 3],
            )
            .unwrap();

        assert_eq!(transfer.direction, TransferDirection::Deposit);
        assert_eq!(transfer.status, TransferStatus::Pending);
    }

    #[test]
    fn test_wrapped_token() {
        let manager = BridgeManager::new();

        let token = manager.create_wrapped_token(1, "0xABC".to_string()).unwrap();

        assert!(token.wrapped_address.contains("wrapped_"));
        assert_eq!(token.original_chain, 1);
    }
}
