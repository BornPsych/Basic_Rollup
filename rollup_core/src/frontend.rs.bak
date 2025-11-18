use actix_web::{error, web, HttpResponse};
use async_channel::{Receiver, Sender};
use crossbeam::channel::Sender as CBSender;
use serde::{Deserialize, Serialize};
use solana_sdk::{keccak::Hash, transaction::Transaction};
use std::collections::HashMap;

use crate::{
    rollupdb::RollupDBMessage,
    state::{StateTransition, TransactionBatch},
};

/// Message format to send data from DB to frontend
#[derive(Serialize, Deserialize, Clone)]
pub struct FrontendMessage {
    pub get_tx: Option<Hash>,
    pub transaction: Option<StateTransition>,
    pub account: Option<solana_sdk::account::AccountSharedData>,
    pub state_root: Option<Hash>,
    pub batch_info: Option<TransactionBatch>,
}

/// Request format for getting a transaction
#[derive(Serialize, Deserialize, Debug)]
pub struct GetTransactionRequest {
    pub tx_hash: String,
}

/// Request format for submitting transactions
#[derive(Serialize, Deserialize, Debug)]
pub struct SubmitTransactionRequest {
    pub sender: String,
    pub sol_transaction: Transaction,
}

/// Response for transaction submission
#[derive(Serialize, Deserialize)]
pub struct SubmitTransactionResponse {
    pub status: String,
    pub message: String,
    pub tx_hash: Option<String>,
}

/// Response for transaction query
#[derive(Serialize, Deserialize)]
pub struct GetTransactionResponse {
    pub found: bool,
    pub transaction: Option<StateTransition>,
}

/// Response for statistics
#[derive(Serialize, Deserialize)]
pub struct StatsResponse {
    pub rollup_name: String,
    pub version: String,
    pub status: String,
}

/// Test endpoint
pub async fn test() -> HttpResponse {
    log::info!("Test endpoint called");
    HttpResponse::Ok().json(HashMap::from([
        ("status", "ok"),
        ("message", "Rollup is running"),
    ]))
}

/// Submit a transaction to the rollup
pub async fn submit_transaction(
    body: web::Json<SubmitTransactionRequest>,
    sequencer_sender: web::Data<CBSender<Transaction>>,
) -> actix_web::Result<HttpResponse> {
    log::info!("Transaction submission request from: {}", body.sender);
    log::debug!("Transaction details: {:?}", body.sol_transaction);

    // Validate transaction
    if let Err(e) = body.sol_transaction.verify() {
        log::warn!("Invalid transaction signature: {}", e);
        return Ok(HttpResponse::BadRequest().json(SubmitTransactionResponse {
            status: "error".to_string(),
            message: format!("Invalid transaction signature: {}", e),
            tx_hash: None,
        }));
    }

    // Compute transaction hash
    let tx_hash = {
        use solana_sdk::keccak::Hasher;
        let mut hasher = Hasher::default();
        if let Ok(serialized) = bincode::serialize(&body.sol_transaction) {
            hasher.hash(&serialized);
        }
        hasher.result()
    };

    // Send to sequencer
    match sequencer_sender.send(body.sol_transaction.clone()) {
        Ok(_) => {
            log::info!("Transaction {} sent to sequencer", tx_hash);
            Ok(HttpResponse::Ok().json(SubmitTransactionResponse {
                status: "submitted".to_string(),
                message: "Transaction submitted successfully".to_string(),
                tx_hash: Some(tx_hash.to_string()),
            }))
        }
        Err(e) => {
            log::error!("Failed to send transaction to sequencer: {}", e);
            Ok(HttpResponse::InternalServerError().json(SubmitTransactionResponse {
                status: "error".to_string(),
                message: format!("Failed to submit transaction: {}", e),
                tx_hash: None,
            }))
        }
    }
}

/// Get a transaction by hash
pub async fn get_transaction(
    body: web::Json<GetTransactionRequest>,
    rollupdb_sender: web::Data<CBSender<RollupDBMessage>>,
    frontend_receiver: web::Data<Receiver<FrontendMessage>>,
) -> actix_web::Result<HttpResponse> {
    log::info!("Transaction query request for hash: {}", body.tx_hash);

    // Parse hash
    let tx_hash = Hash::new(body.tx_hash.as_bytes());

    // Request transaction from database
    if let Err(e) = rollupdb_sender.send(RollupDBMessage {
        lock_accounts: None,
        unlock_accounts: None,
        add_processed_transaction: None,
        frontend_get_tx: Some(tx_hash),
        add_settle_proof: None,
        get_account: None,
        get_batch_for_settlement: false,
    }) {
        log::error!("Failed to query database: {}", e);
        return Ok(HttpResponse::InternalServerError().json(GetTransactionResponse {
            found: false,
            transaction: None,
        }));
    }

    // Wait for response with timeout
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        frontend_receiver.recv(),
    )
    .await
    {
        Ok(Ok(message)) => {
            if let Some(transaction) = message.transaction {
                log::info!("Transaction found: {:?}", tx_hash);
                Ok(HttpResponse::Ok().json(GetTransactionResponse {
                    found: true,
                    transaction: Some(transaction),
                }))
            } else {
                log::info!("Transaction not found: {:?}", tx_hash);
                Ok(HttpResponse::NotFound().json(GetTransactionResponse {
                    found: false,
                    transaction: None,
                }))
            }
        }
        Ok(Err(e)) => {
            log::error!("Error receiving from database: {}", e);
            Ok(HttpResponse::InternalServerError().json(GetTransactionResponse {
                found: false,
                transaction: None,
            }))
        }
        Err(_) => {
            log::warn!("Timeout waiting for transaction response");
            Ok(HttpResponse::RequestTimeout().json(GetTransactionResponse {
                found: false,
                transaction: None,
            }))
        }
    }
}

/// Get rollup statistics
pub async fn get_stats() -> HttpResponse {
    log::info!("Stats endpoint called");

    HttpResponse::Ok().json(StatsResponse {
        rollup_name: "Solana SVM Rollup".to_string(),
        version: "0.1.0".to_string(),
        status: "running".to_string(),
    })
}

/// Health check endpoint
pub async fn health_check() -> HttpResponse {
    HttpResponse::Ok().json(HashMap::from([("status", "healthy")]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_submit_transaction_request_serialization() {
        let tx = Transaction::default();
        let req = SubmitTransactionRequest {
            sender: "test".to_string(),
            sol_transaction: tx,
        };

        let serialized = serde_json::to_string(&req).unwrap();
        assert!(!serialized.is_empty());
    }
}
