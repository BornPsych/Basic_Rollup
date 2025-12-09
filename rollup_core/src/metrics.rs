use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use dashmap::DashMap;
use chrono::{DateTime, Utc};

/// Comprehensive metrics system for the rollup
pub struct MetricsCollector {
    // Transaction metrics
    total_transactions: AtomicU64,
    successful_transactions: AtomicU64,
    failed_transactions: AtomicU64,

    // Batch metrics
    total_batches: AtomicU64,
    total_settled_batches: AtomicU64,

    // Performance metrics
    avg_tx_processing_time_ms: AtomicU64,
    avg_batch_creation_time_ms: AtomicU64,

    // Network metrics
    total_bytes_processed: AtomicU64,
    total_bytes_settled: AtomicU64,

    // Fee metrics
    total_fees_collected: AtomicU64,
    total_gas_used: AtomicU64,

    // State metrics
    current_state_size: AtomicUsize,
    total_accounts: AtomicUsize,

    // Time series data (hourly)
    hourly_tx_count: Arc<DashMap<String, u64>>,
    hourly_gas_used: Arc<DashMap<String, u64>>,

    // Start time
    start_time: DateTime<Utc>,
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self {
            total_transactions: AtomicU64::new(0),
            successful_transactions: AtomicU64::new(0),
            failed_transactions: AtomicU64::new(0),
            total_batches: AtomicU64::new(0),
            total_settled_batches: AtomicU64::new(0),
            avg_tx_processing_time_ms: AtomicU64::new(0),
            avg_batch_creation_time_ms: AtomicU64::new(0),
            total_bytes_processed: AtomicU64::new(0),
            total_bytes_settled: AtomicU64::new(0),
            total_fees_collected: AtomicU64::new(0),
            total_gas_used: AtomicU64::new(0),
            current_state_size: AtomicUsize::new(0),
            total_accounts: AtomicUsize::new(0),
            hourly_tx_count: Arc::new(DashMap::new()),
            hourly_gas_used: Arc::new(DashMap::new()),
            start_time: Utc::now(),
        }
    }

    // Transaction metrics
    pub fn record_transaction_success(&self, processing_time_ms: u64, gas_used: u64, fee: u64) {
        self.total_transactions.fetch_add(1, Ordering::Relaxed);
        self.successful_transactions.fetch_add(1, Ordering::Relaxed);
        self.total_gas_used.fetch_add(gas_used, Ordering::Relaxed);
        self.total_fees_collected.fetch_add(fee, Ordering::Relaxed);

        // Update average processing time
        let current_avg = self.avg_tx_processing_time_ms.load(Ordering::Relaxed);
        let total_tx = self.total_transactions.load(Ordering::Relaxed);
        let new_avg = ((current_avg * (total_tx - 1)) + processing_time_ms) / total_tx;
        self.avg_tx_processing_time_ms.store(new_avg, Ordering::Relaxed);

        // Record hourly metrics
        let hour_key = Utc::now().format("%Y-%m-%d-%H").to_string();
        self.hourly_tx_count
            .entry(hour_key.clone())
            .and_modify(|count| *count += 1)
            .or_insert(1);
        self.hourly_gas_used
            .entry(hour_key)
            .and_modify(|gas| *gas += gas_used)
            .or_insert(gas_used);
    }

    pub fn record_transaction_failure(&self) {
        self.total_transactions.fetch_add(1, Ordering::Relaxed);
        self.failed_transactions.fetch_add(1, Ordering::Relaxed);
    }

    // Batch metrics
    pub fn record_batch_created(&self, creation_time_ms: u64, batch_size_bytes: u64) {
        self.total_batches.fetch_add(1, Ordering::Relaxed);
        self.total_bytes_processed.fetch_add(batch_size_bytes, Ordering::Relaxed);

        let current_avg = self.avg_batch_creation_time_ms.load(Ordering::Relaxed);
        let total_batches = self.total_batches.load(Ordering::Relaxed);
        let new_avg = ((current_avg * (total_batches - 1)) + creation_time_ms) / total_batches;
        self.avg_batch_creation_time_ms.store(new_avg, Ordering::Relaxed);
    }

    pub fn record_batch_settled(&self, settlement_size_bytes: u64) {
        self.total_settled_batches.fetch_add(1, Ordering::Relaxed);
        self.total_bytes_settled.fetch_add(settlement_size_bytes, Ordering::Relaxed);
    }

    // State metrics
    pub fn update_state_size(&self, size: usize) {
        self.current_state_size.store(size, Ordering::Relaxed);
    }

    pub fn update_account_count(&self, count: usize) {
        self.total_accounts.store(count, Ordering::Relaxed);
    }

    // Get snapshot
    pub fn get_snapshot(&self) -> MetricsSnapshot {
        let uptime_seconds = (Utc::now() - self.start_time).num_seconds() as u64;
        let total_tx = self.total_transactions.load(Ordering::Relaxed);
        let success_tx = self.successful_transactions.load(Ordering::Relaxed);

        MetricsSnapshot {
            uptime_seconds,
            total_transactions: total_tx,
            successful_transactions: success_tx,
            failed_transactions: self.failed_transactions.load(Ordering::Relaxed),
            success_rate: if total_tx > 0 {
                (success_tx as f64 / total_tx as f64 * 100.0) as f32
            } else {
                0.0
            },
            total_batches: self.total_batches.load(Ordering::Relaxed),
            total_settled_batches: self.total_settled_batches.load(Ordering::Relaxed),
            avg_tx_processing_time_ms: self.avg_tx_processing_time_ms.load(Ordering::Relaxed),
            avg_batch_creation_time_ms: self.avg_batch_creation_time_ms.load(Ordering::Relaxed),
            total_bytes_processed: self.total_bytes_processed.load(Ordering::Relaxed),
            total_bytes_settled: self.total_bytes_settled.load(Ordering::Relaxed),
            total_fees_collected: self.total_fees_collected.load(Ordering::Relaxed),
            total_gas_used: self.total_gas_used.load(Ordering::Relaxed),
            current_state_size: self.current_state_size.load(Ordering::Relaxed),
            total_accounts: self.total_accounts.load(Ordering::Relaxed),
            transactions_per_second: if uptime_seconds > 0 {
                total_tx as f64 / uptime_seconds as f64
            } else {
                0.0
            },
        }
    }

    // Get hourly breakdown
    pub fn get_hourly_stats(&self) -> Vec<HourlyStats> {
        let mut stats = Vec::new();

        for entry in self.hourly_tx_count.iter() {
            let hour = entry.key().clone();
            let tx_count = *entry.value();
            let gas_used = self.hourly_gas_used
                .get(&hour)
                .map(|v| *v)
                .unwrap_or(0);

            stats.push(HourlyStats {
                hour,
                transaction_count: tx_count,
                gas_used,
            });
        }

        stats.sort_by(|a, b| a.hour.cmp(&b.hour));
        stats
    }

    // Reset metrics (for testing)
    pub fn reset(&self) {
        self.total_transactions.store(0, Ordering::Relaxed);
        self.successful_transactions.store(0, Ordering::Relaxed);
        self.failed_transactions.store(0, Ordering::Relaxed);
        self.total_batches.store(0, Ordering::Relaxed);
        self.total_settled_batches.store(0, Ordering::Relaxed);
        self.total_bytes_processed.store(0, Ordering::Relaxed);
        self.total_bytes_settled.store(0, Ordering::Relaxed);
        self.total_fees_collected.store(0, Ordering::Relaxed);
        self.total_gas_used.store(0, Ordering::Relaxed);
        self.hourly_tx_count.clear();
        self.hourly_gas_used.clear();
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub uptime_seconds: u64,
    pub total_transactions: u64,
    pub successful_transactions: u64,
    pub failed_transactions: u64,
    pub success_rate: f32,
    pub total_batches: u64,
    pub total_settled_batches: u64,
    pub avg_tx_processing_time_ms: u64,
    pub avg_batch_creation_time_ms: u64,
    pub total_bytes_processed: u64,
    pub total_bytes_settled: u64,
    pub total_fees_collected: u64,
    pub total_gas_used: u64,
    pub current_state_size: usize,
    pub total_accounts: usize,
    pub transactions_per_second: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HourlyStats {
    pub hour: String,
    pub transaction_count: u64,
    pub gas_used: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_collection() {
        let metrics = MetricsCollector::new();

        metrics.record_transaction_success(100, 50000, 1000);
        metrics.record_transaction_success(150, 60000, 1200);
        metrics.record_transaction_failure();

        let snapshot = metrics.get_snapshot();
        assert_eq!(snapshot.total_transactions, 3);
        assert_eq!(snapshot.successful_transactions, 2);
        assert_eq!(snapshot.failed_transactions, 1);
    }

    #[test]
    fn test_batch_metrics() {
        let metrics = MetricsCollector::new();

        metrics.record_batch_created(1000, 50000);
        metrics.record_batch_settled(48000);

        let snapshot = metrics.get_snapshot();
        assert_eq!(snapshot.total_batches, 1);
        assert_eq!(snapshot.total_settled_batches, 1);
    }
}
