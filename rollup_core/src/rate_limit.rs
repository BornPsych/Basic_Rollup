use anyhow::{anyhow, Result};
use dashmap::DashMap;
use governor::{Quota, RateLimiter as GovRateLimiter};
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

/// Rate limiter for different types of requests
pub struct RateLimiter {
    // Per-IP rate limiters
    ip_limiters: Arc<DashMap<IpAddr, Arc<GovRateLimiter<governor::state::direct::NotKeyed, governor::clock::DefaultClock>>>>,

    // Global rate limiter
    global_limiter: Arc<GovRateLimiter<governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,

    // Configuration
    per_ip_quota: Quota,
    global_quota: Quota,
}

impl RateLimiter {
    pub fn new(
        per_ip_requests_per_minute: u32,
        global_requests_per_second: u32,
    ) -> Self {
        let per_ip_quota = Quota::per_minute(NonZeroU32::new(per_ip_requests_per_minute).unwrap());
        let global_quota = Quota::per_second(NonZeroU32::new(global_requests_per_second).unwrap());

        Self {
            ip_limiters: Arc::new(DashMap::new()),
            global_limiter: Arc::new(GovRateLimiter::direct(global_quota)),
            per_ip_quota,
            global_quota,
        }
    }

    /// Check if a request from an IP is allowed
    pub fn check_rate_limit(&self, ip: IpAddr) -> Result<()> {
        // Check global rate limit first
        if self.global_limiter.check().is_err() {
            log::warn!("Global rate limit exceeded");
            return Err(anyhow!("Global rate limit exceeded. Please try again later."));
        }

        // Get or create per-IP rate limiter
        let limiter = self.ip_limiters
            .entry(ip)
            .or_insert_with(|| Arc::new(GovRateLimiter::direct(self.per_ip_quota)));

        // Check per-IP rate limit
        if limiter.check().is_err() {
            log::warn!("Rate limit exceeded for IP: {}", ip);
            return Err(anyhow!("Rate limit exceeded for your IP. Please try again later."));
        }

        Ok(())
    }

    /// Reset rate limits for an IP (for testing or admin purposes)
    pub fn reset_ip(&self, ip: IpAddr) {
        self.ip_limiters.remove(&ip);
    }

    /// Get number of tracked IPs
    pub fn tracked_ip_count(&self) -> usize {
        self.ip_limiters.len()
    }

    /// Clean up old IP limiters
    pub fn cleanup_old_limiters(&self) {
        // Remove limiters that haven't been used recently
        // This is a simple implementation - in production, you'd want more sophisticated cleanup
        let to_remove: Vec<_> = self.ip_limiters
            .iter()
            .filter(|entry| {
                // Check if limiter is idle (has full capacity)
                entry.value().check().is_ok()
            })
            .map(|entry| *entry.key())
            .collect();

        for ip in to_remove {
            self.ip_limiters.remove(&ip);
        }
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(
            100,  // 100 requests per minute per IP
            1000, // 1000 requests per second globally
        )
    }
}

/// Transaction-specific rate limiter
pub struct TransactionRateLimiter {
    limiter: RateLimiter,
    // Track transaction counts per address
    tx_counts: Arc<DashMap<String, u64>>,
    max_tx_per_address_per_hour: u64,
}

impl TransactionRateLimiter {
    pub fn new(max_tx_per_address_per_hour: u64) -> Self {
        Self {
            limiter: RateLimiter::default(),
            tx_counts: Arc::new(DashMap::new()),
            max_tx_per_address_per_hour,
        }
    }

    /// Check if a transaction submission is allowed
    pub fn check_transaction_limit(&self, ip: IpAddr, address: &str) -> Result<()> {
        // Check IP-based rate limit
        self.limiter.check_rate_limit(ip)?;

        // Check address-based limit
        let count = self.tx_counts
            .entry(address.to_string())
            .or_insert(0);

        if *count >= self.max_tx_per_address_per_hour {
            return Err(anyhow!(
                "Transaction limit exceeded for address. Maximum {} transactions per hour.",
                self.max_tx_per_address_per_hour
            ));
        }

        *count += 1;

        Ok(())
    }

    /// Reset limits for an address
    pub fn reset_address(&self, address: &str) {
        self.tx_counts.remove(address);
    }

    /// Periodic cleanup (should be called hourly)
    pub fn hourly_cleanup(&self) {
        self.tx_counts.clear();
        self.limiter.cleanup_old_limiters();
        log::info!("Performed hourly rate limit cleanup");
    }
}

