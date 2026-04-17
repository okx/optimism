# main.rs — Line by Line Deep Dive
## What it does, why it was written that way, and what production looks like

---

## The Big Picture First

```
main.rs does exactly five things:
  1. Boot reth (execution layer)
  2. Wire reth's internal handles into ChannelEngineClient
  3. Configure kona with enough plumbing to start
  4. Boot kona (consensus layer) using that client
  5. Stay alive until either side exits
```

It is intentionally thin — all real engine logic lives in `engine-bridge/src/client.rs`.
main.rs is just the boot sequence and glue.

---

## Section 1: Lines 1–8 — Header Comments

```rust
// xlayer-node: reth (op-reth) + kona in a single process, connected via ChannelEngineClient.
// All reth flags pass through to reth's own CLI.
// Only --xlayer-config is ours.
// On startup: reth launches first, then kona connects to it in-process.
// On shutdown: ctrl+c (or either side exiting) cancels the other.
```

**What**: Accurate description of the design.
**Nothing to fix here.**

---

## Section 2: Lines 9–32 — Imports

```rust
#![allow(missing_docs)]
```

**What**: Suppresses the "missing doc comment" lint for the whole file.
**Why it's here**: We haven't written rustdoc yet. Fine for a dev binary.
**Production**: Remove once docs are added, or keep if this is an internal binary.

---

```rust
use k256::ecdsa::SigningKey;
use kona_disc::LocalNode;
use libp2p::Multiaddr;
use rand::rngs::OsRng;
```

**What**: P2P identity. Used once to create the P2P node identity (line 139).
**Why these specific crates**: kona's P2P stack uses libp2p. The node needs an ECDSA signing key to establish its peer identity on the gossip network.
**Production concern**: Currently a random key generated at every boot. See Section 8.

---

```rust
use kona_engine::RollupBoostServerArgs;
use rollup_boost::ExecutionMode;
```

**What**: Rollup boost is a feature for external block builders (like MEV boost on L1). These two imports exist only to fill in `EngineConfig.rollup_boost`.
**Why it's here**: `EngineConfig` requires a `RollupBoostServerArgs` field. We don't use rollup boost, but we still have to provide the struct.
**This is a workaround.** See Section 9.

---

```rust
#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();
```

**What**: Replaces Rust's default allocator with jemalloc (on Linux) or mimalloc (on macOS/Windows).
**Why**: reth uses this in all its binaries. jemalloc significantly reduces memory fragmentation and improves throughput under high allocation pressure (txpool, EVM execution).
**Production**: Keep exactly as-is. This is correct and important.

---

## Section 3: Lines 37–73 — CLI and Config Structs

### XlayerArgs

```rust
#[derive(Debug, clap::Args, Clone)]
struct XlayerArgs {
    #[arg(long = "xlayer-config", required = true)]
    xlayer_config: PathBuf,

    #[command(flatten)]
    rollup: RollupArgs,
}
```

**What**: Our additions on top of reth's CLI.
- `xlayer_config`: the one new flag we add. Points to a TOML file.
- `rollup: RollupArgs`: flattened from op-reth. This is what makes `--rollup.sequencer-http`, `--rollup.disable-tx-pool-gossip`, etc. all still work.

**Why `--xlayer-config` as a file instead of individual flags?**
Kona has ~10 settings. Adding each as a CLI flag would pollute reth's CLI namespace and would be hard to manage. A TOML sidecar keeps them cleanly separated.

**Production**: This design is fine. You might later replace the TOML with environment variables for container deployments (12-factor app style). Both approaches are valid.

---

### XlayerConfig

```rust
struct XlayerConfig {
    l1_rpc_url: url::Url,
    l2_rpc_url: url::Url,      // ← POTENTIAL CONFUSION: this is reth's own HTTP port
    beacon_url: url::Url,
    rollup_config: PathBuf,
    l1_genesis_file: PathBuf,
    unsafe_block_signer: Address,  // ← default = zero address
    l1_confs: u64,                 // ← default = 5
    rpc_port: u16,                 // ← default = 9545
}
```

