use anyhow::{anyhow, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::hash_utils::Hash;

/// Governance system for on-chain parameter updates and proposals
pub struct GovernanceSystem {
    proposals: Arc<DashMap<u64, Proposal>>,
    votes: Arc<DashMap<u64, HashMap<String, Vote>>>,
    parameters: Arc<DashMap<String, Parameter>>,
    proposal_counter: AtomicU64,
    min_proposal_stake: u64,
    voting_period: u64, // seconds
    execution_delay: u64, // seconds
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub proposal_id: u64,
    pub proposer: String,
    pub title: String,
    pub description: String,
    pub proposal_type: ProposalType,
    pub status: ProposalStatus,
    pub created_at: u64,
    pub voting_starts_at: u64,
    pub voting_ends_at: u64,
    pub execution_time: Option<u64>,
    pub yes_votes: u64,
    pub no_votes: u64,
    pub abstain_votes: u64,
    pub total_voting_power: u64,
    pub quorum: u64,
    pub approval_threshold: f64, // 0.0 to 1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProposalType {
    ParameterChange {
        parameter_name: String,
        new_value: String,
    },
    EmergencyPause,
    EmergencyUnpause,
    ValidatorManagement {
        action: ValidatorAction,
        validator_address: String,
    },
    TreasurySpend {
        recipient: String,
        amount: u64,
    },
    UpgradeContract {
        contract_address: String,
        new_code_hash: Hash,
    },
    Custom {
        action: String,
        params: HashMap<String, String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ValidatorAction {
    Add,
    Remove,
    UpdateCommission,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProposalStatus {
    Pending,
    Active,
    Passed,
    Rejected,
    Executed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vote {
    pub voter: String,
    pub voting_power: u64,
    pub choice: VoteChoice,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum VoteChoice {
    Yes,
    No,
    Abstain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub value: String,
    pub parameter_type: ParameterType,
    pub min_value: Option<String>,
    pub max_value: Option<String>,
    pub last_updated: u64,
    pub update_history: Vec<ParameterUpdate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParameterType {
    Integer,
    Float,
    Boolean,
    String,
    Address,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterUpdate {
    pub proposal_id: u64,
    pub old_value: String,
    pub new_value: String,
    pub timestamp: u64,
}

impl GovernanceSystem {
    pub fn new(min_proposal_stake: u64, voting_period: u64, execution_delay: u64) -> Self {
        Self {
            proposals: Arc::new(DashMap::new()),
            votes: Arc::new(DashMap::new()),
            parameters: Arc::new(DashMap::new()),
            proposal_counter: AtomicU64::new(0),
            min_proposal_stake,
            voting_period,
            execution_delay,
        }
    }

    /// Create a new proposal
    pub fn create_proposal(
        &self,
        proposer: String,
        title: String,
        description: String,
        proposal_type: ProposalType,
        voting_power: u64,
        quorum: u64,
        approval_threshold: f64,
    ) -> Result<Proposal> {
        if voting_power < self.min_proposal_stake {
            return Err(anyhow!(
                "Insufficient voting power {} < minimum {}",
                voting_power,
                self.min_proposal_stake
            ));
        }

        if approval_threshold < 0.0 || approval_threshold > 1.0 {
            return Err(anyhow!("Approval threshold must be between 0.0 and 1.0"));
        }

        let proposal_id = self.proposal_counter.fetch_add(1, Ordering::SeqCst);
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let voting_starts = now + 86400; // 1 day delay
        let voting_ends = voting_starts + self.voting_period;

        let proposal = Proposal {
            proposal_id,
            proposer,
            title,
            description,
            proposal_type,
            status: ProposalStatus::Pending,
            created_at: now,
            voting_starts_at: voting_starts,
            voting_ends_at: voting_ends,
            execution_time: None,
            yes_votes: 0,
            no_votes: 0,
            abstain_votes: 0,
            total_voting_power: voting_power,
            quorum,
            approval_threshold,
        };

        self.proposals.insert(proposal_id, proposal.clone());
        self.votes.insert(proposal_id, HashMap::new());

        log::info!(
            "Created proposal {} by {} - {}",
            proposal_id,
            proposal.proposer,
            proposal.title
        );

        Ok(proposal)
    }

    /// Cast a vote on a proposal
    pub fn vote(
        &self,
        proposal_id: u64,
        voter: String,
        voting_power: u64,
        choice: VoteChoice,
    ) -> Result<()> {
        let mut proposal = self
            .proposals
            .get_mut(&proposal_id)
            .ok_or_else(|| anyhow!("Proposal not found"))?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        if now < proposal.voting_starts_at {
            return Err(anyhow!("Voting has not started yet"));
        }

        if now > proposal.voting_ends_at {
            return Err(anyhow!("Voting has ended"));
        }

        if proposal.status != ProposalStatus::Pending && proposal.status != ProposalStatus::Active {
            return Err(anyhow!("Proposal is not active"));
        }

        proposal.status = ProposalStatus::Active;

        // Record vote
        let vote = Vote {
            voter: voter.clone(),
            voting_power,
            choice: choice.clone(),
            timestamp: now,
        };

        let mut votes = self.votes.get_mut(&proposal_id).unwrap();

        // Check if already voted
        if let Some(existing_vote) = votes.get(&voter) {
            // Remove old vote
            match existing_vote.choice {
                VoteChoice::Yes => proposal.yes_votes -= existing_vote.voting_power,
                VoteChoice::No => proposal.no_votes -= existing_vote.voting_power,
                VoteChoice::Abstain => proposal.abstain_votes -= existing_vote.voting_power,
            }
        }

        // Add new vote
        match choice {
            VoteChoice::Yes => proposal.yes_votes += voting_power,
            VoteChoice::No => proposal.no_votes += voting_power,
            VoteChoice::Abstain => proposal.abstain_votes += voting_power,
        }

        votes.insert(voter.clone(), vote);

        log::info!(
            "Vote cast on proposal {} by {} - {:?}",
            proposal_id,
            voter,
            choice
        );

        Ok(())
    }

    /// Finalize a proposal after voting ends
    pub fn finalize_proposal(&self, proposal_id: u64) -> Result<ProposalStatus> {
        let mut proposal = self
            .proposals
            .get_mut(&proposal_id)
            .ok_or_else(|| anyhow!("Proposal not found"))?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        if now <= proposal.voting_ends_at {
            return Err(anyhow!("Voting period has not ended yet"));
        }

        if proposal.status == ProposalStatus::Executed
            || proposal.status == ProposalStatus::Cancelled
        {
            return Err(anyhow!("Proposal already finalized"));
        }

        let total_votes = proposal.yes_votes + proposal.no_votes + proposal.abstain_votes;

        // Check quorum
        if total_votes < proposal.quorum {
            proposal.status = ProposalStatus::Rejected;
            log::info!(
                "Proposal {} rejected - quorum not met ({} < {})",
                proposal_id,
                total_votes,
                proposal.quorum
            );
            return Ok(ProposalStatus::Rejected);
        }

        // Check approval threshold
        let approval_rate = if total_votes > 0 {
            proposal.yes_votes as f64 / total_votes as f64
        } else {
            0.0
        };

        if approval_rate >= proposal.approval_threshold {
            proposal.status = ProposalStatus::Passed;
            proposal.execution_time = Some(now + self.execution_delay);

            log::info!(
                "Proposal {} passed ({:.1}% approval)",
                proposal_id,
                approval_rate * 100.0
            );
        } else {
            proposal.status = ProposalStatus::Rejected;

            log::info!(
                "Proposal {} rejected ({:.1}% < {:.1}% threshold)",
                proposal_id,
                approval_rate * 100.0,
                proposal.approval_threshold * 100.0
            );
        }

        Ok(proposal.status.clone())
    }

    /// Execute a passed proposal
    pub fn execute_proposal(&self, proposal_id: u64) -> Result<()> {
        let mut proposal = self
            .proposals
            .get_mut(&proposal_id)
            .ok_or_else(|| anyhow!("Proposal not found"))?;

        if proposal.status != ProposalStatus::Passed {
            return Err(anyhow!("Proposal has not passed"));
        }

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        if let Some(execution_time) = proposal.execution_time {
            if now < execution_time {
                return Err(anyhow!(
                    "Execution delay not met (available at {})",
                    execution_time
                ));
            }
        }

        // Execute based on proposal type
        match &proposal.proposal_type {
            ProposalType::ParameterChange {
                parameter_name,
                new_value,
            } => {
                self.update_parameter(proposal_id, parameter_name, new_value)?;
            }
            ProposalType::EmergencyPause => {
                log::warn!("Emergency pause executed via proposal {}", proposal_id);
            }
            ProposalType::EmergencyUnpause => {
                log::info!("Emergency unpause executed via proposal {}", proposal_id);
            }
            _ => {
                log::info!("Executing proposal {} - {:?}", proposal_id, proposal.proposal_type);
            }
        }

        proposal.status = ProposalStatus::Executed;

        log::info!("Executed proposal {}", proposal_id);

        Ok(())
    }

    /// Register a new parameter
    pub fn register_parameter(
        &self,
        name: String,
        initial_value: String,
        parameter_type: ParameterType,
        min_value: Option<String>,
        max_value: Option<String>,
    ) -> Result<()> {
        if self.parameters.contains_key(&name) {
            return Err(anyhow!("Parameter already exists"));
        }

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        let parameter = Parameter {
            name: name.clone(),
            value: initial_value,
            parameter_type,
            min_value,
            max_value,
            last_updated: now,
            update_history: Vec::new(),
        };

        self.parameters.insert(name.clone(), parameter);

        log::info!("Registered parameter: {}", name);

        Ok(())
    }

    /// Update parameter value (via governance)
    fn update_parameter(&self, proposal_id: u64, name: &str, new_value: &str) -> Result<()> {
        let mut parameter = self
            .parameters
            .get_mut(name)
            .ok_or_else(|| anyhow!("Parameter not found"))?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        let update = ParameterUpdate {
            proposal_id,
            old_value: parameter.value.clone(),
            new_value: new_value.to_string(),
            timestamp: now,
        };

        parameter.value = new_value.to_string();
        parameter.last_updated = now;
        parameter.update_history.push(update);

        log::info!("Updated parameter {} via proposal {}", name, proposal_id);

        Ok(())
    }

    /// Get proposal
    pub fn get_proposal(&self, proposal_id: u64) -> Option<Proposal> {
        self.proposals.get(&proposal_id).map(|p| p.clone())
    }

    /// Get all proposals
    pub fn get_all_proposals(&self) -> Vec<Proposal> {
        self.proposals.iter().map(|e| e.value().clone()).collect()
    }

    /// Get active proposals
    pub fn get_active_proposals(&self) -> Vec<Proposal> {
        self.proposals
            .iter()
            .filter(|e| {
                matches!(
                    e.value().status,
                    ProposalStatus::Pending | ProposalStatus::Active
                )
            })
            .map(|e| e.value().clone())
            .collect()
    }

    /// Get parameter
    pub fn get_parameter(&self, name: &str) -> Option<Parameter> {
        self.parameters.get(name).map(|p| p.clone())
    }

    /// Get governance statistics
    pub fn get_stats(&self) -> GovernanceStats {
        let proposals: Vec<_> = self.get_all_proposals();

        GovernanceStats {
            total_proposals: proposals.len(),
            active_proposals: proposals
                .iter()
                .filter(|p| {
                    matches!(
                        p.status,
                        ProposalStatus::Pending | ProposalStatus::Active
                    )
                })
                .count(),
            passed_proposals: proposals
                .iter()
                .filter(|p| matches!(p.status, ProposalStatus::Passed | ProposalStatus::Executed))
                .count(),
            rejected_proposals: proposals
                .iter()
                .filter(|p| matches!(p.status, ProposalStatus::Rejected))
                .count(),
            total_parameters: self.parameters.len(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceStats {
    pub total_proposals: usize,
    pub active_proposals: usize,
    pub passed_proposals: usize,
    pub rejected_proposals: usize,
    pub total_parameters: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proposal_creation() {
        let gov = GovernanceSystem::new(1000, 86400, 3600);

        let proposal = gov
            .create_proposal(
                "proposer1".to_string(),
                "Test Proposal".to_string(),
                "Description".to_string(),
                ProposalType::ParameterChange {
                    parameter_name: "test_param".to_string(),
                    new_value: "100".to_string(),
                },
                2000,
                500,
                0.6,
            )
            .unwrap();

        assert_eq!(proposal.proposal_id, 0);
        assert_eq!(proposal.status, ProposalStatus::Pending);
    }

    #[test]
    fn test_voting() {
        let gov = GovernanceSystem::new(1000, 86400, 0);

        let mut proposal = gov
            .create_proposal(
                "proposer1".to_string(),
                "Test".to_string(),
                "Desc".to_string(),
                ProposalType::EmergencyPause,
                2000,
                500,
                0.6,
            )
            .unwrap();

        // Override voting period for test
        proposal.voting_starts_at = 0;
        proposal.voting_ends_at = u64::MAX;
        gov.proposals.insert(0, proposal);

        gov.vote(0, "voter1".to_string(), 300, VoteChoice::Yes)
            .unwrap();

        let proposal = gov.get_proposal(0).unwrap();
        assert_eq!(proposal.yes_votes, 300);
    }
}
