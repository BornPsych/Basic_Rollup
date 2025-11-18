use std::thread;

use actix_web::{web, App, HttpServer};
use async_channel;
use crossbeam;
use frontend::FrontendMessage;
use rollupdb::RollupDBMessage;
use solana_sdk::transaction::Transaction;
use tokio::runtime::Builder;

mod data_availability;
mod frontend;
mod merkle;
mod rollupdb;
mod sequencer;
mod settle;
mod state;

fn main() {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    log::info!("============================================");
    log::info!("   Solana SVM Rollup - Full Implementation");
    log::info!("============================================");
    log::info!("Starting rollup components...");

    // Create communication channels
    let (sequencer_sender, sequencer_receiver) = crossbeam::channel::unbounded::<Transaction>();
    let (rollupdb_sender, rollupdb_receiver) = crossbeam::channel::unbounded::<RollupDBMessage>();
    let (frontend_sender, frontend_receiver) = async_channel::unbounded::<FrontendMessage>();

    let db_sender = rollupdb_sender.clone();
    let fe_sender = frontend_sender.clone();

    // Spawn sequencer and database thread
    log::info!("Starting sequencer and database threads...");
    let _processing_thread = thread::spawn(move || {
        let rt = Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap();

        // Run sequencer (blocking)
        let sequencer_handle = thread::spawn(|| {
            if let Err(e) = sequencer::run(sequencer_receiver, db_sender) {
                log::error!("Sequencer error: {:?}", e);
            }
        });

        // Run rollup DB (async)
        rt.block_on(async {
            rollupdb::RollupDB::run(rollupdb_receiver, fe_sender).await;
        });

        sequencer_handle.join().unwrap();
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
                    .app_data(web::Data::new(frontend_sender.clone()))
                    .app_data(web::Data::new(frontend_receiver.clone()))
                    .route("/", web::get().to(frontend::test))
                    .route("/health", web::get().to(frontend::health_check))
                    .route("/stats", web::get().to(frontend::get_stats))
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

    // Wait for server to finish
    server_thread.join().unwrap();
}
