use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use parking_lot::RwLock;

/// Fee tier based on transaction priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeeTier {
    Economy,    // Low priority, low fee
    Standard,   // Normal priority, standard fee
    Fast,       // High priority, higher fee
    Instant,    // Urgent priority, highest fee
}

impl FeeTier {
    pub fn multiplier(&self) -> f64 {
        match self {
            FeeTier::Economy => 0.5,
            FeeTier::Standard => 1.0,
            FeeTier::Fast => 2.0,
            FeeTier::Instant => 4.0,
        }
    }
}

/// Gas price oracle for dynamic fee calculation
pub struct GasPriceOracle {
    base_fee: AtomicU64,              // Base fee in lamports
    min_fee: u64,                      // Minimum fee
    max_fee: u64,                      // Maximum fee
    congestion_multiplier: Arc<RwLock<f64>>, // Multiplier based on network congestion
}

impl GasPriceOracle {
    pub fn new(base_fee: u64, min_fee: u64, max_fee: u64) -> Self {
        Self {
            base_fee: AtomicU64::new(base_fee),
            min_fee,
            max_fee,
            congestion_multiplier: Arc::new(RwLock::new(1.0)),
        }
    }

    /// Get current base fee
    pub fn get_base_fee(&self) -> u64 {
        self.base_fee.load(Ordering::Relaxed)
    }

    /// Update base fee based on network conditions
    pub fn update_base_fee(&self, new_base_fee: u64) {
        let clamped_fee = new_base_fee.clamp(self.min_fee, self.max_fee);
        self.base_fee.store(clamped_fee, Ordering::Relaxed);
        log::info!("Updated base fee to {} lamports", clamped_fee);
    }

    /// Calculate fee for a transaction
    pub fn calculate_fee(
        &self,
        compute_units: u64,
        tier: FeeTier,
        data_size: usize,
    ) -> TransactionFee {
        let base_fee = self.get_base_fee();
        let congestion = *self.congestion_multiplier.read();

        // Calculate gas price
        let gas_price = (base_fee as f64 * tier.multiplier() * congestion) as u64;

        // Calculate execution fee (based on compute units)
        let execution_fee = (gas_price * compute_units) / 1_000_000;

        // Calculate data fee (based on data size)
        let data_fee = (data_size as u64 * gas_price) / 10_000;

        // Calculate priority fee
        let priority_fee = match tier {
            FeeTier::Economy => 0,
            FeeTier::Standard => base_fee / 10,
            FeeTier::Fast => base_fee / 5,
            FeeTier::Instant => base_fee / 2,
        };

        let total_fee = execution_fee + data_fee + priority_fee;

        TransactionFee {
            execution_fee,
            data_fee,
            priority_fee,
            total_fee,
            gas_price,
            compute_units,
        }
    }

    /// Update congestion multiplier based on mempool utilization
    pub fn update_congestion(&self, mempool_utilization: f64) {
        let new_multiplier = if mempool_utilization > 0.9 {
            2.0 // Very congested
        } else if mempool_utilization > 0.7 {
            1.5 // Moderately congested
        } else if mempool_utilization > 0.5 {
            1.2 // Slightly congested
        } else {
            1.0 // Normal
        };

        *self.congestion_multiplier.write() = new_multiplier;
        log::debug!("Updated congestion multiplier to {:.2}", new_multiplier);
    }

    /// Get fee estimate for different tiers
    pub fn get_fee_estimates(&self, compute_units: u64, data_size: usize) -> FeeEstimates {
        FeeEstimates {
            economy: self.calculate_fee(compute_units, FeeTier::Economy, data_size),
            standard: self.calculate_fee(compute_units, FeeTier::Standard, data_size),
            fast: self.calculate_fee(compute_units, FeeTier::Fast, data_size),
            instant: self.calculate_fee(compute_units, FeeTier::Instant, data_size),
        }
    }
}

impl Default for GasPriceOracle {
    fn default() -> Self {
        Self::new(
            5000,      // 5000 lamports base fee (0.000005 SOL)
            1000,      // 1000 lamports minimum
            1_000_000, // 1M lamports maximum
        )
    }
}

