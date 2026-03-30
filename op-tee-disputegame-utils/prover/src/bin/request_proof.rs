//! Submit a remote Groth16 proof request to Boundless for a given attestation document.
//!
//! Reads the following environment variables (no .env file loading):
//!   RPC_URL      - Ethereum RPC endpoint
//!   PRIVATE_KEY  - Wallet private key (hex, with or without 0x prefix)
//!   PINATA_JWT   - Pinata API JWT (required by Boundless client init)
//!   PROGRAM_URL  - (optional) overrides value from --program-info file
//!   IMAGE_ID     - (optional) overrides value from --program-info file
//!
//! Usage:
//!   cargo run --release -p prover --bin request_proof -- \
//!       --attestation <base64-encoded-doc> \
//!       [--program-info /tmp/program_info.json] \
//!       [--output /tmp/proof_result.json]
//!
//! PROGRAM_URL and IMAGE_ID must be resolvable either via --program-info or env vars.

use alloy::primitives::utils::parse_units;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolCall;
use base64::Engine as _;
use boundless_market::{storage::storage_provider_from_env, Client};
use std::path::PathBuf;
use std::time::Duration;
use url::Url;

// ABI definition for the on-chain TeeVerifier register function.
alloy::sol! {
    interface ITeeVerifier {
        struct AttestationData {
            uint64 timestampMs;
            bytes32 pcrHash;
            bytes publicKey;
            bytes userData;
        }
        /// Register a TEE EOA on-chain by verifying a Groth16 ZK proof of the Nitro attestation.
        function register(bytes calldata seal, AttestationData calldata attestationData) external;
    }
}

fn parse_args() -> (String, Option<PathBuf>, Option<PathBuf>) {
    let args: Vec<String> = std::env::args().collect();
    let mut attestation_b64: Option<String> = None;
    let mut program_info_path: Option<PathBuf> = None;
    let mut output_path: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--attestation" | "-a" => {
                i += 1;
                attestation_b64 = Some(args[i].clone());
            }
            "--program-info" | "-p" => {
                i += 1;
                program_info_path = Some(PathBuf::from(&args[i]));
            }
            "--output" | "-o" => {
                i += 1;
                output_path = Some(PathBuf::from(&args[i]));
            }
            "--help" | "-h" => {
                println!("Usage: request_proof [OPTIONS]");
                println!();
                println!("Options:");
                println!("  --attestation, -a <B64>   Base64-encoded attestation document (required)");
                println!("  --program-info, -p <PATH> JSON file from upload_program (program_url + image_id)");
                println!("  --output, -o <PATH>       Write proof result JSON to this path");
                println!();
                println!("Env vars:");
                println!("  RPC_URL, PRIVATE_KEY, PINATA_JWT  (required)");
                println!("  PROGRAM_URL, IMAGE_ID              (override --program-info values)");
                std::process::exit(0);
            }
            other => {
                eprintln!("Unknown argument: {}", other);
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let attestation_b64 =
        attestation_b64.expect("--attestation / -a <base64-encoded-doc> is required");
    (attestation_b64, program_info_path, output_path)
}

/// Resolve PROGRAM_URL and IMAGE_ID.
/// JSON file is loaded first (if given), then env vars override.
fn resolve_program_info(program_info_path: Option<&PathBuf>) -> (String, String) {
    let mut program_url: Option<String> = None;
    let mut image_id: Option<String> = None;

    if let Some(path) = program_info_path {
        let raw = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));
        let json: serde_json::Value =
            serde_json::from_str(&raw).unwrap_or_else(|e| panic!("Invalid JSON in {}: {}", path.display(), e));
        program_url = json.get("program_url").and_then(|v| v.as_str()).map(str::to_string);
        image_id = json.get("image_id").and_then(|v| v.as_str()).map(str::to_string);
    }

    // Env vars override file values
    if let Ok(v) = std::env::var("PROGRAM_URL") {
        program_url = Some(v);
    }
    if let Ok(v) = std::env::var("IMAGE_ID") {
        image_id = Some(v);
    }

    let program_url = program_url
        .expect("PROGRAM_URL must be set via --program-info file or PROGRAM_URL env var");
    let image_id =
        image_id.expect("IMAGE_ID must be set via --program-info file or IMAGE_ID env var");

    (program_url, image_id)
}