**`l2_rpc_url` naming is confusing.**
It sounds like "the L2 node's RPC URL" — which it is — but specifically it's reth's own HTTP port (`:8123`) used by kona to do L2 block reads. It is NOT the Engine API. The name should probably be `l2_rpc_url_for_queries` or `reth_http_url`. This is a devnet shortcut.

**`unsafe_block_signer` defaults to zero address (0x000...000).**
This means P2P gossip block signature validation is effectively disabled. Any peer can send blocks and they'll be accepted. This is fine for a single-node devnet. In production this must be the sequencer's actual signing address — otherwise anyone could gossip fake unsafe blocks.

**`rollup_config` and `l1_genesis_file` are file paths.**
In production you'd likely embed or fetch these from a well-known URL or config service rather than depending on local file paths. But for now this is clean and explicit.

**`l1_confs = 5` default.**
Sequencer waits for 5 L1 confirmations before including L1-derived attributes. Prevents reorg-induced issues. Production chains typically use 4–6. Fine.

---

## Section 4: Lines 75–81 — main() Setup

```rust
fn main() {
    reth_cli_util::sigsegv_handler::install();
```

**What**: Installs a signal handler for `SIGSEGV`. If reth crashes with a segfault (rare, but possible if a C library like libmdbx has a bug), this prints a backtrace instead of just dying silently.
**Production**: Keep. This is a reth best practice.

---

```rust
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        unsafe { std::env::set_var("RUST_BACKTRACE", "1"); }
    }
```

**What**: Ensures backtraces print on panic, unless the user has already set `RUST_BACKTRACE`.
**Why `unsafe`**: `set_var` is unsafe in Rust 1.80+ because it's not thread-safe. The comment explains it's safe here because we're single-threaded at this point (no tokio runtime yet).
**Production**: This is fine but slightly awkward. Alternative: pass `RUST_BACKTRACE=1` in your systemd unit or container env. The `unsafe` block is technically correct but will generate clippy warnings if you enable the `unsafe_op_in_unsafe_fn` lint.

---

## Section 5: Lines 83–100 — Launch Reth

```rust
    if let Err(err) =
        Cli::<OpChainSpecParser, XlayerArgs>::parse().run(|builder, xlayer_args| async move {
```

**What**: This is reth's CLI entry point. `Cli::parse()` parses all CLI flags (both reth's and ours). `.run(closure)` sets up the tokio runtime and calls the closure with:
- `builder`: reth's `NodeBuilder` — knows how to launch an op-reth node
- `xlayer_args`: our `XlayerArgs` struct

**Why this structure**: We inherit reth's entire CLI for free. Every reth flag (`--datadir`, `--chain`, `--http.*`, `--authrpc.*`, `--metrics`) works without us writing any parsing code. We only add `--xlayer-config` on top.

**Production**: This is the correct pattern. Nothing to change.

---

```rust
            let node_handle = builder
                .node(OpNode::new(xlayer_args.rollup))
                .launch_with_debug_capabilities()
                .await?;
```

**What**: This single `.await` starts ALL of reth:
- Opens/creates MDBX database
- Starts the engine tree (canonical chain state machine)
- Starts the payload builder service (block building)
- Starts the HTTP RPC server
- Starts the metrics endpoint
- Starts the P2P network (if discovery enabled)

Returns AFTER the engine is ready to accept FCU/new_payload calls.

**`launch_with_debug_capabilities()`** vs `launch()`:
The debug variant exposes extra handles for introspection. We need it to access `add_ons_handle.beacon_engine_handle` (next section). Standard `launch()` doesn't expose that.

**Production concern**: `launch_with_debug_capabilities` is named "debug" which sounds non-production. The name is misleading — it's used in reth's own integration tests and tooling. The only difference is that additional handles are exposed. Functionally identical to `launch()`. Fine to use in production.

---

## Section 6: Lines 104–119 — Wire the Handles

```rust
            let engine_handle = node_handle.node.add_ons_handle.beacon_engine_handle.clone();
            let payload_handle = node_handle.node.payload_builder_handle.clone();
```