/// Breakdown of transaction fees
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionFee {
    pub execution_fee: u64,  // Fee for execution (compute)
    pub data_fee: u64,        // Fee for data storage
    pub priority_fee: u64,    // Additional fee for priority
    pub total_fee: u64,       // Total fee
    pub gas_price: u64,       // Effective gas price
    pub compute_units: u64,   // Compute units used
}

/// Fee estimates for all tiers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeEstimates {
    pub economy: TransactionFee,
    pub standard: TransactionFee,
    pub fast: TransactionFee,
    pub instant: TransactionFee,
}

/// Fee market with dynamic pricing
pub struct FeeMarket {
    oracle: Arc<GasPriceOracle>,
    total_fees_collected: AtomicU64,
    total_fees_burned: AtomicU64,
}

impl FeeMarket {
    pub fn new(oracle: Arc<GasPriceOracle>) -> Self {
        Self {
            oracle,
            total_fees_collected: AtomicU64::new(0),
            total_fees_burned: AtomicU64::new(0),
        }
    }

    /// Calculate and collect fee for a transaction
    pub fn collect_fee(
        &self,
        compute_units: u64,
        tier: FeeTier,
        data_size: usize,
    ) -> TransactionFee {
        let fee = self.oracle.calculate_fee(compute_units, tier, data_size);

        // Collect fee
        self.total_fees_collected.fetch_add(fee.total_fee, Ordering::Relaxed);

        // Burn base fee (EIP-1559 style)
        let burn_amount = fee.execution_fee / 2;
        self.total_fees_burned.fetch_add(burn_amount, Ordering::Relaxed);

        log::debug!("Collected fee: {} lamports (burned: {})", fee.total_fee, burn_amount);

        fee
    }

    /// Get total fees collected
    pub fn get_total_fees_collected(&self) -> u64 {
        self.total_fees_collected.load(Ordering::Relaxed)
    }

    /// Get total fees burned
    pub fn get_total_fees_burned(&self) -> u64 {
        self.total_fees_burned.load(Ordering::Relaxed)
    }

    /// Get fee statistics
    pub fn get_stats(&self) -> FeeMarketStats {
        FeeMarketStats {
            total_collected: self.get_total_fees_collected(),
            total_burned: self.get_total_fees_burned(),
            base_fee: self.oracle.get_base_fee(),
            congestion_multiplier: *self.oracle.congestion_multiplier.read(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeeMarketStats {
    pub total_collected: u64,
    pub total_burned: u64,
    pub base_fee: u64,
    pub congestion_multiplier: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fee_calculation() {
        let oracle = GasPriceOracle::default();
        let fee = oracle.calculate_fee(100_000, FeeTier::Standard, 500);

        assert!(fee.total_fee > 0);
        assert!(fee.execution_fee > 0);
        assert_eq!(fee.total_fee, fee.execution_fee + fee.data_fee + fee.priority_fee);
    }

    #[test]
    fn test_fee_tiers() {
        let oracle = GasPriceOracle::default();

        let economy = oracle.calculate_fee(100_000, FeeTier::Economy, 500);
        let instant = oracle.calculate_fee(100_000, FeeTier::Instant, 500);

        assert!(instant.total_fee > economy.total_fee);
    }

    #[test]
    fn test_congestion_multiplier() {
        let oracle = GasPriceOracle::default();

        oracle.update_congestion(0.95); // Very congested
        let fee_high = oracle.calculate_fee(100_000, FeeTier::Standard, 500);

        oracle.update_congestion(0.3); // Low congestion
        let fee_low = oracle.calculate_fee(100_000, FeeTier::Standard, 500);

        assert!(fee_high.total_fee > fee_low.total_fee);
    }

    #[test]
    fn test_fee_market() {
        let oracle = Arc::new(GasPriceOracle::default());
        let market = FeeMarket::new(oracle);

        let fee = market.collect_fee(100_000, FeeTier::Standard, 500);

        assert_eq!(market.get_total_fees_collected(), fee.total_fee);
        assert!(market.get_total_fees_burned() > 0);
    }
}
