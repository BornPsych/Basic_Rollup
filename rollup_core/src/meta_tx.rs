use anyhow::{anyhow, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::hash_utils::Hash;
use crate::types::Transaction;

/// Meta transaction support for gasless transactions and transaction sponsorship
pub struct MetaTransactionManager {
    meta_txs: Arc<DashMap<Hash, MetaTransaction>>,
    sponsors: Arc<DashMap<String, Sponsor>>,
    fee_rebates: Arc<DashMap<String, FeeRebate>>,
    sponsorship_counter: AtomicU64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaTransaction {
    pub meta_tx_hash: Hash,
    pub inner_tx: Transaction,
    pub signature: Vec<u8>,
    pub relayer: String,
    pub sponsor: Option<String>,
    pub gas_paid_by: String,
    pub created_at: u64,
    pub executed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sponsor {
    pub sponsor_id: String,
    pub sponsor_address: String,
    pub budget: u64,
    pub spent: u64,
    pub sponsored_count: u64,
    pub whitelist: Vec<String>, // Whitelisted addresses
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeRebate {
    pub user_address: String,
    pub rebate_percentage: f64, // 0.0 to 1.0
    pub total_rebated: u64,
    pub eligible_until: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SponsorshipRequest {
    pub tx: Transaction,
    pub requester: String,
    pub preferred_sponsor: Option<String>,
}

impl MetaTransactionManager {
    pub fn new() -> Self {
        Self {
            meta_txs: Arc::new(DashMap::new()),
            sponsors: Arc::new(DashMap::new()),
            fee_rebates: Arc::new(DashMap::new()),
            sponsorship_counter: AtomicU64::new(0),
        }
    }

    /// Register a transaction sponsor
    pub fn register_sponsor(&self, sponsor: Sponsor) -> Result<()> {
        if self.sponsors.contains_key(&sponsor.sponsor_id) {
            return Err(anyhow!("Sponsor already registered"));
        }

        log::info!(
            "Registered sponsor {} with budget {}",
            sponsor.sponsor_id,
            sponsor.budget
        );

        self.sponsors.insert(sponsor.sponsor_id.clone(), sponsor);
        Ok(())
    }

    /// Create a meta transaction
    pub fn create_meta_transaction(
        &self,
        tx: Transaction,
        signature: Vec<u8>,
        relayer: String,
    ) -> Result<MetaTransaction> {
        let meta_tx_hash = Hash::new(&bincode::serialize(&(&tx, &signature, &relayer)).unwrap_or_default());
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        let meta_tx = MetaTransaction {
            meta_tx_hash,
            inner_tx: tx,
            signature,
            relayer,
            sponsor: None,
            gas_paid_by: String::new(),
            created_at: now,
            executed: false,
        };

        self.meta_txs.insert(meta_tx_hash, meta_tx.clone());

        log::info!("Created meta transaction {:?}", meta_tx_hash);

        Ok(meta_tx)
    }

    /// Request transaction sponsorship
    pub fn request_sponsorship(
        &self,
        meta_tx_hash: Hash,
        estimated_gas: u64,
    ) -> Result<String> {
        let mut meta_tx = self
            .meta_txs
            .get_mut(&meta_tx_hash)
            .ok_or_else(|| anyhow!("Meta transaction not found"))?;

        // Find an eligible sponsor
        let sponsor_id = self.find_sponsor(&meta_tx.inner_tx.from, estimated_gas)?;

        // Update sponsor budget
        let mut sponsor = self.sponsors.get_mut(&sponsor_id).unwrap();

        if sponsor.spent + estimated_gas > sponsor.budget {
            return Err(anyhow!("Sponsor budget exceeded"));
        }

        sponsor.spent += estimated_gas;
        sponsor.sponsored_count += 1;

        meta_tx.sponsor = Some(sponsor_id.clone());
        meta_tx.gas_paid_by = sponsor.sponsor_address.clone();

        log::info!(
            "Transaction {:?} sponsored by {}",
            meta_tx_hash,
            sponsor_id
        );

        Ok(sponsor_id)
    }

    /// Find an eligible sponsor for a transaction
    fn find_sponsor(&self, user: &str, estimated_gas: u64) -> Result<String> {
        for sponsor_entry in self.sponsors.iter() {
            let sponsor = sponsor_entry.value();

            if !sponsor.is_active {
                continue;
            }

            if sponsor.spent + estimated_gas > sponsor.budget {
                continue;
            }

            // Check whitelist
            if !sponsor.whitelist.is_empty() && !sponsor.whitelist.contains(&user.to_string()) {
                continue;
            }

            return Ok(sponsor.sponsor_id.clone());
        }

        Err(anyhow!("No eligible sponsor found"))
    }

    /// Add fee rebate for a user
    pub fn add_fee_rebate(&self, rebate: FeeRebate) -> Result<()> {
        if rebate.rebate_percentage < 0.0 || rebate.rebate_percentage > 1.0 {
            return Err(anyhow!("Rebate percentage must be between 0.0 and 1.0"));
        }

        self.fee_rebates.insert(rebate.user_address.clone(), rebate);

        log::info!(
            "Added fee rebate for {} - {:.1}%",
            rebate.user_address,
            rebate.rebate_percentage * 100.0
        );

        Ok(())
    }

    /// Calculate fee rebate for a user
    pub fn calculate_rebate(&self, user: &str, fee_paid: u64) -> u64 {
        if let Some(rebate) = self.fee_rebates.get(user) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            if now <= rebate.eligible_until {
                return (fee_paid as f64 * rebate.rebate_percentage) as u64;
            }
        }

        0
    }

    /// Apply fee rebate
    pub fn apply_rebate(&self, user: &str, fee_paid: u64) -> Result<u64> {
        let rebate_amount = self.calculate_rebate(user, fee_paid);

        if rebate_amount > 0 {
            if let Some(mut rebate) = self.fee_rebates.get_mut(user) {
                rebate.total_rebated += rebate_amount;

                log::info!("Applied {} fee rebate to {}", rebate_amount, user);
            }
        }

        Ok(rebate_amount)
    }

    /// Execute a meta transaction
    pub fn execute_meta_transaction(&self, meta_tx_hash: Hash) -> Result<()> {
        let mut meta_tx = self
            .meta_txs
            .get_mut(&meta_tx_hash)
            .ok_or_else(|| anyhow!("Meta transaction not found"))?;

        if meta_tx.executed {
            return Err(anyhow!("Meta transaction already executed"));
        }

        // Verify signature (simplified)
        // In production would verify the signature matches the transaction sender

        meta_tx.executed = true;

        log::info!("Executed meta transaction {:?}", meta_tx_hash);

        Ok(())
    }

    /// Get meta transaction
    pub fn get_meta_transaction(&self, hash: &Hash) -> Option<MetaTransaction> {
        self.meta_txs.get(hash).map(|m| m.clone())
    }

    /// Get sponsor
    pub fn get_sponsor(&self, sponsor_id: &str) -> Option<Sponsor> {
        self.sponsors.get(sponsor_id).map(|s| s.clone())
    }

    /// Get fee rebate
    pub fn get_rebate(&self, user: &str) -> Option<FeeRebate> {
        self.fee_rebates.get(user).map(|r| r.clone())
    }

    /// Get statistics
    pub fn get_stats(&self) -> MetaTxStats {
        let meta_txs: Vec<_> = self.meta_txs.iter().map(|e| e.value().clone()).collect();
        let sponsors: Vec<_> = self.sponsors.iter().map(|e| e.value().clone()).collect();

        MetaTxStats {
            total_meta_txs: meta_txs.len(),
            executed_meta_txs: meta_txs.iter().filter(|m| m.executed).count(),
            total_sponsors: sponsors.len(),
            active_sponsors: sponsors.iter().filter(|s| s.is_active).count(),
            total_sponsored: sponsors.iter().map(|s| s.sponsored_count).sum(),
            total_spent: sponsors.iter().map(|s| s.spent).sum(),
            total_rebates: self.fee_rebates.len(),
            total_rebated: self
                .fee_rebates
                .iter()
                .map(|e| e.value().total_rebated)
                .sum(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaTxStats {
    pub total_meta_txs: usize,
    pub executed_meta_txs: usize,
    pub total_sponsors: usize,
    pub active_sponsors: usize,
    pub total_sponsored: u64,
    pub total_spent: u64,
    pub total_rebates: usize,
    pub total_rebated: u64,
}

impl Default for MetaTransactionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sponsor_registration() {
        let manager = MetaTransactionManager::new();

        let sponsor = Sponsor {
            sponsor_id: "sponsor1".to_string(),
            sponsor_address: "0xSPONSOR".to_string(),
            budget: 100000,
            spent: 0,
            sponsored_count: 0,
            whitelist: vec![],
            is_active: true,
        };

        manager.register_sponsor(sponsor).unwrap();

        let retrieved = manager.get_sponsor("sponsor1").unwrap();
        assert_eq!(retrieved.budget, 100000);
    }

    #[test]
    fn test_meta_transaction() {
        let manager = MetaTransactionManager::new();

        let tx = Transaction {
            from: "user1".to_string(),
            to: Some("user2".to_string()),
            value: 100,
            data: vec![],
            nonce: 0,
            gas_limit: 21000,
            max_fee_per_gas: 1,
        };

        let meta_tx = manager
            .create_meta_transaction(tx, vec![1, 2, 3], "relayer1".to_string())
            .unwrap();

        assert!(!meta_tx.executed);
        assert_eq!(meta_tx.relayer, "relayer1");
    }

    #[test]
    fn test_sponsorship() {
        let manager = MetaTransactionManager::new();

        let sponsor = Sponsor {
            sponsor_id: "sponsor1".to_string(),
            sponsor_address: "0xSPONSOR".to_string(),
            budget: 100000,
            spent: 0,
            sponsored_count: 0,
            whitelist: vec![],
            is_active: true,
        };

        manager.register_sponsor(sponsor).unwrap();

        let tx = Transaction {
            from: "user1".to_string(),
            to: Some("user2".to_string()),
            value: 100,
            data: vec![],
            nonce: 0,
            gas_limit: 21000,
            max_fee_per_gas: 1,
        };

        let meta_tx = manager
            .create_meta_transaction(tx, vec![1, 2, 3], "relayer1".to_string())
            .unwrap();

        let sponsor_id = manager
            .request_sponsorship(meta_tx.meta_tx_hash, 21000)
            .unwrap();

        assert_eq!(sponsor_id, "sponsor1");

        let updated_sponsor = manager.get_sponsor("sponsor1").unwrap();
        assert_eq!(updated_sponsor.spent, 21000);
        assert_eq!(updated_sponsor.sponsored_count, 1);
    }

    #[test]
    fn test_fee_rebate() {
        let manager = MetaTransactionManager::new();

        let rebate = FeeRebate {
            user_address: "user1".to_string(),
            rebate_percentage: 0.5,
            total_rebated: 0,
            eligible_until: u64::MAX,
        };

        manager.add_fee_rebate(rebate).unwrap();

        let rebate_amount = manager.calculate_rebate("user1", 1000);
        assert_eq!(rebate_amount, 500);

        manager.apply_rebate("user1", 1000).unwrap();

        let updated_rebate = manager.get_rebate("user1").unwrap();
        assert_eq!(updated_rebate.total_rebated, 500);
    }
}