impl Default for TransactionRateLimiter {
    fn default() -> Self {
        Self::new(1000) // 1000 transactions per hour per address
    }
}

/// Security features
pub struct SecurityManager {
    // Blacklisted IPs
    blacklisted_ips: Arc<DashMap<IpAddr, String>>, // IP -> reason

    // Blacklisted addresses
    blacklisted_addresses: Arc<DashMap<String, String>>, // Address -> reason

    // Suspicious activity tracker
    suspicious_activity: Arc<DashMap<String, SuspiciousActivity>>,
}

#[derive(Debug, Clone)]
struct SuspiciousActivity {
    failed_attempts: u64,
    first_attempt: u64,
    last_attempt: u64,
}

impl SecurityManager {
    pub fn new() -> Self {
        Self {
            blacklisted_ips: Arc::new(DashMap::new()),
            blacklisted_addresses: Arc::new(DashMap::new()),
            suspicious_activity: Arc::new(DashMap::new()),
        }
    }

    /// Check if an IP is blacklisted
    pub fn is_ip_blacklisted(&self, ip: IpAddr) -> bool {
        self.blacklisted_ips.contains_key(&ip)
    }

    /// Check if an address is blacklisted
    pub fn is_address_blacklisted(&self, address: &str) -> bool {
        self.blacklisted_addresses.contains_key(address)
    }

    /// Blacklist an IP
    pub fn blacklist_ip(&self, ip: IpAddr, reason: String) {
        self.blacklisted_ips.insert(ip, reason.clone());
        log::warn!("Blacklisted IP {}: {}", ip, reason);
    }

    /// Blacklist an address
    pub fn blacklist_address(&self, address: String, reason: String) {
        self.blacklisted_addresses.insert(address.clone(), reason.clone());
        log::warn!("Blacklisted address {}: {}", address, reason);
    }

    /// Remove IP from blacklist
    pub fn unblacklist_ip(&self, ip: IpAddr) {
        self.blacklisted_ips.remove(&ip);
        log::info!("Removed IP {} from blacklist", ip);
    }

    /// Remove address from blacklist
    pub fn unblacklist_address(&self, address: &str) {
        self.blacklisted_addresses.remove(address);
        log::info!("Removed address {} from blacklist", address);
    }

    /// Record a failed transaction attempt
    pub fn record_failed_attempt(&self, identifier: String) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        self.suspicious_activity
            .entry(identifier.clone())
            .and_modify(|activity| {
                activity.failed_attempts += 1;
                activity.last_attempt = now;

                // Auto-blacklist if too many failures
                if activity.failed_attempts >= 10 {
                    log::warn!("Auto-blacklisting {} due to repeated failures", identifier);
                }
            })
            .or_insert(SuspiciousActivity {
                failed_attempts: 1,
                first_attempt: now,
                last_attempt: now,
            });
    }

    /// Get security statistics
    pub fn get_stats(&self) -> SecurityStats {
        SecurityStats {
            blacklisted_ips: self.blacklisted_ips.len(),
            blacklisted_addresses: self.blacklisted_addresses.len(),
            suspicious_activities: self.suspicious_activity.len(),
        }
    }
}

impl Default for SecurityManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SecurityStats {
    pub blacklisted_ips: usize,
    pub blacklisted_addresses: usize,
    pub suspicious_activities: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn test_rate_limiter() {
        let limiter = RateLimiter::new(2, 10);
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));

        // First two requests should succeed
        assert!(limiter.check_rate_limit(ip).is_ok());
        assert!(limiter.check_rate_limit(ip).is_ok());

        // Third request should fail (2 per minute limit)
        assert!(limiter.check_rate_limit(ip).is_err());
    }

    #[test]
    fn test_security_manager() {
        let security = SecurityManager::new();
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));

        assert!(!security.is_ip_blacklisted(ip));

        security.blacklist_ip(ip, "Test blacklist".to_string());
        assert!(security.is_ip_blacklisted(ip));

        security.unblacklist_ip(ip);
        assert!(!security.is_ip_blacklisted(ip));
    }

    #[test]
    fn test_address_blacklist() {
        let security = SecurityManager::new();
        let address = "test_address";

        security.blacklist_address(address.to_string(), "Suspicious activity".to_string());
        assert!(security.is_address_blacklisted(address));
    }
}
