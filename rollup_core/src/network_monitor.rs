use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Network status monitor for rollup health and performance
pub struct NetworkMonitor {
    start_time: Instant,
    metrics: Arc<NetworkMetrics>,
    peer_stats: Arc<DashMap<String, PeerStats>>,
    health_history: Arc<DashMap<u64, HealthSnapshot>>,
}

#[derive(Debug, Clone, Default)]
struct NetworkMetrics {
    total_requests: AtomicU64,
    successful_requests: AtomicU64,
    failed_requests: AtomicU64,
    total_bytes_sent: AtomicU64,
    total_bytes_received: AtomicU64,
    active_connections: AtomicUsize,
    peak_connections: AtomicUsize,
    total_gas_processed: AtomicU64,
    total_transactions_processed: AtomicU64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerStats {
    pub peer_id: String,
    pub connected_at: u64,
    pub last_seen: u64,
    pub requests_sent: u64,
    pub requests_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub latency_ms: u64,
    pub is_healthy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthSnapshot {
    pub timestamp: u64,
    pub status: NetworkStatus,
    pub tps: f64,
    pub active_connections: usize,
    pub success_rate: f64,
    pub avg_latency_ms: u64,
    pub network_load: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NetworkStatus {
    Healthy,
    Degraded,
    Critical,
    Offline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkStats {
    pub uptime_seconds: u64,
    pub status: NetworkStatus,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub success_rate: f64,
    pub total_bytes_sent: u64,
    pub total_bytes_received: u64,
    pub active_connections: usize,
    pub peak_connections: usize,
    pub total_peers: usize,
    pub healthy_peers: usize,
    pub transactions_per_second: f64,
    pub avg_latency_ms: u64,
    pub network_load: f64,
}

impl NetworkMonitor {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            metrics: Arc::new(NetworkMetrics::default()),
            peer_stats: Arc::new(DashMap::new()),
            health_history: Arc::new(DashMap::new()),
        }
    }

    /// Record a successful request
    pub fn record_request(&self, bytes_sent: u64, bytes_received: u64, success: bool) {
        self.metrics.total_requests.fetch_add(1, Ordering::Relaxed);

        if success {
            self.metrics.successful_requests.fetch_add(1, Ordering::Relaxed);
        } else {
            self.metrics.failed_requests.fetch_add(1, Ordering::Relaxed);
        }

        self.metrics.total_bytes_sent.fetch_add(bytes_sent, Ordering::Relaxed);
        self.metrics.total_bytes_received.fetch_add(bytes_received, Ordering::Relaxed);
    }

    /// Record transaction processing
    pub fn record_transaction(&self, gas_used: u64) {
        self.metrics.total_transactions_processed.fetch_add(1, Ordering::Relaxed);
        self.metrics.total_gas_processed.fetch_add(gas_used, Ordering::Relaxed);
    }

    /// Update active connections
    pub fn set_active_connections(&self, count: usize) {
        self.metrics.active_connections.store(count, Ordering::Relaxed);

        // Update peak
        let current_peak = self.metrics.peak_connections.load(Ordering::Relaxed);
        if count > current_peak {
            self.metrics.peak_connections.store(count, Ordering::Relaxed);
        }
    }

    /// Register or update peer
    pub fn update_peer(&self, peer_id: String, latency_ms: u64, is_healthy: bool) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

        self.peer_stats.entry(peer_id.clone()).and_modify(|stats| {
            stats.last_seen = now;
            stats.latency_ms = latency_ms;
            stats.is_healthy = is_healthy;
        }).or_insert(PeerStats {
            peer_id,
            connected_at: now,
            last_seen: now,
            requests_sent: 0,
            requests_received: 0,
            bytes_sent: 0,
            bytes_received: 0,
            latency_ms,
            is_healthy,
        });
    }

    /// Record peer communication
    pub fn record_peer_communication(&self, peer_id: &str, bytes_sent: u64, bytes_received: u64, outgoing: bool) {
        if let Some(mut stats) = self.peer_stats.get_mut(peer_id) {
            if outgoing {
                stats.requests_sent += 1;
                stats.bytes_sent += bytes_sent;
            } else {
                stats.requests_received += 1;
                stats.bytes_received += bytes_received;
            }
        }
    }

    /// Get current network status
    pub fn get_status(&self) -> NetworkStatus {
        let total = self.metrics.total_requests.load(Ordering::Relaxed);
        let failed = self.metrics.failed_requests.load(Ordering::Relaxed);
        let active = self.metrics.active_connections.load(Ordering::Relaxed);

        if total == 0 {
            return NetworkStatus::Healthy;
        }

        let failure_rate = failed as f64 / total as f64;

        if active == 0 {
            NetworkStatus::Offline
        } else if failure_rate > 0.5 {
            NetworkStatus::Critical
        } else if failure_rate > 0.2 {
            NetworkStatus::Degraded
        } else {
            NetworkStatus::Healthy
        }
    }

    /// Get network statistics
    pub fn get_stats(&self) -> NetworkStats {
        let uptime = self.start_time.elapsed().as_secs();
        let total_requests = self.metrics.total_requests.load(Ordering::Relaxed);
        let successful = self.metrics.successful_requests.load(Ordering::Relaxed);
        let failed = self.metrics.failed_requests.load(Ordering::Relaxed);
        let total_txs = self.metrics.total_transactions_processed.load(Ordering::Relaxed);

        let success_rate = if total_requests > 0 {
            successful as f64 / total_requests as f64
        } else {
            1.0
        };

        let tps = if uptime > 0 {
            total_txs as f64 / uptime as f64
        } else {
            0.0
        };

        let peers: Vec<_> = self.peer_stats.iter().map(|e| e.value().clone()).collect();
        let healthy_peers = peers.iter().filter(|p| p.is_healthy).count();
        let avg_latency = if !peers.is_empty() {
            peers.iter().map(|p| p.latency_ms).sum::<u64>() / peers.len() as u64
        } else {
            0
        };

        // Calculate network load (0.0 to 1.0)
        let active = self.metrics.active_connections.load(Ordering::Relaxed);
        let peak = self.metrics.peak_connections.load(Ordering::Relaxed).max(1);
        let network_load = active as f64 / peak as f64;

        NetworkStats {
            uptime_seconds: uptime,
            status: self.get_status(),
            total_requests,
            successful_requests: successful,
            failed_requests: failed,
            success_rate,
            total_bytes_sent: self.metrics.total_bytes_sent.load(Ordering::Relaxed),
            total_bytes_received: self.metrics.total_bytes_received.load(Ordering::Relaxed),
            active_connections: active,
            peak_connections: peak,
            total_peers: peers.len(),
            healthy_peers,
            transactions_per_second: tps,
            avg_latency_ms: avg_latency,
            network_load,
        }
    }

    /// Take a health snapshot
    pub fn snapshot_health(&self) {
        let stats = self.get_stats();
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

        let snapshot = HealthSnapshot {
            timestamp,
            status: stats.status,
            tps: stats.transactions_per_second,
            active_connections: stats.active_connections,
            success_rate: stats.success_rate,
            avg_latency_ms: stats.avg_latency_ms,
            network_load: stats.network_load,
        };

        self.health_history.insert(timestamp, snapshot);

        // Keep only last 24 hours
        let cutoff = timestamp.saturating_sub(86400);
        let old_keys: Vec<_> = self.health_history
            .iter()
            .filter(|e| *e.key() < cutoff)
            .map(|e| *e.key())
            .collect();

        for key in old_keys {
            self.health_history.remove(&key);
        }
    }

    /// Get health history
    pub fn get_health_history(&self, duration_seconds: u64) -> Vec<HealthSnapshot> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let cutoff = now.saturating_sub(duration_seconds);

        let mut snapshots: Vec<_> = self.health_history
            .iter()
            .filter(|e| *e.key() >= cutoff)
            .map(|e| e.value().clone())
            .collect();

        snapshots.sort_by_key(|s| s.timestamp);
        snapshots
    }

