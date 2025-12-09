use anyhow::{anyhow, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::hash_utils::Hash;

/// DEX (Decentralized Exchange) integration with AMM and order book
pub struct DEXManager {
    pools: Arc<DashMap<String, LiquidityPool>>,
    orders: Arc<DashMap<u64, Order>>,
    trades: Arc<DashMap<Hash, Trade>>,
    order_counter: AtomicU64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidityPool {
    pub pool_id: String,
    pub token_a: String,
    pub token_b: String,
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub total_liquidity: u64,
    pub fee_rate: f64, // e.g., 0.003 for 0.3%
    pub total_volume: u64,
    pub total_fees_collected: u64,
    pub providers: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub order_id: u64,
    pub trader: String,
    pub order_type: OrderType,
    pub token_in: String,
    pub token_out: String,
    pub amount_in: u64,
    pub min_amount_out: u64,
    pub status: OrderStatus,
    pub created_at: u64,
    pub filled_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OrderType {
    Market,
    Limit { limit_price: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OrderStatus {
    Pending,
    Filled,
    PartiallyFilled,
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub trade_hash: Hash,
    pub pool_id: String,
    pub trader: String,
    pub token_in: String,
    pub token_out: String,
    pub amount_in: u64,
    pub amount_out: u64,
    pub fee_paid: u64,
    pub price: f64,
    pub timestamp: u64,
}

impl DEXManager {
    pub fn new() -> Self {
        Self {
            pools: Arc::new(DashMap::new()),
            orders: Arc::new(DashMap::new()),
            trades: Arc::new(DashMap::new()),
            order_counter: AtomicU64::new(0),
        }
    }

    /// Create a new liquidity pool
    pub fn create_pool(
        &self,
        pool_id: String,
        token_a: String,
        token_b: String,
        initial_reserve_a: u64,
        initial_reserve_b: u64,
        fee_rate: f64,
    ) -> Result<LiquidityPool> {
        if self.pools.contains_key(&pool_id) {
            return Err(anyhow!("Pool already exists"));
        }

        if initial_reserve_a == 0 || initial_reserve_b == 0 {
            return Err(anyhow!("Initial reserves must be non-zero"));
        }

        let total_liquidity = (initial_reserve_a as f64 * initial_reserve_b as f64).sqrt() as u64;

        let pool = LiquidityPool {
            pool_id: pool_id.clone(),
            token_a,
            token_b,
            reserve_a: initial_reserve_a,
            reserve_b: initial_reserve_b,
            total_liquidity,
            fee_rate,
            total_volume: 0,
            total_fees_collected: 0,
            providers: 1,
        };

        self.pools.insert(pool_id.clone(), pool.clone());

        log::info!(
            "Created liquidity pool {} ({}/{})",
            pool_id,
            pool.token_a,
            pool.token_b
        );

        Ok(pool)
    }

    /// Add liquidity to a pool
    pub fn add_liquidity(
        &self,
        pool_id: &str,
        amount_a: u64,
        amount_b: u64,
    ) -> Result<u64> {
        let mut pool = self
            .pools
            .get_mut(pool_id)
            .ok_or_else(|| anyhow!("Pool not found"))?;

        // Calculate liquidity tokens to mint
        let liquidity_minted = if pool.total_liquidity == 0 {
            (amount_a as f64 * amount_b as f64).sqrt() as u64
        } else {
            let liquidity_a = (amount_a as f64 / pool.reserve_a as f64) * pool.total_liquidity as f64;
            let liquidity_b = (amount_b as f64 / pool.reserve_b as f64) * pool.total_liquidity as f64;
            liquidity_a.min(liquidity_b) as u64
        };

        pool.reserve_a += amount_a;
        pool.reserve_b += amount_b;
        pool.total_liquidity += liquidity_minted;
        pool.providers += 1;

        log::info!(
            "Added liquidity to pool {} - {} {} and {} {}",
            pool_id,
            amount_a,
            pool.token_a,
            amount_b,
            pool.token_b
        );

        Ok(liquidity_minted)
    }

    /// Swap tokens using AMM (Automated Market Maker)
    pub fn swap(
        &self,
        pool_id: &str,
        trader: String,
        token_in: String,
        amount_in: u64,
        min_amount_out: u64,
    ) -> Result<Trade> {
        let mut pool = self
            .pools
            .get_mut(pool_id)
            .ok_or_else(|| anyhow!("Pool not found"))?;

        // Determine which token is being swapped
        let (reserve_in, reserve_out, token_out) = if token_in == pool.token_a {
            (pool.reserve_a, pool.reserve_b, pool.token_b.clone())
        } else if token_in == pool.token_b {
            (pool.reserve_b, pool.reserve_a, pool.token_a.clone())
        } else {
            return Err(anyhow!("Token not in pool"));
        };

        // Calculate output amount using constant product formula: x * y = k
        // amount_out = (reserve_out * amount_in) / (reserve_in + amount_in)
        let amount_in_with_fee = (amount_in as f64 * (1.0 - pool.fee_rate)) as u64;
        let amount_out = (reserve_out as f64 * amount_in_with_fee as f64
            / (reserve_in as f64 + amount_in_with_fee as f64)) as u64;

        if amount_out < min_amount_out {
            return Err(anyhow!(
                "Insufficient output amount: {} < {}",
                amount_out,
                min_amount_out
            ));
        }

        let fee_paid = amount_in - amount_in_with_fee;

        // Update reserves
        if token_in == pool.token_a {
            pool.reserve_a += amount_in;
            pool.reserve_b -= amount_out;
        } else {
            pool.reserve_b += amount_in;
            pool.reserve_a -= amount_out;
        }

        pool.total_volume += amount_in;
        pool.total_fees_collected += fee_paid;

        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let trade_hash = Hash::new(&bincode::serialize(&(pool_id, trader.clone(), now)).unwrap_or_default());

        let price = amount_out as f64 / amount_in as f64;

        let trade = Trade {
            trade_hash,
            pool_id: pool_id.to_string(),
            trader,
            token_in,
            token_out,
            amount_in,
            amount_out,
            fee_paid,
            price,
            timestamp: now,
        };

        self.trades.insert(trade_hash, trade.clone());

        log::info!(
            "Swap executed in pool {} - {} {} for {} {}",
            pool_id,
            amount_in,
            trade.token_in,
            amount_out,
            trade.token_out
        );

        Ok(trade)
    }

    /// Place a limit order
    pub fn place_order(
        &self,
        trader: String,
        token_in: String,
        token_out: String,
        amount_in: u64,
        min_amount_out: u64,
        order_type: OrderType,
    ) -> Result<Order> {
        let order_id = self.order_counter.fetch_add(1, Ordering::SeqCst);
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        let order = Order {
            order_id,
            trader,
            order_type,
            token_in,
            token_out,
            amount_in,
            min_amount_out,
            status: OrderStatus::Pending,
            created_at: now,
            filled_at: None,
        };

        self.orders.insert(order_id, order.clone());

        log::info!("Placed order {} - {} for {}", order_id, order.token_in, order.token_out);

        Ok(order)
    }

    /// Get pool
    pub fn get_pool(&self, pool_id: &str) -> Option<LiquidityPool> {
        self.pools.get(pool_id).map(|p| p.clone())
    }

    /// Get quote for swap
    pub fn get_quote(&self, pool_id: &str, token_in: &str, amount_in: u64) -> Result<u64> {
        let pool = self
            .pools
            .get(pool_id)
            .ok_or_else(|| anyhow!("Pool not found"))?;

        let (reserve_in, reserve_out) = if token_in == pool.token_a {
            (pool.reserve_a, pool.reserve_b)
        } else if token_in == pool.token_b {
            (pool.reserve_b, pool.reserve_a)
        } else {
            return Err(anyhow!("Token not in pool"));
        };

        let amount_in_with_fee = (amount_in as f64 * (1.0 - pool.fee_rate)) as u64;
        let amount_out = (reserve_out as f64 * amount_in_with_fee as f64
            / (reserve_in as f64 + amount_in_with_fee as f64)) as u64;

        Ok(amount_out)
    }

    /// Get all pools
    pub fn get_all_pools(&self) -> Vec<LiquidityPool> {
        self.pools.iter().map(|e| e.value().clone()).collect()
    }

    /// Get DEX statistics
    pub fn get_stats(&self) -> DEXStats {
        let pools: Vec<_> = self.get_all_pools();
        let trades: Vec<_> = self.trades.iter().map(|e| e.value().clone()).collect();

        DEXStats {
            total_pools: pools.len(),
            total_liquidity: pools.iter().map(|p| p.total_liquidity).sum(),
            total_volume: pools.iter().map(|p| p.total_volume).sum(),
            total_fees: pools.iter().map(|p| p.total_fees_collected).sum(),
            total_trades: trades.len(),
            total_orders: self.orders.len(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DEXStats {
    pub total_pools: usize,
    pub total_liquidity: u64,
    pub total_volume: u64,
    pub total_fees: u64,
    pub total_trades: usize,
    pub total_orders: usize,
}

impl Default for DEXManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_creation() {
        let dex = DEXManager::new();

        let pool = dex
            .create_pool(
                "ETH/USDC".to_string(),
                "ETH".to_string(),
                "USDC".to_string(),
                1000,
                2000000,
                0.003,
            )
            .unwrap();

        assert_eq!(pool.token_a, "ETH");
        assert_eq!(pool.token_b, "USDC");
        assert!(pool.total_liquidity > 0);
    }

    #[test]
    fn test_swap() {
        let dex = DEXManager::new();

        dex.create_pool(
            "ETH/USDC".to_string(),
            "ETH".to_string(),
            "USDC".to_string(),
            1000,
            2000000,
            0.003,
        )
        .unwrap();

        let trade = dex
            .swap(
                "ETH/USDC",
                "trader1".to_string(),
                "ETH".to_string(),
                10,
                1,
            )
            .unwrap();

        assert_eq!(trade.token_in, "ETH");
        assert_eq!(trade.token_out, "USDC");
        assert!(trade.amount_out > 0);
    }

    #[test]
    fn test_add_liquidity() {
        let dex = DEXManager::new();

        dex.create_pool(
            "ETH/USDC".to_string(),
            "ETH".to_string(),
            "USDC".to_string(),
            1000,
            2000000,
            0.003,
        )
        .unwrap();

        let liquidity = dex.add_liquidity("ETH/USDC", 100, 200000).unwrap();

        assert!(liquidity > 0);

        let pool = dex.get_pool("ETH/USDC").unwrap();
        assert_eq!(pool.reserve_a, 1100);
        assert_eq!(pool.reserve_b, 2200000);
    }

    #[test]
    fn test_quote() {
        let dex = DEXManager::new();

        dex.create_pool(
            "ETH/USDC".to_string(),
            "ETH".to_string(),
            "USDC".to_string(),
            1000,
            2000000,
            0.003,
        )
        .unwrap();

        let quote = dex.get_quote("ETH/USDC", "ETH", 10).unwrap();
        assert!(quote > 0);
    }
}
