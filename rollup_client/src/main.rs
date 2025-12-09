use anyhow::Result;
use bincode;
use serde::{Deserialize, Serialize};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    instruction::Instruction,
    keccak::{Hash, Hasher},
    native_token::LAMPORTS_PER_SOL,
    signature::Signature,
    signer::{self, keypair::read_keypair_file, Signer},
    system_instruction, system_program,
    transaction::Transaction,
};
use solana_transaction_status::UiTransactionEncoding;
use std::{collections::HashMap, str::FromStr, thread, time::Duration};

#[derive(Serialize, Deserialize, Debug)]
struct SubmitTransactionRequest {
    sender: String,
    sol_transaction: Transaction,
}

#[derive(Serialize, Deserialize, Debug)]
struct SubmitTransactionResponse {
    status: String,
    message: String,
    tx_hash: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GetTransactionRequest {
    pub tx_hash: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("============================================");
    println!("   Rollup Client - Testing Tool");
    println!("============================================\n");

    let client = reqwest::Client::new();

    // Test 1: Health check
    println!("Test 1: Checking rollup health...");
    match test_health_check(&client).await {
        Ok(_) => println!("✓ Health check passed\n"),
        Err(e) => {
            println!("✗ Health check failed: {}\n", e);
            return Err(e);
        }
    }

    // Test 2: Get stats
    println!("Test 2: Getting rollup stats...");
    match test_stats(&client).await {
        Ok(_) => println!("✓ Stats retrieved successfully\n"),
        Err(e) => println!("✗ Stats retrieval failed: {}\n", e),
    }

    // Test 3: Submit a transaction
    println!("Test 3: Submitting a test transaction...");
    match test_submit_transaction(&client).await {
        Ok(tx_hash) => {
            println!("✓ Transaction submitted successfully");
            println!("  Transaction hash: {}\n", tx_hash);

            // Wait a bit for processing
            println!("Waiting for transaction to be processed...");
            thread::sleep(Duration::from_secs(2));

            // Test 4: Query the transaction
            println!("Test 4: Querying submitted transaction...");
            match test_get_transaction(&client, &tx_hash).await {
                Ok(_) => println!("✓ Transaction query successful\n"),
                Err(e) => println!("✗ Transaction query failed: {}\n", e),
            }
        }
        Err(e) => {
            println!("✗ Transaction submission failed: {}\n", e);
        }
    }

    // Test 5: Submit multiple transactions (batch test)
    println!("Test 5: Submitting multiple transactions for batch testing...");
    match test_batch_submission(&client, 5).await {
        Ok(count) => println!("✓ Successfully submitted {} transactions\n", count),
        Err(e) => println!("✗ Batch submission failed: {}\n", e),
    }

    println!("============================================");
    println!("   All tests completed!");
    println!("============================================");

    Ok(())
}

async fn test_health_check(client: &reqwest::Client) -> Result<()> {
    let response = client
        .get("http://127.0.0.1:8080/health")
        .send()
        .await?
        .json::<HashMap<String, String>>()
        .await?;

    println!("  Health status: {:?}", response);
    Ok(())
}

async fn test_stats(client: &reqwest::Client) -> Result<()> {
    let response = client
        .get("http://127.0.0.1:8080/stats")
        .send()
        .await?
        .json::<HashMap<String, String>>()
        .await?;

    println!("  Stats: {:?}", response);
    Ok(())
}

async fn test_submit_transaction(client: &reqwest::Client) -> Result<String> {
    // Create a simple transfer transaction
    let tx = create_test_transaction()?;

    let request = SubmitTransactionRequest {
        sender: "Test Client".to_string(),
        sol_transaction: tx.clone(),
    };

    let response = client
        .post("http://127.0.0.1:8080/submit_transaction")
        .json(&request)
        .send()
        .await?
        .json::<SubmitTransactionResponse>()
        .await?;

    println!("  Response: {}", response.message);

    response
        .tx_hash
        .ok_or_else(|| anyhow::anyhow!("No transaction hash returned"))
}

async fn test_get_transaction(client: &reqwest::Client, tx_hash: &str) -> Result<()> {
    let request = GetTransactionRequest {
        tx_hash: tx_hash.to_string(),
    };

    let response = client
        .post("http://127.0.0.1:8080/get_transaction")
        .json(&request)
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await?;

    println!("  Response status: {}", status);
    println!("  Response body: {}", body);

    Ok(())
}

async fn test_batch_submission(client: &reqwest::Client, count: usize) -> Result<usize> {
    let mut successful = 0;

    for i in 0..count {
        match test_submit_transaction(client).await {
            Ok(hash) => {
                println!("  Transaction {}/{} submitted: {}", i + 1, count, hash);
                successful += 1;
                // Small delay between transactions
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => {
                println!("  Transaction {}/{} failed: {}", i + 1, count, e);
            }
        }
    }

    Ok(successful)
}

fn create_test_transaction() -> Result<Transaction> {
    // Create dummy keypairs for testing
    // In production, you would load real keypairs
    let payer = signer::keypair::Keypair::new();
    let recipient = signer::keypair::Keypair::new();

    // Use a dummy RPC client just to get a recent blockhash
    // The rollup doesn't verify blockhashes in the current implementation
    let rpc_url = std::env::var("SOLANA_RPC_URL")
        .unwrap_or_else(|_| "https://api.devnet.solana.com".to_string());

    let rpc_client = RpcClient::new(rpc_url);
    let recent_blockhash = rpc_client.get_latest_blockhash()?;

    // Create a simple transfer instruction
    let transfer_ix = system_instruction::transfer(
        &payer.pubkey(),
        &recipient.pubkey(),
        LAMPORTS_PER_SOL / 100, // 0.01 SOL
    );

    // Create and sign transaction
    let tx = Transaction::new_signed_with_payer(
        &[transfer_ix],
        Some(&payer.pubkey()),
        &[&payer],
        recent_blockhash,
    );

    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_transaction() {
        let tx = create_test_transaction();
        assert!(tx.is_ok());

        let tx = tx.unwrap();
        assert_eq!(tx.message.instructions.len(), 1);
    }
}
