//! Upload the guest ELF to IPFS via Pinata and print the resulting PROGRAM_URL + IMAGE_ID.
//!
//! Reads the following environment variables (no .env file loading):
//!   RPC_URL     - Ethereum RPC endpoint
//!   PRIVATE_KEY - Wallet private key (hex, with or without 0x prefix)
//!   PINATA_JWT  - Pinata API JWT for IPFS upload
//!
//! Usage:
//!   cargo run --release -p prover --features embed-methods --bin upload_program
//!   cargo run --release -p prover --features embed-methods --bin upload_program -- /tmp/program_info.json

use alloy::signers::local::PrivateKeySigner;
use boundless_market::{storage::storage_provider_from_env, Client};
#[cfg(feature = "embed-methods")]
use methods::{GUEST_ELF, GUEST_ID};
use url::Url;

#[tokio::main]
async fn main() {
    // Optional output path from first positional argument
    let args: Vec<String> = std::env::args().collect();
    let output_path: Option<String> = args.get(1).cloned();

    // Read required env vars directly — no .env file loading
    let rpc_url = std::env::var("RPC_URL")
        .expect("RPC_URL env var is required (e.g. https://base-rpc.publicnode.com)");
    let private_key_str =
        std::env::var("PRIVATE_KEY").expect("PRIVATE_KEY env var is required");

    let normalized_key = if private_key_str.starts_with("0x") {
        private_key_str.clone()
    } else {
        format!("0x{}", private_key_str)
    };
    let private_key: PrivateKeySigner = normalized_key.parse().expect("Invalid PRIVATE_KEY");

    let storage_provider =
        storage_provider_from_env().expect("Storage provider init failed (check PINATA_JWT env var)");

    let client = Client::builder()
        .with_rpc_url(Url::parse(&rpc_url).expect("Invalid RPC_URL"))
        .with_private_key(private_key)
        .with_storage_provider(Some(storage_provider))
        .build()
        .await
        .expect("Failed to build Boundless client");

    // Derive IMAGE_ID
    #[cfg(feature = "embed-methods")]
    let image_id_hex: String = {
        let bytes: Vec<u8> = GUEST_ID.iter().flat_map(|w| w.to_le_bytes()).collect();
        hex::encode(&bytes)
    };
    #[cfg(not(feature = "embed-methods"))]
    let image_id_hex: String = {
        std::env::var("IMAGE_ID")
            .expect("IMAGE_ID env var required when embed-methods feature is not enabled")
            .trim_start_matches("0x")
            .to_string()
    };

    // Upload guest ELF
    #[cfg(feature = "embed-methods")]
    let program_url = {
        eprintln!(
            "Uploading guest ELF ({} bytes / {:.1} MB) to IPFS via Pinata...",
            GUEST_ELF.len(),
            GUEST_ELF.len() as f64 / 1_048_576.0
        );
        client
            .upload_program(GUEST_ELF)
            .await
            .expect("Failed to upload guest ELF to storage")
            .to_string()
    };

    #[cfg(not(feature = "embed-methods"))]
    let program_url = std::env::var("PROGRAM_URL")
        .expect("PROGRAM_URL env var required when embed-methods feature is not enabled");

    // Log to console
    println!("PROGRAM_URL={}", program_url);
    println!("IMAGE_ID=0x{}", image_id_hex);

    // Write JSON to file if path given
    if let Some(path) = output_path {
        let json = serde_json::json!({
            "program_url": program_url,
            "image_id": format!("0x{}", image_id_hex),
        });
        std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap())
            .unwrap_or_else(|e| panic!("Failed to write {}: {}", path, e));
        eprintln!("Written to {}", path);
    }
}
