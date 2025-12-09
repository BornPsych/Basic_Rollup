use anyhow::{anyhow, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Oracle integration for price feeds and external data
pub struct OracleManager {
    oracles: Arc<DashMap<String, Oracle>>,
    price_feeds: Arc<DashMap<String, PriceFeed>>,
    data_feeds: Arc<DashMap<String, DataFeed>>,
    update_counter: AtomicU64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Oracle {
    pub oracle_id: String,
    pub oracle_type: OracleType,
    pub provider: String,
    pub is_active: bool,
    pub total_updates: u64,
    pub last_update: u64,
    pub reliability_score: f64, // 0.0 to 1.0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OracleType {
    PriceFeed,
    RandomNumber,
    Weather,
    Sports,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceFeed {
    pub symbol: String,
    pub price: u64, // Price in smallest unit (e.g., cents, wei)
    pub decimals: u8,
    pub last_updated: u64,
    pub source: String,
    pub confidence: f64, // 0.0 to 1.0
    pub price_history: Vec<PricePoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricePoint {
    pub timestamp: u64,
    pub price: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataFeed {
    pub feed_id: String,
    pub data_type: String,
    pub value: Vec<u8>,
    pub last_updated: u64,
    pub source: String,
    pub verified: bool,
}

impl OracleManager {
    pub fn new() -> Self {
        Self {
            oracles: Arc::new(DashMap::new()),
            price_feeds: Arc::new(DashMap::new()),
            data_feeds: Arc::new(DashMap::new()),
            update_counter: AtomicU64::new(0),
        }
    }

    /// Register a new oracle
    pub fn register_oracle(&self, oracle: Oracle) -> Result<()> {
        if self.oracles.contains_key(&oracle.oracle_id) {
            return Err(anyhow!("Oracle already registered"));
        }

        log::info!(
            "Registered oracle {} ({})",
            oracle.oracle_id,
            oracle.provider
        );

        self.oracles.insert(oracle.oracle_id.clone(), oracle);
        Ok(())
    }

    /// Update price feed
    pub fn update_price(
        &self,
        oracle_id: String,
        symbol: String,
        price: u64,
        decimals: u8,
        confidence: f64,
    ) -> Result<()> {
        // Verify oracle exists and is active
        let mut oracle = self
            .oracles
            .get_mut(&oracle_id)
            .ok_or_else(|| anyhow!("Oracle not found"))?;

        if !oracle.is_active {
            return Err(anyhow!("Oracle is not active"));
        }

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        // Update or create price feed
        self.price_feeds
            .entry(symbol.clone())
            .and_modify(|feed| {
                // Add to history
                feed.price_history.push(PricePoint {
                    timestamp: now,
                    price: feed.price,
                });

                // Keep only last 100 points
                if feed.price_history.len() > 100 {
                    feed.price_history.remove(0);
                }

                feed.price = price;
                feed.last_updated = now;
                feed.confidence = confidence;
            })
            .or_insert(PriceFeed {
                symbol: symbol.clone(),
                price,
                decimals,
                last_updated: now,
                source: oracle_id.clone(),
                confidence,
                price_history: vec![],
            });

        oracle.total_updates += 1;
        oracle.last_update = now;

        self.update_counter.fetch_add(1, Ordering::Relaxed);

        log::debug!(
            "Updated price for {} to {} (decimals: {}, confidence: {:.2})",
            symbol,
            price,
            decimals,
            confidence
        );

        Ok(())
    }

    /// Update data feed
    pub fn update_data_feed(
        &self,
        oracle_id: String,
        feed_id: String,
        data_type: String,
        value: Vec<u8>,
        verified: bool,
    ) -> Result<()> {
        let mut oracle = self
            .oracles
            .get_mut(&oracle_id)
            .ok_or_else(|| anyhow!("Oracle not found"))?;

        if !oracle.is_active {
            return Err(anyhow!("Oracle is not active"));
        }

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        let feed = DataFeed {
            feed_id: feed_id.clone(),
            data_type,
            value,
            last_updated: now,
            source: oracle_id.clone(),
            verified,
        };

        self.data_feeds.insert(feed_id.clone(), feed);

        oracle.total_updates += 1;
        oracle.last_update = now;

        log::info!("Updated data feed {}", feed_id);

        Ok(())
    }

    /// Get price
    pub fn get_price(&self, symbol: &str) -> Option<u64> {
        self.price_feeds.get(symbol).map(|f| f.price)
    }

    /// Get price feed with details
    pub fn get_price_feed(&self, symbol: &str) -> Option<PriceFeed> {
        self.price_feeds.get(symbol).map(|f| f.clone())
    }

    /// Get price with age check
    pub fn get_fresh_price(&self, symbol: &str, max_age_seconds: u64) -> Result<u64> {
        let feed = self
            .price_feeds
            .get(symbol)
            .ok_or_else(|| anyhow!("Price feed not found"))?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let age = now - feed.last_updated;

        if age > max_age_seconds {
            return Err(anyhow!("Price data is stale ({} seconds old)", age));
        }

        if feed.confidence < 0.7 {
            return Err(anyhow!(
                "Price confidence too low ({:.2})",
                feed.confidence
            ));
        }

        Ok(feed.price)
    }

    /// Get data feed
    pub fn get_data_feed(&self, feed_id: &str) -> Option<DataFeed> {
        self.data_feeds.get(feed_id).map(|f| f.clone())
    }

    /// Get all price feeds
    pub fn get_all_prices(&self) -> Vec<PriceFeed> {
        self.price_feeds.iter().map(|e| e.value().clone()).collect()
    }

    /// Calculate TWAP (Time Weighted Average Price)
    pub fn calculate_twap(&self, symbol: &str, period_seconds: u64) -> Result<u64> {
        let feed = self
            .price_feeds
            .get(symbol)
            .ok_or_else(|| anyhow!("Price feed not found"))?;

        if feed.price_history.is_empty() {
            return Ok(feed.price);
        }

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let cutoff = now - period_seconds;

        let recent_prices: Vec<_> = feed
            .price_history
            .iter()
            .filter(|p| p.timestamp >= cutoff)
            .collect();

        if recent_prices.is_empty() {
            return Ok(feed.price);
        }

        let sum: u64 = recent_prices.iter().map(|p| p.price).sum();
        let avg = sum / recent_prices.len() as u64;

        Ok(avg)
    }

    /// Get oracle statistics
    pub fn get_stats(&self) -> OracleStats {
        let oracles: Vec<_> = self.oracles.iter().map(|e| e.value().clone()).collect();

        OracleStats {
            total_oracles: oracles.len(),
            active_oracles: oracles.iter().filter(|o| o.is_active).count(),
            total_price_feeds: self.price_feeds.len(),
            total_data_feeds: self.data_feeds.len(),
            total_updates: self.update_counter.load(Ordering::Relaxed),
            avg_reliability: if !oracles.is_empty() {
                oracles.iter().map(|o| o.reliability_score).sum::<f64>() / oracles.len() as f64
            } else {
                0.0
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OracleStats {
    pub total_oracles: usize,
    pub active_oracles: usize,
    pub total_price_feeds: usize,
    pub total_data_feeds: usize,
    pub total_updates: u64,
    pub avg_reliability: f64,
}

impl Default for OracleManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oracle_registration() {
        let manager = OracleManager::new();

        let oracle = Oracle {
            oracle_id: "chainlink1".to_string(),
            oracle_type: OracleType::PriceFeed,
            provider: "Chainlink".to_string(),
            is_active: true,
            total_updates: 0,
            last_update: 0,
            reliability_score: 0.95,
        };

        manager.register_oracle(oracle).unwrap();

        let stats = manager.get_stats();
        assert_eq!(stats.total_oracles, 1);
        assert_eq!(stats.active_oracles, 1);
    }

    #[test]
    fn test_price_update() {
        let manager = OracleManager::new();

        let oracle = Oracle {
            oracle_id: "oracle1".to_string(),
            oracle_type: OracleType::PriceFeed,
            provider: "Test".to_string(),
            is_active: true,
            total_updates: 0,
            last_update: 0,
            reliability_score: 1.0,
        };

        manager.register_oracle(oracle).unwrap();

        manager
            .update_price(
                "oracle1".to_string(),
                "ETH/USD".to_string(),
                2000_00,
                2,
                0.95,
            )
            .unwrap();

        let price = manager.get_price("ETH/USD").unwrap();
        assert_eq!(price, 2000_00);
    }

    #[test]
    fn test_fresh_price() {
        let manager = OracleManager::new();

        let oracle = Oracle {
            oracle_id: "oracle1".to_string(),
            oracle_type: OracleType::PriceFeed,
            provider: "Test".to_string(),
            is_active: true,
            total_updates: 0,
            last_update: 0,
            reliability_score: 1.0,
        };

        manager.register_oracle(oracle).unwrap();

        manager
            .update_price(
                "oracle1".to_string(),
                "BTC/USD".to_string(),
                50000_00,
                2,
                0.98,
            )
            .unwrap();

        let price = manager.get_fresh_price("BTC/USD", 300).unwrap();
        assert_eq!(price, 50000_00);
    }

    #[test]
    fn test_twap() {
        let manager = OracleManager::new();

        let oracle = Oracle {
            oracle_id: "oracle1".to_string(),
            oracle_type: OracleType::PriceFeed,
            provider: "Test".to_string(),
            is_active: true,
            total_updates: 0,
            last_update: 0,
            reliability_score: 1.0,
        };

        manager.register_oracle(oracle).unwrap();

        // Update prices multiple times
        for price in [100, 110, 120, 130, 140] {
            manager
                .update_price(
                    "oracle1".to_string(),
                    "TEST/USD".to_string(),
                    price,
                    0,
                    1.0,
                )
                .unwrap();
        }

        let twap = manager.calculate_twap("TEST/USD", 3600).unwrap();
        assert!(twap > 0);
    }
}
