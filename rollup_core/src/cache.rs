use anyhow::Result;
use dashmap::DashMap;
use lru::LruCache;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::hash_utils::Hash;

/// Cache entry with expiration
#[derive(Debug, Clone)]
struct CacheEntry<T> {
    value: T,
    inserted_at: Instant,
    ttl: Option<Duration>,
    hit_count: u64,
}

impl<T> CacheEntry<T> {
    fn new(value: T, ttl: Option<Duration>) -> Self {
        Self {
            value,
            inserted_at: Instant::now(),
            ttl,
            hit_count: 0,
        }
    }

    fn is_expired(&self) -> bool {
        if let Some(ttl) = self.ttl {
            self.inserted_at.elapsed() > ttl
        } else {
            false
        }
    }

    fn hit(&mut self) -> &T {
        self.hit_count += 1;
        &self.value
    }
}

/// Multi-layer cache system
pub struct MultiLayerCache<K, V>
where
    K: std::hash::Hash + Eq + Clone,
    V: Clone,
{
    // L1: Hot cache (LRU, small, fast)
    l1_cache: Arc<Mutex<LruCache<K, CacheEntry<V>>>>,

    // L2: Warm cache (Hash map, larger)
    l2_cache: Arc<DashMap<K, CacheEntry<V>>>,

    // Configuration
    l1_size: usize,
    l2_size: usize,
    default_ttl: Option<Duration>,
}