#[tokio::main]
async fn main() {
    let (attestation_b64, program_info_path, output_path) = parse_args();

    // Decode attestation
    let att_bytes = base64::engine::general_purpose::STANDARD
        .decode(&attestation_b64)
        .expect("Failed to base64-decode --attestation value");
    eprintln!("Attestation: {} bytes", att_bytes.len());

    // Resolve program info
    let (program_url, image_id_str) = resolve_program_info(program_info_path.as_ref());
    let image_id_hex = image_id_str.trim_start_matches("0x").to_string();
    eprintln!("PROGRAM_URL: {}", program_url);
    eprintln!("IMAGE_ID:    0x{}", image_id_hex);

    // Build Boundless client
    let rpc_url = std::env::var("RPC_URL")
        .expect("RPC_URL env var is required");
    let private_key_str = std::env::var("PRIVATE_KEY")
        .expect("PRIVATE_KEY env var is required");

    let normalized_key = if private_key_str.starts_with("0x") {
        private_key_str.clone()
    } else {
        format!("0x{}", private_key_str)
    };
    let private_key: PrivateKeySigner = normalized_key.parse().expect("Invalid PRIVATE_KEY");

    let storage_provider =
        storage_provider_from_env().expect("Storage provider init failed (check PINATA_JWT env var)");

    let max_price = std::env::var("MAX_PRICE_PER_CYCLE").unwrap_or_else(|_| "50000".to_string());
    let min_price = std::env::var("MIN_PRICE_PER_CYCLE").unwrap_or_else(|_| "10000".to_string());

    let client = Client::builder()
        .with_rpc_url(Url::parse(&rpc_url).expect("Invalid RPC_URL"))
        .with_private_key(private_key)
        .with_storage_provider(Some(storage_provider))
        .config_offer_layer(|config| {
            config
                .max_price_per_cycle(parse_units(&max_price, "wei").unwrap())
                .min_price_per_cycle(parse_units(&min_price, "wei").unwrap())
        })
        .build()
        .await
        .expect("Failed to build Boundless client");

    // Build proof request
    let url = Url::parse(&program_url).expect("Invalid PROGRAM_URL");
    let request = client
        .new_request()
        .with_program_url(url)
        .expect("Failed to set program URL")
        .with_stdin(att_bytes)
        .with_groth16_proof();

    // Submit on-chain
    eprintln!("\nSubmitting proof request to Boundless Market...");
    let (request_id, expires_at) = client
        .submit_onchain(request)
        .await
        .unwrap_or_else(|e| {
            eprintln!("submit_onchain failed: {:?}", e);
            std::process::exit(1);
        });
    eprintln!("Request ID:  {:x}", request_id);
    eprintln!("Expires at:  {:?}", expires_at);
    eprintln!("Explorer:    https://explorer.boundless.network/orders/0x{:x}", request_id);

    // Wait for fulfillment
    let poll_secs: u64 = std::env::var("POLL_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    eprintln!("\nWaiting for prover (polling every {}s)...", poll_secs);

    let fulfillment = client
        .wait_for_request_fulfillment(request_id, Duration::from_secs(poll_secs), expires_at)
        .await
        .unwrap_or_else(|e| {
            eprintln!("wait_for_request_fulfillment failed: {:?}", e);
            eprintln!("Request ID: {:x}", request_id);
            std::process::exit(1);
        });

    // Extract seal and journal
    let seal = fulfillment.seal.to_vec();
    let fulfillment_data = fulfillment.data().expect("Failed to parse fulfillment data");
    let journal = fulfillment_data
        .journal()
        .expect("Journal missing from fulfillment")
        .to_vec();

    let seal_hex = hex::encode(&seal);
    let journal_hex = hex::encode(&journal);

    // Parse journal fields (layout from guest/src/main.rs):
    //   [0..8]   timestamp (u64 BE, milliseconds)
    //   [8..40]  SHA256(PCR0) (32 bytes)
    //   [40..136] root CA pubkey (96 bytes, P-384 point without 0x04 prefix)
    //   [136]    enclave pubkey length (1 byte)
    //   [137..137+n] enclave pubkey bytes
    //   [137+n..137+n+2] user data length (u16 BE)
    //   [139+n..] user data bytes
    let mut joff = 0usize;
    let timestamp_ms = u64::from_be_bytes(journal[joff..joff + 8].try_into().unwrap());
    joff += 8;
    let pcr0_hash: [u8; 32] = journal[joff..joff + 32].try_into().unwrap();
    let pcr0_hash_hex = hex::encode(&pcr0_hash);
    joff += 32;
    let root_pubkey_hex = hex::encode(&journal[joff..joff + 96]);
    joff += 96;
    let enclave_pubkey_len = journal[joff] as usize;
    joff += 1;
    let enclave_pubkey = journal[joff..joff + enclave_pubkey_len].to_vec();
    let enclave_pubkey_hex = hex::encode(&enclave_pubkey);
    joff += enclave_pubkey_len;
    let user_data_len = u16::from_be_bytes(journal[joff..joff + 2].try_into().unwrap()) as usize;
    joff += 2;
    let user_data = journal[joff..joff + user_data_len].to_vec();
    let user_data_hex = hex::encode(&user_data);

    // ABI-encode calldata for: register(bytes calldata seal, AttestationData calldata attestationData)
    let register_calldata = ITeeVerifier::registerCall {
        seal: seal.clone().into(),
        attestationData: ITeeVerifier::AttestationData {
            timestampMs: timestamp_ms,
            pcrHash: pcr0_hash.into(),
            publicKey: enclave_pubkey.clone().into(),
            userData: user_data.clone().into(),
        },
    }
    .abi_encode();
    let register_calldata_hex = hex::encode(&register_calldata);

    // Print to stdout
    println!("seal=0x{}", seal_hex);
    println!("journal=0x{}", journal_hex);
    println!("register_calldata=0x{}", register_calldata_hex);

    // Write JSON result if output path given
    if let Some(path) = output_path {
        let json = serde_json::json!({
            "seal_hex": seal_hex,
            "journal_hex": journal_hex,
            "image_id_hex": image_id_hex,
            "request_id": format!("{:x}", request_id),
            "register_calldata": format!("{}", register_calldata_hex),
            "journal_parsed": {
                "timestamp_ms": timestamp_ms,
                "pcr0_hash": pcr0_hash_hex,
                "root_pubkey": root_pubkey_hex,
                "enclave_pubkey": enclave_pubkey_hex,
                "user_data": user_data_hex,
            },
        });
        std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap())
            .unwrap_or_else(|e| panic!("Failed to write {}: {}", path.display(), e));
        eprintln!("Proof result written to {}", path.display());
    }
}
