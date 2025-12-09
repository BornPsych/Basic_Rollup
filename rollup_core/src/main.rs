use std::thread;
use std::sync::Arc;

use actix_web::{web, App, HttpServer};
use tokio::runtime::Builder;

mod cache;
mod checkpoint;
mod compression;
mod data_availability;
mod events;
mod fees;
mod frontend;
mod hash_utils;
mod mempool;
mod merkle;
mod metrics;
mod rate_limit;
mod rollupdb;
mod sequencer;
mod settle;
mod state;
mod types;

// Advanced features (50-feature implementation)
mod websocket;
mod admin;
mod replay_protection;
mod fraud_proofs;
mod query_engine;
mod snapshot;
mod parallel_executor;
mod batching;
mod simulation;
mod network_monitor;
mod validator;
mod governance;
mod emergency;
mod tracing;
mod profiler;
mod contracts;
mod bridge;
mod oracle;
mod dex;
mod meta_tx;
mod tx_pool;

use events::EventBus;
use fees::{FeeMarket, GasPriceOracle};
use mempool::Mempool;
use metrics::MetricsCollector;
use rate_limit::TransactionRateLimiter;
use cache::RollupCaches;

fn main() {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    log::info!("============================================");
    log::info!("   Solana SVM Rollup - Advanced Features");
    log::info!("============================================");
    log::info!("Version: 0.2.0");
    log::info!("Features:");
    log::info!("  ✓ Transaction Mempool with Prioritization");
    log::info!("  ✓ Dynamic Fee Market");
    log::info!("  ✓ Comprehensive Metrics");
    log::info!("  ✓ Event System");
    log::info!("  ✓ Rate Limiting & Security");
    log::info!("  ✓ Checkpointing & Recovery");
    log::info!("  ✓ Batch Compression");
    log::info!("  ✓ Multi-Layer Caching");
    log::info!("  ✓ Data Availability Layer");
    log::info!("============================================");

    // Initialize shared components
    let mempool = Arc::new(Mempool::new(10000)); // 10k transaction capacity
    let metrics = Arc::new(MetricsCollector::new());
    let event_bus = Arc::new(EventBus::new(5000)); // Keep last 5000 events
    let gas_oracle = Arc::new(GasPriceOracle::default());
    let fee_market = Arc::new(FeeMarket::new(gas_oracle.clone()));
    let rate_limiter = Arc::new(TransactionRateLimiter::default());
    let caches = Arc::new(RollupCaches::new());

    log::info!("Initialized shared components");
    log::info!("  - Mempool capacity: {}", 10000);
    log::info!("  - Event history: {} events", 5000);
    log::info!("  - Cache layers: L1 + L2");

    // Create communication channels
    let (sequencer_sender, sequencer_receiver) = crossbeam::channel::unbounded();
    let (rollupdb_sender, rollupdb_receiver) = crossbeam::channel::unbounded();
    let (frontend_sender, frontend_receiver) = async_channel::unbounded();

    // Clone for threads
    let mempool_clone = mempool.clone();
    let metrics_clone = metrics.clone();
    let event_bus_clone = event_bus.clone();
    let fee_market_clone = fee_market.clone();

    // Spawn sequencer and database thread
    log::info!("Starting sequencer and database threads...");
    let _processing_thread = thread::spawn(move || {
        let rt = Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap();

        // Run sequencer
        let sequencer_handle = thread::spawn(move || {
            if let Err(e) = sequencer::run(
                sequencer_receiver,
                rollupdb_sender,
                mempool_clone,
                metrics_clone,
                event_bus_clone,
                fee_market_clone,
            ) {
                log::error!("Sequencer error: {:?}", e);
            }
        });

        // Run rollup DB
        rt.block_on(async {
            rollupdb::RollupDB::run(rollupdb_receiver, frontend_sender).await;
        });

        sequencer_handle.join().unwrap();
    });

    // Spawn periodic cleanup thread
    let caches_cleanup = caches.clone();
    let rate_limiter_cleanup = rate_limiter.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(std::time::Duration::from_secs(3600)); // Every hour
            log::info!("Running periodic cleanup...");
            caches_cleanup.cleanup_all();
            rate_limiter_cleanup.hourly_cleanup();
        }
    });

    // Spawn HTTP server thread
    log::info!("Starting HTTP server on http://127.0.0.1:8080...");
    let server_thread = thread::spawn(move || {
        let rt = Builder::new_multi_thread()
            .worker_threads(4)
            .enable_io()
            .build()
            .unwrap();

        rt.block_on(async {
            HttpServer::new(move || {
                App::new()
                    .app_data(web::Data::new(sequencer_sender.clone()))
                    .app_data(web::Data::new(rollupdb_sender.clone()))
                    .app_data(web::Data::new(frontend_receiver.clone()))
                    .app_data(web::Data::new(mempool.clone()))
                    .app_data(web::Data::new(metrics.clone()))
                    .app_data(web::Data::new(event_bus.clone()))
                    .app_data(web::Data::new(fee_market.clone()))
                    .app_data(web::Data::new(rate_limiter.clone()))
                    .app_data(web::Data::new(caches.clone()))
                    // Public endpoints
                    .route("/", web::get().to(frontend::test))
                    .route("/health", web::get().to(frontend::health_check))
                    .route("/stats", web::get().to(frontend::get_stats))
                    .route("/metrics", web::get().to(frontend::get_metrics))
                    .route("/fees", web::get().to(frontend::get_fee_estimates))
                    .route("/events", web::get().to(frontend::get_recent_events))
                    .route("/cache/stats", web::get().to(frontend::get_cache_stats))
                    .route(
                        "/submit_transaction",
                        web::post().to(frontend::submit_transaction),
                    )
                    .route(
                        "/get_transaction",
                        web::post().to(frontend::get_transaction),
                    )
            })
            .worker_max_blocking_threads(2)
            .bind("127.0.0.1:8080")
            .unwrap()
            .run()
            .await
            .unwrap();
        });
    });

    log::info!("All components started successfully!");
    log::info!("============================================");
    log::info!("Rollup is now accepting transactions");
    log::info!("HTTP API available at http://127.0.0.1:8080");
    log::info!("============================================");
    log::info!("");
    log::info!("Available endpoints:");
    log::info!("  GET  /              - Test endpoint");
    log::info!("  GET  /health        - Health check");
    log::info!("  GET  /stats         - Rollup statistics");
    log::info!("  GET  /metrics       - Detailed metrics");
    log::info!("  GET  /fees          - Fee estimates");
    log::info!("  GET  /events        - Recent events");
    log::info!("  GET  /cache/stats   - Cache statistics");
    log::info!("  POST /submit_transaction  - Submit transaction");
    log::info!("  POST /get_transaction     - Query transaction");
    log::info!("============================================");

    // Wait for server to finish
    server_thread.join().unwrap();
}