    /// Get peer statistics
    pub fn get_peer_stats(&self) -> Vec<PeerStats> {
        self.peer_stats.iter().map(|e| e.value().clone()).collect()
    }

    /// Check if network is healthy
    pub fn is_healthy(&self) -> bool {
        matches!(self.get_status(), NetworkStatus::Healthy)
    }

    /// Cleanup stale peers
    pub fn cleanup_stale_peers(&self, timeout_seconds: u64) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let cutoff = now.saturating_sub(timeout_seconds);

        let stale_peers: Vec<_> = self.peer_stats
            .iter()
            .filter(|e| e.value().last_seen < cutoff)
            .map(|e| e.key().clone())
            .collect();

        for peer_id in stale_peers {
            self.peer_stats.remove(&peer_id);
            log::info!("Removed stale peer: {}", peer_id);
        }
    }
}

impl Default for NetworkMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_monitor() {
        let monitor = NetworkMonitor::new();

        monitor.record_request(100, 200, true);
        monitor.record_request(150, 250, true);
        monitor.record_request(120, 220, false);

        let stats = monitor.get_stats();
        assert_eq!(stats.total_requests, 3);
        assert_eq!(stats.successful_requests, 2);
        assert_eq!(stats.failed_requests, 1);
        assert!(stats.success_rate > 0.6);
    }

    #[test]
    fn test_peer_tracking() {
        let monitor = NetworkMonitor::new();

        monitor.update_peer("peer1".to_string(), 50, true);
        monitor.update_peer("peer2".to_string(), 100, true);

        monitor.record_peer_communication("peer1", 1000, 2000, true);

        let peers = monitor.get_peer_stats();
        assert_eq!(peers.len(), 2);

        let peer1 = peers.iter().find(|p| p.peer_id == "peer1").unwrap();
        assert_eq!(peer1.requests_sent, 1);
        assert_eq!(peer1.bytes_sent, 1000);
    }

    #[test]
    fn test_health_snapshot() {
        let monitor = NetworkMonitor::new();

        monitor.record_request(100, 200, true);
        monitor.snapshot_health();

        let history = monitor.get_health_history(3600);
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn test_network_status() {
        let monitor = NetworkMonitor::new();

        // All successful
        for _ in 0..10 {
            monitor.record_request(100, 100, true);
        }
        monitor.set_active_connections(5);

        assert_eq!(monitor.get_status(), NetworkStatus::Healthy);

        // High failure rate
        for _ in 0..20 {
            monitor.record_request(100, 100, false);
        }

        assert_eq!(monitor.get_status(), NetworkStatus::Critical);
    }
}