impl<K, V> MultiLayerCache<K, V>
where
    K: std::hash::Hash + Eq + Clone + std::fmt::Debug,
    V: Clone,
{
    pub fn new(l1_size: usize, l2_size: usize, default_ttl: Option<Duration>) -> Self {
        Self {
            l1_cache: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(l1_size).unwrap()))),
            l2_cache: Arc::new(DashMap::new()),
            l1_size,
            l2_size,
            default_ttl,
        }
    }

    /// Get value from cache
    pub fn get(&self, key: &K) -> Option<V> {
        // Try L1 cache first
        {
            let mut l1 = self.l1_cache.lock();
            if let Some(entry) = l1.get_mut(key) {
                if !entry.is_expired() {
                    return Some(entry.hit().clone());
                } else {
                    l1.pop(key);
                }
            }
        }

        // Try L2 cache
        if let Some(mut entry) = self.l2_cache.get_mut(key) {
            if !entry.is_expired() {
                let value = entry.hit().clone();

                // Promote to L1
                let mut l1 = self.l1_cache.lock();
                l1.put(key.clone(), entry.clone());

                return Some(value);
            } else {
                drop(entry);
                self.l2_cache.remove(key);
            }
        }

        None
    }

    /// Put value into cache
    pub fn put(&self, key: K, value: V) {
        self.put_with_ttl(key, value, self.default_ttl);
    }

    /// Put value with custom TTL
    pub fn put_with_ttl(&self, key: K, value: V, ttl: Option<Duration>) {
        let entry = CacheEntry::new(value, ttl);

        // Always put in L1 (hot cache)
        {
            let mut l1 = self.l1_cache.lock();
            if let Some((evicted_key, evicted_entry)) = l1.push(key.clone(), entry.clone()) {
                // Move evicted entry to L2 if it has been accessed multiple times
                if evicted_entry.hit_count > 1 && self.l2_cache.len() < self.l2_size {
                    self.l2_cache.insert(evicted_key, evicted_entry);
                }
            }
        }
    }

    /// Remove from cache
    pub fn remove(&self, key: &K) -> Option<V> {
        // Remove from L1
        let l1_value = {
            let mut l1 = self.l1_cache.lock();
            l1.pop(key).map(|entry| entry.value)
        };

        // Remove from L2
        let l2_value = self.l2_cache.remove(key).map(|(_, entry)| entry.value);

        l1_value.or(l2_value)
    }

    /// Clear all caches
    pub fn clear(&self) {
        self.l1_cache.lock().clear();
        self.l2_cache.clear();
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            l1_size: self.l1_cache.lock().len(),
            l1_capacity: self.l1_size,
            l2_size: self.l2_cache.len(),
            l2_capacity: self.l2_size,
        }
    }

    /// Cleanup expired entries
    pub fn cleanup_expired(&self) {
        // Cleanup L1
        {
            let mut l1 = self.l1_cache.lock();
            let expired_keys: Vec<_> = l1
                .iter()
                .filter(|(_, entry)| entry.is_expired())
                .map(|(k, _)| k.clone())
                .collect();

            for key in expired_keys {
                l1.pop(&key);
            }
        }

        // Cleanup L2
        let expired_keys: Vec<_> = self.l2_cache
            .iter()
            .filter(|entry| entry.value().is_expired())
            .map(|entry| entry.key().clone())
            .collect();

        for key in expired_keys {
            self.l2_cache.remove(&key);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    pub l1_size: usize,
    pub l1_capacity: usize,
    pub l2_size: usize,
    pub l2_capacity: usize,
}

/// Specialized caches for rollup components
pub struct RollupCaches {
    // Account cache
    pub accounts: MultiLayerCache<String, Vec<u8>>,

    // Transaction cache
    pub transactions: MultiLayerCache<Hash, Vec<u8>>,

    // State root cache
    pub state_roots: MultiLayerCache<u64, Hash>, // batch_id -> state_root

    // Block hash cache (for RPC calls)
    pub blockhashes: MultiLayerCache<String, String>,
}

impl RollupCaches {
    pub fn new() -> Self {
        Self {
            accounts: MultiLayerCache::new(
                1000,                        // L1: 1000 hot accounts
                10_000,                      // L2: 10,000 warm accounts
                Some(Duration::from_secs(300)), // 5 minute TTL
            ),
            transactions: MultiLayerCache::new(
                500,                         // L1: 500 recent transactions
                5_000,                       // L2: 5,000 transactions
                Some(Duration::from_secs(600)), // 10 minute TTL
            ),
            state_roots: MultiLayerCache::new(
                100,   // L1: 100 recent state roots
                1_000, // L2: 1,000 state roots
                None,  // No expiration for state roots
            ),
            blockhashes: MultiLayerCache::new(
                50,                          // L1: 50 recent blockhashes
                500,                         // L2: 500 blockhashes
                Some(Duration::from_secs(120)), // 2 minute TTL
            ),
        }
    }

    /// Get all cache statistics
    pub fn get_all_stats(&self) -> AllCacheStats {
        AllCacheStats {
            accounts: self.accounts.stats(),
            transactions: self.transactions.stats(),
            state_roots: self.state_roots.stats(),
            blockhashes: self.blockhashes.stats(),
        }
    }

    /// Cleanup all expired entries
    pub fn cleanup_all(&self) {
        self.accounts.cleanup_expired();
        self.transactions.cleanup_expired();
        self.state_roots.cleanup_expired();
        self.blockhashes.cleanup_expired();
    }

    /// Clear all caches
    pub fn clear_all(&self) {
        self.accounts.clear();
        self.transactions.clear();
        self.state_roots.clear();
        self.blockhashes.clear();
    }
}

impl Default for RollupCaches {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllCacheStats {
    pub accounts: CacheStats,
    pub transactions: CacheStats,
    pub state_roots: CacheStats,
    pub blockhashes: CacheStats,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multi_layer_cache() {
        let cache = MultiLayerCache::<String, String>::new(2, 5, None);

        cache.put("key1".to_string(), "value1".to_string());
        cache.put("key2".to_string(), "value2".to_string());

        assert_eq!(cache.get(&"key1".to_string()), Some("value1".to_string()));
        assert_eq!(cache.get(&"key2".to_string()), Some("value2".to_string()));
    }

    #[test]
    fn test_cache_eviction() {
        let cache = MultiLayerCache::<String, String>::new(2, 5, None);

        // Fill L1 cache
        cache.put("key1".to_string(), "value1".to_string());
        cache.put("key2".to_string(), "value2".to_string());

        // This should evict key1 to L2
        cache.put("key3".to_string(), "value3".to_string());

        // key1 should still be accessible from L2
        assert!(cache.get(&"key1".to_string()).is_some());
    }

    #[test]
    fn test_cache_ttl() {
        let cache = MultiLayerCache::<String, String>::new(
            10,
            100,
            Some(Duration::from_millis(100)),
        );

        cache.put("key1".to_string(), "value1".to_string());
        assert_eq!(cache.get(&"key1".to_string()), Some("value1".to_string()));

        // Wait for expiration
        std::thread::sleep(Duration::from_millis(150));

        assert_eq!(cache.get(&"key1".to_string()), None);
    }

    #[test]
    fn test_cache_stats() {
        let cache = MultiLayerCache::<String, String>::new(10, 100, None);

        cache.put("key1".to_string(), "value1".to_string());
        cache.put("key2".to_string(), "value2".to_string());

        let stats = cache.stats();
        assert!(stats.l1_size > 0);
    }

    #[test]
    fn test_rollup_caches() {
        let caches = RollupCaches::new();

        caches.accounts.put("account1".to_string(), vec![1, 2, 3]);
        assert!(caches.accounts.get(&"account1".to_string()).is_some());

        let stats = caches.get_all_stats();
        assert!(stats.accounts.l1_size > 0);
    }
}
