// xlayer-node: reth (op-reth) + kona in a single process, connected via ChannelEngineClient.
//
// All reth flags (--datadir, --chain, --http.*, --authrpc.*) pass through to reth's own CLI.
// Only --xlayer-config is ours — points at a TOML file with kona-specific settings.
//
// On startup: reth launches first, then kona connects to it in-process.
// On shutdown: ctrl+c (or either side exiting) cancels the other.

#![allow(missing_docs)]

use clap::Parser;
use engine_bridge::ChannelEngineClient;
use k256::ecdsa::SigningKey;
use kona_disc::LocalNode;
use kona_engine::RollupBoostServerArgs;
use kona_genesis::RollupConfig;
use kona_gossip;
use kona_rpc::RpcBuilder;
use kona_node_service::{
    EngineConfig, L1ConfigBuilder, NetworkConfig, NodeMode, RollupNodeBuilder, SequencerConfig,
};
use alloy_primitives::Address;
use alloy_provider::RootProvider;
use alloy_rpc_types_engine::JwtSecret;
use libp2p::Multiaddr;
use op_alloy_network::Optimism;
use rand::rngs::OsRng;
use reth_optimism_cli::{Cli, chainspec::OpChainSpecParser};
use reth_optimism_node::{OpNode, args::RollupArgs};
use rollup_boost::ExecutionMode;
use std::{net::{IpAddr, Ipv4Addr, SocketAddr}, path::PathBuf, sync::Arc, time::Duration};
use tracing::info;

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

// Extra CLI args — added on top of reth's own flags.
// RollupArgs is flattened so all op-reth flags (--rollup.*, --sequencer.*) still work.
#[derive(Debug, clap::Args, Clone)]
struct XlayerArgs {
    /// Path to xlayer TOML config (kona-specific settings: L1 RPC, beacon, rollup config path)
    #[arg(long = "xlayer-config", required = true)]
    xlayer_config: PathBuf,

    #[command(flatten)]
    rollup: RollupArgs,
}

// Kona-specific config read from --xlayer-config TOML.
// Reth reads its own settings from its CLI flags.
#[derive(serde::Deserialize)]
struct XlayerConfig {
    l1_rpc_url: url::Url,
    /// Reth's own HTTP RPC port — used by kona for L2 block queries (not on hot path)
    l2_rpc_url: url::Url,
    beacon_url: url::Url,
    /// kona-node equivalent of --l2-config-file
    rollup_config: PathBuf,
    /// kona-node equivalent of --l1-config-file — full alloy genesis JSON
    l1_genesis_file: PathBuf,
    /// Unsafe block signer address for P2P gossip validation
    #[serde(default)]
    unsafe_block_signer: Address,
    /// kona-node equivalent of --sequencer.l1-confs (default 5 on devnet)
    #[serde(default = "default_l1_confs")]
    l1_confs: u64,
    /// Kona rollup RPC port (kona-node equivalent of --rpc.port, default 9545)
    #[serde(default = "default_rpc_port")]
    rpc_port: u16,
}

fn default_l1_confs() -> u64 { 5 }
fn default_rpc_port() -> u16 { 9545 }