**What**: Pull out the two in-process channels to reth's internals.
- `engine_handle`: sends FCU and new_payload to the engine tree
- `payload_handle`: sends get_payload requests to the payload builder

**Why `.clone()`**: These handles are cheap `Arc`-wrapped mpsc senders. Cloning is just incrementing a reference count.

**`add_ons_handle.beacon_engine_handle`** — the field path is long and fragile.
This accesses reth internals that aren't part of any stable public API. If reth reorganises its `add_ons_handle` struct (renames a field, moves it), this line breaks at compile time.

**Production concern**: This is the most brittle line in the file. A production deployment needs a monitoring plan for reth upstream upgrades. Every reth upgrade must be manually verified against this field path. Consider pinning to a specific reth commit.

---

```rust
            let l1_provider = RootProvider::new_http(cfg.l1_rpc_url.clone());
            let l2_provider = RootProvider::<Optimism>::new_http(cfg.l2_rpc_url.clone());
```

**What**: Simple HTTP providers for non-hot-path reads (L1 block fetches, L2 block queries for kona's state tracking).
**No connection pooling, no retry logic, no circuit breaking.**
**Production**: Add retry logic (alloy has `RetryProvider` wrappers), connection limits, and timeouts. A flaky L1 RPC under load can cause these to fail silently.

---

## Section 7: Lines 121–131 — L1 Config

```rust
            let l1_genesis: alloy_genesis::Genesis =
                serde_json::from_str(&std::fs::read_to_string(&cfg.l1_genesis_file)?)?;
            let l1_config = L1ConfigBuilder {
                chain_config: l1_genesis.config,
                trust_rpc: false,
                beacon: cfg.beacon_url.clone(),
                rpc_url: cfg.l1_rpc_url.clone(),
                slot_duration_override: None,
            };
```

**What**: kona needs to know L1's chain config (hardfork times, chain ID, etc.) to correctly parse L1 blocks and derive L2 attributes. We read it from the same L1 genesis JSON file that l1-geth used to initialise.

**`trust_rpc: false`**: kona will independently verify L1 data rather than trusting whatever the RPC returns. Correct for a sequencer. A read-only follower could set `true` for performance.

**`slot_duration_override: None`**: Uses Ethereum's standard 12-second slot time. If your L1 devnet uses a different slot time (some use 6s), you'd set this. Devnet uses 12s, so None is correct.

**Production**: The genesis file approach works. In production you'd typically reference L1 genesis by URL/IPFS hash or from a well-known config service, not a local file. But for an in-process node this is fine.

---

## Section 8: Lines 133–146 — P2P Config (The Devnet Hack)

```rust
            let signing_key = SigningKey::random(&mut OsRng);
```

**HACK #1 — Random signing key on every boot.**

**What**: Generates a fresh ECDSA key for this node's P2P identity. Every restart = new identity = different peer ID on the gossip network.

**Why this is a hack**:
In production, a sequencer's P2P identity is persistent and well-known. Other nodes (verifiers, RPC nodes) have the sequencer's peer ID hardcoded as a trusted peer (`--p2p.static`). If the sequencer's peer ID changes on every restart, all peers lose the connection.

**Production**: Load the signing key from a file that persists across restarts. If the file doesn't exist, generate and save it.

```rust
// Production pattern:
let signing_key = match std::fs::read(datadir.join("p2p-key")) {
    Ok(bytes) => SigningKey::from_bytes(bytes.as_slice().into())?,
    Err(_) => {
        let key = SigningKey::random(&mut OsRng);
        std::fs::write(datadir.join("p2p-key"), key.to_bytes())?;
        key
    }
};
```

---

```rust
            let mut p2p_config = NetworkConfig::new(
                (*rollup_cfg).clone(),
                LocalNode::new(signing_key, IpAddr::from([127, 0, 0, 1]), 30303, 30303),
                "/ip4/127.0.0.1/tcp/9223".parse::<Multiaddr>()?,
                cfg.unsafe_block_signer,
            );
```

**HACK #2 — Hardcoded loopback + hardcoded port.**

`IpAddr::from([127, 0, 0, 1])` and `"/ip4/127.0.0.1/tcp/9223"` are literals.

**What this means**:
- The node only listens for P2P connections on loopback
- The gossip port is hardcoded to 9223
- Other nodes on different machines cannot reach this node's P2P

**Why it's acceptable for devnet**: Single machine. No remote peers needed.

**Production**: These should come from config:
```rust
LocalNode::new(signing_key, cfg.p2p_listen_addr, cfg.p2p_tcp_port, cfg.p2p_udp_port)
cfg.p2p_advertised_addr.parse::<Multiaddr>()?
```

---

```rust
            p2p_config.gossip_config = kona_gossip::default_config();
```

**HACK #3 — Post-construction override of a nested field.**

**What**: Replaces the gossip config that `NetworkConfig::new()` just set with kona's custom version.

**Why this is necessary**: `NetworkConfig::new()` internally calls `Default::default()` for gossip config. That gives you libp2p's default, which has `flood_publish: true`. kona's gossipsub implementation expects `flood_publish: false` (kona's default builder sets this). With `flood_publish: true`, gossipsub validation fails and the node crashes with `GossipsubCreationFailed`.

**Why it's done as a post-construction override**: `NetworkConfig::new()` doesn't expose a parameter for gossip config. There's no cleaner API. This is a kona API limitation.

**Production**: This is correct and necessary. The comment explains it well. If kona ever fixes `NetworkConfig::new()` to accept gossip config, this can be cleaned up. Until then, this line must stay exactly as-is.

---

## Section 9: Lines 148–169 — EngineConfig (The Biggest Hack)

```rust
            // EngineConfig is required by RollupNodeBuilder but all its fields are dead code
            // in the start_with_client path — the injected ChannelEngineClient is used instead.
            let dummy_jwt = JwtSecret::random();
            let engine_config = EngineConfig {
                config: rollup_cfg.clone(),
                builder_url: cfg.l2_rpc_url.clone(),  // unused
                builder_jwt_secret: dummy_jwt,          // unused
                builder_timeout: Duration::from_secs(5),
                l2_url: cfg.l2_rpc_url.clone(),        // unused
                l2_jwt_secret: dummy_jwt,               // unused
                l2_timeout: Duration::from_secs(5),
                l1_url: cfg.l1_rpc_url.clone(),
                mode: NodeMode::Sequencer,
                rollup_boost: RollupBoostServerArgs {
                    initial_execution_mode: ExecutionMode::Disabled,
                    ...
                },
            };
```

**HACK #4 — Filling in a struct full of unused fields.**

**What**: `RollupNodeBuilder::new()` requires `EngineConfig`. But when you call `rollup_node.start_with_client(client)`, kona skips ALL the EngineConfig connection logic and uses the injected `client` (our `ChannelEngineClient`) instead.

**So every field marked "unused" here is genuinely never read.**

Fields that are dead:
- `builder_url` — URL for a rollup-boost external builder. We don't use rollup-boost.
- `builder_jwt_secret` — JWT for that builder. Filled with a random throw-away secret.
- `l2_url` — URL for connecting to the Engine API via HTTP. Never used because ChannelEngineClient is injected.
- `l2_jwt_secret` — JWT for that connection. Same — throw-away.

Fields that ARE live:
- `config` — kona reads rollup params from here
- `l1_url` — kona uses this to initialise its L1 watcher
- `mode: NodeMode::Sequencer` — tells kona to act as sequencer, not just a verifier
- `rollup_boost.initial_execution_mode: Disabled` — disables rollup-boost

**`dummy_jwt = JwtSecret::random()`**: Every run generates a different JWT secret. If kona accidentally tried to use it, it would fail to authenticate. The randomness is intentional — makes misuse obvious rather than silently succeeding with a weak key.

**Production**: This is a kona API design issue. Ideally `start_with_client` should accept a builder that doesn't require `EngineConfig` at all, since EngineConfig's purpose is to configure the HTTP engine client — which we're replacing. This is tech debt upstream in kona.

If you own the kona fork, the clean fix is:
```rust
// kona API (aspirational):
RollupNodeBuilder::new_with_external_client(rollup_cfg, l1_config, p2p_config, rpc_config)
    .with_sequencer_config(...)
    .build()
// No EngineConfig needed — the client handles all engine communication
```

Until kona exposes that, the dummy EngineConfig approach is the only option.

---

## Section 10: Lines 171–181 — RPC Config (A Subtle Required Hack)

```rust
            // RPC server is REQUIRED: kona's l1_query_tx is only held by the RpcContext.
            // With rpc_config = None, the sender is dropped immediately → L1 watcher
            // gets None from inbound_queries.recv() → StreamEnded crash at startup.
            let rpc_config = RpcBuilder {
                no_restart: false,
                socket: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), cfg.rpc_port),
                enable_admin: true,
                admin_persistence: None,
                ws_enabled: false,
                dev_enabled: false,
            };
```

**HACK #5 — RPC server required not because we need it, but because kona's internals depend on it.**

**The actual bug in kona**: kona passes a query channel sender to the L1 watcher. That sender is held inside `RpcContext`. If you don't start the RPC server, `RpcContext` is never created, the sender is dropped, and the L1 watcher's channel immediately reads `None` → crash.

**Sequence without rpc_config**:
```
kona starts L1 watcher
L1 watcher holds receiver end of l1_query_tx channel
RpcContext (holds sender) is never created because rpc_config = None
Sender drops immediately
L1 watcher: inbound_queries.recv() → None → StreamEnded error → crash
```

**Consequence**: We must start a rollup RPC server on port 9545 even if we didn't want one. We actually DO want one (it's how you query `optimism_syncStatus`) so this is fine in practice — but the reason it's required is an architectural coupling inside kona, not a deliberate design choice.

**`enable_admin: true`**: Exposes `admin_stopSequencer`, `admin_startSequencer`. Required for operational control. Fine.

**`ws_enabled: false`**: No WebSocket on port 9545. The rollup RPC doesn't need WebSocket subscriptions.

**Production**: Acceptable as-is. If kona eventually decouples the L1 watcher from the RPC server lifecycle, the `rpc_config = None` path would become available. For now, always set `Some(rpc_config)`.

---

## Section 11: Lines 183–196 — Build the Rollup Node

```rust
            let rollup_node = RollupNodeBuilder::new(
                (*rollup_cfg).clone(),   // ← deref Arc then clone — creates an owned copy
                l1_config,
                false,                   // l2_trust_rpc
                engine_config,
                p2p_config,
                Some(rpc_config),
            )
            .with_sequencer_config(SequencerConfig {
                sequencer_stopped: false,     // start producing blocks immediately
                l1_conf_delay: cfg.l1_confs, // wait N L1 blocks before including L1 attrs
                ..Default::default()
            })
            .build();
```

**`(*rollup_cfg).clone()`**: `rollup_cfg` is an `Arc<RollupConfig>`. `RollupNodeBuilder::new()` takes an owned `RollupConfig`, not an `Arc`. So we deref the Arc to get `&RollupConfig`, then clone it to get an owned `RollupConfig`. This creates a second copy in memory. Not wrong, just slightly wasteful. Production code would have `RollupNodeBuilder` accept `Arc<RollupConfig>`.

**`l2_trust_rpc: false`**: kona independently verifies L2 state against the L1 derivation. If `true`, kona would just trust whatever the L2 RPC says. Always `false` for a sequencer.

**`sequencer_stopped: false`**: Start sequencing immediately on boot. If `true`, the sequencer starts paused and waits for `admin_startSequencer` call before producing blocks. Useful for HA failover scenarios.

**`..Default::default()`**: The rest of `SequencerConfig` fields use defaults. These include block building interval, max sequencer drift handling, etc. For devnet defaults are fine.

---

## Section 12: Lines 198–215 — Launch and Wait

```rust
            let kona_task = tokio::spawn(async move {
                rollup_node.start_with_client(client).await
            });
```

**What**: Launches kona as a tokio task. `start_with_client` takes ownership of `client` (`Arc<ChannelEngineClient>`) and runs indefinitely.

**Why `tokio::spawn` not `tokio::spawn_blocking`**: kona is async (it's doing I/O: polling L1, waiting for payloads). `spawn_blocking` is for CPU-bound or blocking I/O. Kona is neither.

**`move` closure**: Moves `rollup_node` and `client` into the task. After this line, you cannot access either from outside the task.

---

```rust
            tokio::select! {
                _ = node_handle.node_exit_future => {
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
```

**What**: Waits until ANY of these three happens, then falls through to `Ok(())` and exits.

**Shutdown behaviour analysis**:

| Event | What happens |
|-------|-------------|
| reth exits | Logs, then xlayer-node exits. kona is orphaned (its task is dropped). |
| kona exits with error | `res?.map_err(...)` propagates the error. xlayer-node exits with error. reth is orphaned. |
| ctrl+c | Logs, then xlayer-node exits. Both reth and kona tasks are dropped. |

**HACK #6 — No graceful shutdown coordination.**

When `tokio::select!` completes, the process exits via `Ok(())`. Rust drops everything. The kona task and reth task get their futures dropped — this is an abrupt termination, not a graceful shutdown.

**What's missing in production**:
1. When ctrl+c is received, signal kona to finish its current block
2. When kona signals it's done, signal reth to flush state to MDBX and close cleanly
3. When reth is done, exit

Current behaviour: hard drop on ctrl+c. reth may not flush its last write-ahead log entries cleanly. Unlikely to cause corruption (MDBX is crash-safe) but not ideal.

**Production pattern**:
```rust
tokio::select! {
    _ = node_handle.node_exit_future => { ... }
    res = kona_task => { ... }
    _ = tokio::signal::ctrl_c() => {
        info!("ctrl+c — starting graceful shutdown");
        // 1. Tell kona to stop sequencing
        // 2. Wait for in-flight block to complete
        // 3. node_handle.stop().await  ← reth's graceful shutdown
    }
}
```

---

## Summary: Hacks vs Production-Ready

| # | What | Hack or Fine? | Production Fix |
|---|------|--------------|----------------|
| 1 | Random P2P key every boot | **Hack** | Persist key to file |
| 2 | Hardcoded loopback + port 9223 | **Hack** | Read from config |
| 3 | Gossip config post-construction override | Necessary workaround | Fix kona `NetworkConfig::new()` API |
| 4 | EngineConfig full of dummy/unused fields | Necessary workaround | kona should expose `new_with_external_client` |
| 5 | RPC server required to prevent channel drop crash | Necessary workaround | kona should decouple L1 watcher from RPC lifecycle |
| 6 | No graceful shutdown | **Hack** | Coordinated shutdown sequence |
| 7 | `unsafe set_var` | Minor / acceptable | Set env var in systemd unit instead |
| 8 | `l2_rpc_url` naming confusion | Minor | Rename to `reth_http_url` in config |
| 9 | `launch_with_debug_capabilities` | Fine | Fine in production |
| 10 | `(*rollup_cfg).clone()` double copy | Minor waste | kona should accept `Arc<RollupConfig>` |
| 11 | `RootProvider` with no retries | Risky | Use retry provider wrappers |

### What's genuinely production-ready right now:
- Global allocator (jemalloc) ✓
- Config split (reth CLI flags + kona TOML) ✓
- `start_with_client` pattern (in-process engine) ✓
- `l2_trust_rpc: false` ✓
- `enable_admin: true` ✓
- sigsegv handler ✓

### What must change before production:
1. Persistent P2P key (critical — without it, every restart breaks peer connections)
2. Graceful shutdown (important — prevents dirty restarts under load)
3. Configurable P2P listen address (needed for multi-machine deployments)
4. Retry-wrapped HTTP providers (needed for resilience against RPC flakiness)
5. `unsafe_block_signer` must be set to actual sequencer address (security)
