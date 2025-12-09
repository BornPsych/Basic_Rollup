use anyhow::Result;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::hash_utils::Hash;
use crate::types::Transaction;

/// Transaction tracing system for detailed execution analysis
pub struct TransactionTracer {
    traces: Arc<DashMap<Hash, ExecutionTrace>>,
    debug_mode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTrace {
    pub tx_hash: Hash,
    pub steps: Vec<TraceStep>,
    pub total_gas_used: u64,
    pub execution_time_us: u64,
    pub state_accesses: Vec<StateAccess>,
    pub logs: Vec<TraceLog>,
    pub revert_reason: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceStep {
    pub step_number: usize,
    pub operation: String,
    pub gas_cost: u64,
    pub gas_remaining: u64,
    pub stack: Vec<String>,
    pub memory_changes: Vec<MemoryChange>,
    pub storage_changes: Vec<StorageChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateAccess {
    pub access_type: AccessType,
    pub address: String,
    pub key: Option<String>,
    pub value: Option<Vec<u8>>,
    pub gas_cost: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AccessType {
    Read,
    Write,
    Create,
    Delete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryChange {
    pub offset: usize,
    pub size: usize,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageChange {
    pub slot: String,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceLog {
    pub level: TraceLogLevel,
    pub message: String,
    pub step: usize,
    pub gas_used: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TraceLogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

impl TransactionTracer {
    pub fn new(debug_mode: bool) -> Self {
        Self {
            traces: Arc::new(DashMap::new()),
            debug_mode,
        }
    }

    /// Start tracing a transaction
    pub fn start_trace(&self, tx_hash: Hash) -> TraceBuilder {
        TraceBuilder::new(tx_hash, self.debug_mode)
    }

    /// Store completed trace
    pub fn store_trace(&self, trace: ExecutionTrace) {
        self.traces.insert(trace.tx_hash, trace);
    }

    /// Get trace by transaction hash
    pub fn get_trace(&self, tx_hash: &Hash) -> Option<ExecutionTrace> {
        self.traces.get(tx_hash).map(|t| t.clone())
    }

    /// Get all traces
    pub fn get_all_traces(&self) -> Vec<ExecutionTrace> {
        self.traces.iter().map(|e| e.value().clone()).collect()
    }

    /// Clear old traces
    pub fn cleanup(&self, max_traces: usize) {
        if self.traces.len() > max_traces {
            // Remove oldest traces (simplified - in production would sort by timestamp)
            let to_remove = self.traces.len() - max_traces;
            let keys: Vec<_> = self.traces.iter().take(to_remove).map(|e| *e.key()).collect();

            for key in keys {
                self.traces.remove(&key);
            }
        }
    }

    /// Get trace statistics
    pub fn get_stats(&self) -> TraceStats {
        let traces: Vec<_> = self.get_all_traces();

        let total_gas: u64 = traces.iter().map(|t| t.total_gas_used).sum();
        let total_time: u64 = traces.iter().map(|t| t.execution_time_us).sum();

        TraceStats {
            total_traces: traces.len(),
            total_gas_used: total_gas,
            avg_gas_per_trace: if !traces.is_empty() {
                total_gas / traces.len() as u64
            } else {
                0
            },
            avg_execution_time_us: if !traces.is_empty() {
                total_time / traces.len() as u64
            } else {
                0
            },
            traces_with_errors: traces.iter().filter(|t| t.error.is_some()).count(),
        }
    }
}

/// Builder for constructing execution traces
pub struct TraceBuilder {
    tx_hash: Hash,
    steps: Vec<TraceStep>,
    state_accesses: Vec<StateAccess>,
    logs: Vec<TraceLog>,
    start_time: Instant,
    total_gas_used: u64,
    revert_reason: Option<String>,
    error: Option<String>,
    debug_mode: bool,
}

impl TraceBuilder {
    pub fn new(tx_hash: Hash, debug_mode: bool) -> Self {
        Self {
            tx_hash,
            steps: Vec::new(),
            state_accesses: Vec::new(),
            logs: Vec::new(),
            start_time: Instant::now(),
            total_gas_used: 0,
            revert_reason: None,
            error: None,
            debug_mode,
        }
    }

    /// Add a trace step
    pub fn add_step(
        &mut self,
        operation: String,
        gas_cost: u64,
        gas_remaining: u64,
        stack: Vec<String>,
    ) {
        if !self.debug_mode && self.steps.len() > 1000 {
            return; // Limit trace size in production
        }

        self.total_gas_used += gas_cost;

        let step = TraceStep {
            step_number: self.steps.len(),
            operation,
            gas_cost,
            gas_remaining,
            stack,
            memory_changes: Vec::new(),
            storage_changes: Vec::new(),
        };

        self.steps.push(step);
    }

    /// Add state access
    pub fn add_state_access(
        &mut self,
        access_type: AccessType,
        address: String,
        key: Option<String>,
        value: Option<Vec<u8>>,
        gas_cost: u64,
    ) {
        self.state_accesses.push(StateAccess {
            access_type,
            address,
            key,
            value,
            gas_cost,
        });
    }

    /// Add log
    pub fn add_log(&mut self, level: TraceLogLevel, message: String) {
        self.logs.push(TraceLog {
            level,
            message,
            step: self.steps.len(),
            gas_used: self.total_gas_used,
        });
    }

    /// Set revert reason
    pub fn set_revert_reason(&mut self, reason: String) {
        self.revert_reason = Some(reason);
    }

    /// Set error
    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    /// Build final trace
    pub fn build(self) -> ExecutionTrace {
        ExecutionTrace {
            tx_hash: self.tx_hash,
            steps: self.steps,
            total_gas_used: self.total_gas_used,
            execution_time_us: self.start_time.elapsed().as_micros() as u64,
            state_accesses: self.state_accesses,
            logs: self.logs,
            revert_reason: self.revert_reason,
            error: self.error,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceStats {
    pub total_traces: usize,
    pub total_gas_used: u64,
    pub avg_gas_per_trace: u64,
    pub avg_execution_time_us: u64,
    pub traces_with_errors: usize,
}

/// Debug API for advanced transaction inspection
pub struct DebugAPI {
    tracer: Arc<TransactionTracer>,
}

impl DebugAPI {
    pub fn new(tracer: Arc<TransactionTracer>) -> Self {
        Self { tracer }
    }

    /// Get detailed transaction trace
    pub fn debug_transaction(&self, tx_hash: Hash) -> Result<ExecutionTrace> {
        self.tracer
            .get_trace(&tx_hash)
            .ok_or_else(|| anyhow::anyhow!("Trace not found"))
    }

    /// Replay transaction with tracing
    pub fn replay_transaction(&self, tx: Transaction) -> Result<ExecutionTrace> {
        let tx_hash = Hash::new(&bincode::serialize(&tx).unwrap_or_default());
        let mut builder = self.tracer.start_trace(tx_hash);

        // Simplified replay - in production would execute actual transaction
        builder.add_step(
            "CALL".to_string(),
            21000,
            100000,
            vec!["addr1".to_string(), "addr2".to_string()],
        );

        builder.add_state_access(
            AccessType::Read,
            tx.from.clone(),
            Some("balance".to_string()),
            Some(vec![0, 1, 2, 3]),
            100,
        );

        builder.add_log(
            TraceLogLevel::Info,
            "Transaction execution started".to_string(),
        );

        let trace = builder.build();
        self.tracer.store_trace(trace.clone());

        Ok(trace)
    }

    /// Get call stack for transaction
    pub fn get_call_stack(&self, tx_hash: Hash) -> Result<Vec<String>> {
        let trace = self.debug_transaction(tx_hash)?;

        let call_stack: Vec<String> = trace
            .steps
            .iter()
            .filter(|step| step.operation.starts_with("CALL"))
            .map(|step| step.operation.clone())
            .collect();

        Ok(call_stack)
    }

    /// Get state changes for transaction
    pub fn get_state_changes(&self, tx_hash: Hash) -> Result<Vec<StateAccess>> {
        let trace = self.debug_transaction(tx_hash)?;
        Ok(trace.state_accesses.clone())
    }

    /// Get gas usage breakdown
    pub fn get_gas_breakdown(&self, tx_hash: Hash) -> Result<GasBreakdown> {
        let trace = self.debug_transaction(tx_hash)?;

        let mut breakdown = std::collections::HashMap::new();

        for step in &trace.steps {
            *breakdown.entry(step.operation.clone()).or_insert(0u64) += step.gas_cost;
        }

        Ok(GasBreakdown {
            total_gas: trace.total_gas_used,
            breakdown,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasBreakdown {
    pub total_gas: u64,
    pub breakdown: std::collections::HashMap<String, u64>,
}

impl Default for TransactionTracer {
    fn default() -> Self {
        Self::new(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_builder() {
        let tx_hash = Hash::new(b"test");
        let mut builder = TraceBuilder::new(tx_hash, true);

        builder.add_step("ADD".to_string(), 3, 100, vec!["1".to_string(), "2".to_string()]);

        builder.add_state_access(
            AccessType::Read,
            "addr1".to_string(),
            None,
            None,
            100,
        );

        builder.add_log(TraceLogLevel::Info, "Test log".to_string());

        let trace = builder.build();

        assert_eq!(trace.steps.len(), 1);
        assert_eq!(trace.state_accesses.len(), 1);
        assert_eq!(trace.logs.len(), 1);
        assert_eq!(trace.total_gas_used, 3);
    }

    #[test]
    fn test_tracer() {
        let tracer = TransactionTracer::new(true);
        let tx_hash = Hash::new(b"test");

        let mut builder = tracer.start_trace(tx_hash);
        builder.add_step("CALL".to_string(), 700, 10000, vec![]);
        let trace = builder.build();

        tracer.store_trace(trace.clone());

        let retrieved = tracer.get_trace(&tx_hash).unwrap();
        assert_eq!(retrieved.tx_hash, tx_hash);
    }

    #[test]
    fn test_debug_api() {
        let tracer = Arc::new(TransactionTracer::new(true));
        let debug_api = DebugAPI::new(tracer);

        let tx = Transaction {
            from: "addr1".to_string(),
            to: Some("addr2".to_string()),
            value: 100,
            data: vec![],
            nonce: 0,
            gas_limit: 21000,
            max_fee_per_gas: 1,
        };

        let trace = debug_api.replay_transaction(tx).unwrap();
        assert!(trace.total_gas_used > 0);
    }
}