//  ┌─────────────────────────── xlayer-node process ────────────────────────────────┐
//  │                                                                                │
//  │   ┌──────────────────────┐   ChannelEngineClient    ┌──────────────────────┐   │
//  │   │  kona (consensus)    │──────────────────────── ►│  reth (execution)    │   │
//  │   │                      │   reth_engine_handle     │                      │   │
//  │   │  watches L1          │   reth_payload_handle    │  executes TXs        │   │
//  │   │  derives L2 blocks   │   (in-process, no HTTP,  │  builds payloads     │   │
//  │   │  sequences blocks    │    no JWT auth)          │  seals blocks        │   │
//  │   │                      │◄ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ │                      │   │
//  │   │  :9545 rollup RPC    │   l2_provider            │  :8123 eth_* RPC     │   │
//  │   └──────────────────────┘   (L2 queries, not on    └──────────────────────┘   │
//  │             │                 FCU hot path)                                    │
//  └─────────────│──────────────────────────────────────────────────────────────────┘
//                │ l1_provider (block ingestion)
//                │ l1_config   (slot timing, finality)
//                ▼
//  L1 geth :8545  +  L1 beacon :3500
//
fn main() {
    reth_cli_util::sigsegv_handler::install();

    if std::env::var_os("RUST_BACKTRACE").is_none() {
        // SAFETY: single-threaded at this point, no other threads reading env
        unsafe { std::env::set_var("RUST_BACKTRACE", "1"); }
    }

    if let Err(err) =
        Cli::<OpChainSpecParser, XlayerArgs>::parse().run(|builder, xlayer_args| async move {
            info!(target: "xlayer", "launching xlayer-node");

            // Load xlayer config
            let cfg: XlayerConfig =
                toml::from_str(&std::fs::read_to_string(&xlayer_args.xlayer_config)?)?;

            // Load kona's rollup config from JSON file
            let rollup_cfg: RollupConfig =
                serde_json::from_str(&std::fs::read_to_string(&cfg.rollup_config)?)?;
            let rollup_cfg = Arc::new(rollup_cfg);

            // Launch reth — blocks until engine is ready, then returns handle
            let reth_handle = builder
                .node(OpNode::new(xlayer_args.rollup))
                .launch_with_debug_capabilities()
                .await?;

            info!(target: "xlayer", "reth engine ready, starting kona");

            // Extract reth's in-process handles — used by ChannelEngineClient instead of HTTP
            let reth_engine_handle = reth_handle.node.add_ons_handle.beacon_engine_handle.clone();
            let reth_payload_handle = reth_handle.node.payload_builder_handle.clone();

            // HTTP providers for L1 and L2 block queries (not on the FCU hot path)
            let l1_provider = RootProvider::new_http(cfg.l1_rpc_url.clone());
            let l2_provider = RootProvider::<Optimism>::new_http(cfg.l2_rpc_url.clone());

            // In-process engine client: replaces kona's HTTP OpEngineClient with direct channel calls
            let engine_client = Arc::new(ChannelEngineClient::new(
                reth_engine_handle,
                reth_payload_handle,
                rollup_cfg.clone(),
                l1_provider,
                l2_provider,
            ));

            // Read full L1 chain config from the genesis JSON — same file kona-node uses
            // via --l1-config-file. Constructing from defaults would miss hardfork blocks.
            let l1_genesis: alloy_genesis::Genesis =
                serde_json::from_str(&std::fs::read_to_string(&cfg.l1_genesis_file)?)?;
            let l1_config = L1ConfigBuilder {
                chain_config: l1_genesis.config,
                trust_rpc: false,
                beacon: cfg.beacon_url.clone(),
                rpc_url: cfg.l1_rpc_url.clone(),
                slot_duration_override: None,
            };

            // P2P — loopback only for devnet. Random key per boot is fine for testing.
            // gossip_config must use kona's custom default, NOT libp2p's Config::default().
            // NetworkConfig::new() sets gossip_config: Default::default() which uses libp2p's
            // default (flood_publish=true). That combination fails Gossipsub validation.
            // kona-node CLI always starts from kona_gossip::default_config_builder() — we do
            // the same here by overriding the field after construction.
            let signing_key = SigningKey::random(&mut OsRng);
            let mut p2p_config = NetworkConfig::new(
                (*rollup_cfg).clone(),
                LocalNode::new(signing_key, IpAddr::from([127, 0, 0, 1]), 30303, 30303),
                "/ip4/127.0.0.1/tcp/9223".parse::<Multiaddr>()?,
                cfg.unsafe_block_signer,
            );
            p2p_config.gossip_config = kona_gossip::default_config();

            // EngineConfig: l2_url, l2_jwt_secret, builder_url, builder_jwt_secret are all dead
            // code in the start_with_client path — the injected ChannelEngineClient is used
            // instead of kona's built-in HTTP OpEngineClient. Fill JWT fields with random
            // throwaway values; they are never read after construction.
            let unused_jwt = JwtSecret::random();
            let kona_engine_config = EngineConfig {
                config: rollup_cfg.clone(),
                builder_url: cfg.l2_rpc_url.clone(),
                builder_jwt_secret: unused_jwt,
                builder_timeout: Duration::from_secs(5),
                l2_url: cfg.l2_rpc_url.clone(),
                l2_jwt_secret: unused_jwt,
                l2_timeout: Duration::from_secs(5),
                l1_url: cfg.l1_rpc_url.clone(),
                mode: NodeMode::Sequencer,
                rollup_boost: RollupBoostServerArgs {
                    initial_execution_mode: ExecutionMode::Disabled,
                    block_selection_policy: None,
                    external_state_root: false,
                    ignore_unhealthy_builders: true,
                    flashblocks: None,
                },
            };

            // Required: kona's L1 query channel is only kept alive by the RPC context.
            // Passing None drops the sender immediately → L1 watcher crashes at startup.
            let kona_rpc_config = RpcBuilder {
                no_restart: false,
                socket: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), cfg.rpc_port),
                enable_admin: true,
                admin_persistence: None,
                ws_enabled: false,
                dev_enabled: false,
            };

            let rollup_node = RollupNodeBuilder::new(
                (*rollup_cfg).clone(),
                l1_config,
                false, // l2_trust_rpc
                kona_engine_config,
                p2p_config,
                Some(kona_rpc_config),
            )
            .with_sequencer_config(SequencerConfig {
                sequencer_stopped: false,
                l1_conf_delay: cfg.l1_confs,
                ..Default::default()
            })
            .build();

            // Kona runs until shutdown — launch it alongside reth
            let kona_task = tokio::spawn(async move {
                rollup_node.start_with_client(engine_client).await
            });

            // Exit when either side stops, or ctrl+c
            tokio::select! {
                _ = reth_handle.node_exit_future => {
                    info!(target: "xlayer", "reth exited");
                }
                res = kona_task => {
                    info!(target: "xlayer", "kona exited");
                    res?.map_err(|e| eyre::eyre!(e))?;
                }
                _ = tokio::signal::ctrl_c() => {
                    info!(target: "xlayer", "ctrl+c received");
                }
            }

            Ok(())
        })
    {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
