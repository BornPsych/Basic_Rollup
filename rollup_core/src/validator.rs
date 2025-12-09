use anyhow::{anyhow, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::hash_utils::Hash;

/// Validator management system with staking, slashing, and rewards
pub struct ValidatorManager {
    validators: Arc<DashMap<String, Validator>>,
    stakes: Arc<DashMap<String, StakeInfo>>,
    slash_events: Arc<DashMap<u64, SlashEvent>>,
    reward_pool: Arc<DashMap<u64, RewardDistribution>>,
    total_stake: AtomicU64,
    min_stake: u64,
    slash_counter: AtomicU64,
    reward_counter: AtomicU64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Validator {
    pub address: String,
    pub public_key: String,
    pub status: ValidatorStatus,
    pub stake_amount: u64,
    pub commission_rate: f64, // 0.0 to 1.0
    pub total_blocks_produced: u64,
    pub total_blocks_missed: u64,
    pub total_rewards_earned: u64,
    pub total_slashed: u64,
    pub registered_at: u64,
    pub last_active: u64,
    pub uptime_percentage: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ValidatorStatus {
    Active,
    Inactive,
    Slashed,
    Jailed { until: u64 },
    Unbonding { unlock_time: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakeInfo {
    pub staker_address: String,
    pub validator_address: String,
    pub amount: u64,
    pub staked_at: u64,
    pub pending_rewards: u64,
    pub unbonding_amount: u64,
    pub unbonding_completion: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashEvent {
    pub event_id: u64,
    pub validator_address: String,
    pub reason: SlashReason,
    pub amount_slashed: u64,
    pub timestamp: u64,
    pub evidence: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SlashReason {
    DoubleSign,
    Downtime,
    InvalidBlock,
    Misbehavior { description: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewardDistribution {
    pub distribution_id: u64,
    pub epoch: u64,
    pub total_rewards: u64,
    pub rewards_per_validator: HashMap<String, u64>,
    pub timestamp: u64,
}

impl ValidatorManager {
    pub fn new(min_stake: u64) -> Self {
        Self {
            validators: Arc::new(DashMap::new()),
            stakes: Arc::new(DashMap::new()),
            slash_events: Arc::new(DashMap::new()),
            reward_pool: Arc::new(DashMap::new()),
            total_stake: AtomicU64::new(0),
            min_stake,
            slash_counter: AtomicU64::new(0),
            reward_counter: AtomicU64::new(0),
        }
    }

    /// Register a new validator
    pub fn register_validator(
        &self,
        address: String,
        public_key: String,
        stake_amount: u64,
        commission_rate: f64,
    ) -> Result<()> {
        if stake_amount < self.min_stake {
            return Err(anyhow!(
                "Stake amount {} is below minimum {}",
                stake_amount,
                self.min_stake
            ));
        }

        if commission_rate < 0.0 || commission_rate > 1.0 {
            return Err(anyhow!("Commission rate must be between 0.0 and 1.0"));
        }

        if self.validators.contains_key(&address) {
            return Err(anyhow!("Validator already registered"));
        }

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        let validator = Validator {
            address: address.clone(),
            public_key,
            status: ValidatorStatus::Active,
            stake_amount,
            commission_rate,
            total_blocks_produced: 0,
            total_blocks_missed: 0,
            total_rewards_earned: 0,
            total_slashed: 0,
            registered_at: now,
            last_active: now,
            uptime_percentage: 100.0,
        };

        self.validators.insert(address.clone(), validator);

        // Add initial stake
        let stake = StakeInfo {
            staker_address: address.clone(),
            validator_address: address.clone(),
            amount: stake_amount,
            staked_at: now,
            pending_rewards: 0,
            unbonding_amount: 0,
            unbonding_completion: None,
        };

        self.stakes.insert(address.clone(), stake);
        self.total_stake.fetch_add(stake_amount, Ordering::Relaxed);

        log::info!(
            "Registered validator {} with stake {}",
            address,
            stake_amount
        );

        Ok(())
    }

    /// Delegate stake to a validator
    pub fn delegate_stake(
        &self,
        staker: String,
        validator_address: String,
        amount: u64,
    ) -> Result<()> {
        if !self.validators.contains_key(&validator_address) {
            return Err(anyhow!("Validator not found"));
        }

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let stake_key = format!("{}:{}", staker, validator_address);

        self.stakes
            .entry(stake_key)
            .and_modify(|stake| {
                stake.amount += amount;
            })
            .or_insert(StakeInfo {
                staker_address: staker.clone(),
                validator_address: validator_address.clone(),
                amount,
                staked_at: now,
                pending_rewards: 0,
                unbonding_amount: 0,
                unbonding_completion: None,
            });

        // Update validator stake
        if let Some(mut validator) = self.validators.get_mut(&validator_address) {
            validator.stake_amount += amount;
        }

        self.total_stake.fetch_add(amount, Ordering::Relaxed);

        log::info!(
            "Delegated {} stake from {} to validator {}",
            amount,
            staker,
            validator_address
        );

        Ok(())
    }

    /// Start unbonding process
    pub fn unbond_stake(
        &self,
        staker: String,
        validator_address: String,
        amount: u64,
    ) -> Result<u64> {
        let stake_key = format!("{}:{}", staker, validator_address);

        let mut stake = self
            .stakes
            .get_mut(&stake_key)
            .ok_or_else(|| anyhow!("Stake not found"))?;

        if stake.amount < amount {
            return Err(anyhow!("Insufficient staked amount"));
        }

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let unbonding_period = 7 * 24 * 3600; // 7 days
        let unlock_time = now + unbonding_period;

        stake.amount -= amount;
        stake.unbonding_amount += amount;
        stake.unbonding_completion = Some(unlock_time);

        // Update validator stake
        if let Some(mut validator) = self.validators.get_mut(&validator_address) {
            validator.stake_amount -= amount;
            validator.status = ValidatorStatus::Unbonding { unlock_time };
        }

        self.total_stake.fetch_sub(amount, Ordering::Relaxed);

        log::info!(
            "Started unbonding {} stake for {} from validator {} (unlocks at {})",
            amount,
            staker,
            validator_address,
            unlock_time
        );

        Ok(unlock_time)
    }

    /// Slash validator for misbehavior
    pub fn slash_validator(
        &self,
        validator_address: String,
        reason: SlashReason,
        slash_percentage: f64,
        evidence: Vec<u8>,
    ) -> Result<SlashEvent> {
        let mut validator = self
            .validators
            .get_mut(&validator_address)
            .ok_or_else(|| anyhow!("Validator not found"))?;

        let slash_amount = (validator.stake_amount as f64 * slash_percentage) as u64;

        validator.stake_amount -= slash_amount;
        validator.total_slashed += slash_amount;
        validator.status = ValidatorStatus::Slashed;

        self.total_stake.fetch_sub(slash_amount, Ordering::Relaxed);

        let event_id = self.slash_counter.fetch_add(1, Ordering::SeqCst);
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        let event = SlashEvent {
            event_id,
            validator_address: validator_address.clone(),
            reason,
            amount_slashed: slash_amount,
            timestamp: now,
            evidence,
        };

        self.slash_events.insert(event_id, event.clone());

        log::warn!(
            "Slashed validator {} by {} ({:.1}%)",
            validator_address,
            slash_amount,
            slash_percentage * 100.0
        );

        Ok(event)
    }

    /// Distribute rewards to validators
    pub fn distribute_rewards(&self, epoch: u64, total_rewards: u64) -> Result<RewardDistribution> {
        let active_validators: Vec<_> = self
            .validators
            .iter()
            .filter(|e| matches!(e.value().status, ValidatorStatus::Active))
            .map(|e| e.value().clone())
            .collect();

        if active_validators.is_empty() {
            return Err(anyhow!("No active validators"));
        }

        let total_stake: u64 = active_validators.iter().map(|v| v.stake_amount).sum();

        if total_stake == 0 {
            return Err(anyhow!("No stake to distribute rewards to"));
        }

        let mut rewards_per_validator = HashMap::new();

        for validator in active_validators {
            // Calculate reward proportional to stake
            let stake_percentage = validator.stake_amount as f64 / total_stake as f64;
            let validator_reward = (total_rewards as f64 * stake_percentage) as u64;

            // Apply commission
            let commission = (validator_reward as f64 * validator.commission_rate) as u64;
            let delegator_rewards = validator_reward - commission;

            rewards_per_validator.insert(validator.address.clone(), validator_reward);

            // Update validator
            if let Some(mut v) = self.validators.get_mut(&validator.address) {
                v.total_rewards_earned += validator_reward;
            }

            // Update stake rewards
            let stake_key = format!("{}:{}", validator.address, validator.address);
            if let Some(mut stake) = self.stakes.get_mut(&stake_key) {
                stake.pending_rewards += delegator_rewards;
            }
        }

        let distribution_id = self.reward_counter.fetch_add(1, Ordering::SeqCst);
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        let distribution = RewardDistribution {
            distribution_id,
            epoch,
            total_rewards,
            rewards_per_validator,
            timestamp: now,
        };

        self.reward_pool.insert(distribution_id, distribution.clone());

        log::info!(
            "Distributed {} rewards to {} validators for epoch {}",
            total_rewards,
            active_validators.len(),
            epoch
        );

        Ok(distribution)
    }

    /// Record block production
    pub fn record_block(&self, validator_address: &str, missed: bool) {
        if let Some(mut validator) = self.validators.get_mut(validator_address) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            validator.last_active = now;

            if missed {
                validator.total_blocks_missed += 1;
            } else {
                validator.total_blocks_produced += 1;
            }

            // Update uptime
            let total_blocks = validator.total_blocks_produced + validator.total_blocks_missed;
            if total_blocks > 0 {
                validator.uptime_percentage =
                    (validator.total_blocks_produced as f64 / total_blocks as f64) * 100.0;
            }
        }
    }

    /// Get validator
    pub fn get_validator(&self, address: &str) -> Option<Validator> {
        self.validators.get(address).map(|v| v.clone())
    }

    /// Get all validators
    pub fn get_all_validators(&self) -> Vec<Validator> {
        self.validators.iter().map(|e| e.value().clone()).collect()
    }

    /// Get active validators
    pub fn get_active_validators(&self) -> Vec<Validator> {
        self.validators
            .iter()
            .filter(|e| matches!(e.value().status, ValidatorStatus::Active))
            .map(|e| e.value().clone())
            .collect()
    }

    /// Get validator statistics
    pub fn get_stats(&self) -> ValidatorStats {
        let all_validators: Vec<_> = self.get_all_validators();

        ValidatorStats {
            total_validators: all_validators.len(),
            active_validators: all_validators
                .iter()
                .filter(|v| matches!(v.status, ValidatorStatus::Active))
                .count(),
            total_stake: self.total_stake.load(Ordering::Relaxed),
            total_slashed: all_validators.iter().map(|v| v.total_slashed).sum(),
            total_rewards_distributed: all_validators.iter().map(|v| v.total_rewards_earned).sum(),
            avg_uptime: if !all_validators.is_empty() {
                all_validators.iter().map(|v| v.uptime_percentage).sum::<f64>()
                    / all_validators.len() as f64
            } else {
                0.0
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorStats {
    pub total_validators: usize,
    pub active_validators: usize,
    pub total_stake: u64,
    pub total_slashed: u64,
    pub total_rewards_distributed: u64,
    pub avg_uptime: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validator_registration() {
        let manager = ValidatorManager::new(1000);

        manager
            .register_validator(
                "val1".to_string(),
                "pubkey1".to_string(),
                2000,
                0.1,
            )
            .unwrap();

        let validator = manager.get_validator("val1").unwrap();
        assert_eq!(validator.stake_amount, 2000);
        assert_eq!(validator.commission_rate, 0.1);
    }

    #[test]
    fn test_stake_delegation() {
        let manager = ValidatorManager::new(1000);

        manager
            .register_validator(
                "val1".to_string(),
                "pubkey1".to_string(),
                2000,
                0.1,
            )
            .unwrap();

        manager
            .delegate_stake("user1".to_string(), "val1".to_string(), 500)
            .unwrap();

        let validator = manager.get_validator("val1").unwrap();
        assert_eq!(validator.stake_amount, 2500);
    }

    #[test]
    fn test_slashing() {
        let manager = ValidatorManager::new(1000);

        manager
            .register_validator(
                "val1".to_string(),
                "pubkey1".to_string(),
                2000,
                0.1,
            )
            .unwrap();

        let slash_event = manager
            .slash_validator(
                "val1".to_string(),
                SlashReason::DoubleSign,
                0.1,
                vec![],
            )
            .unwrap();

        assert_eq!(slash_event.amount_slashed, 200);

        let validator = manager.get_validator("val1").unwrap();
        assert_eq!(validator.stake_amount, 1800);
        assert_eq!(validator.status, ValidatorStatus::Slashed);
    }

    #[test]
    fn test_reward_distribution() {
        let manager = ValidatorManager::new(1000);

        manager
            .register_validator(
                "val1".to_string(),
                "pubkey1".to_string(),
                2000,
                0.1,
            )
            .unwrap();

        manager
            .register_validator(
                "val2".to_string(),
                "pubkey2".to_string(),
                3000,
                0.1,
            )
            .unwrap();

        let distribution = manager.distribute_rewards(1, 1000).unwrap();

        assert_eq!(distribution.rewards_per_validator.len(), 2);
        assert_eq!(distribution.total_rewards, 1000);
    }
}
