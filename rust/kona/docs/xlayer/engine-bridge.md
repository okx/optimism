# XLayer Engine Bridge — kona-side Design

This document explains the changes made to `kona-engine` and `kona-node-service` for the
**xlayer-node** initiative: running kona (CL/derivation) and op-reth (EL/execution) in
the same OS process, replacing the HTTP Engine API with direct Rust channel communication.

It is intended for kona maintainers reviewing PRs from `feat/xlayer-in-process-engine`.

---

## Background

Standard deployment today:

```
kona-node  ──[HTTP Engine API, JWT auth]──►  op-reth
           ◄──[HTTP response]──────────────
```

Three round trips per block, ~3ms each. At 1s block time, that is ~1% of block time lost to
local loopback transport.

xlayer target:

```
xlayer-node process
├── kona (CL) ──[tokio mpsc channel]──► op-reth (EL)
│              ◄──[oneshot response]──
└── (same OS process, same heap)
```

Same data. Same sequence. Nanoseconds instead of milliseconds. No JWT. No TCP. No JSON.

---

## What this PR changes in kona

### 1. `EngineClient` trait — transport decoupled

**File:** `crates/node/engine/src/client.rs`

**Before:**
```rust
pub trait EngineClient: OpEngineApi<Optimism, Http<HyperAuthClient>> + Send + Sync {
    fn cfg(&self) -> &RollupConfig;
    // ... chain helpers ...
    async fn new_payload_v1(...) -> ...;
    // v2/v3/v4 inherited from OpEngineApi supertrait (HTTP-bound)
}
```

`OpEngineApi<Optimism, Http<HyperAuthClient>>` hardcodes two things:
- `Optimism` — the OP Stack network encoding (still needed)
- `Http<HyperAuthClient>` — TCP socket + JWT auth (this is transport, not interface)

Any struct without `HyperAuthClient` cannot implement `EngineClient`. This blocks
in-process implementations that have no HTTP types.

**After:**
```rust
pub trait EngineClient: Send + Sync {
    fn cfg(&self) -> &RollupConfig;
    // chain helpers unchanged
    async fn new_payload_v1(...) -> TransportResult<PayloadStatus>;
    async fn new_payload_v2(...) -> TransportResult<PayloadStatus>;
    async fn new_payload_v3(...) -> TransportResult<PayloadStatus>;
    async fn new_payload_v4(...) -> TransportResult<PayloadStatus>;
    async fn fork_choice_updated_v2(...) -> TransportResult<ForkchoiceUpdated>;
    async fn fork_choice_updated_v3(...) -> TransportResult<ForkchoiceUpdated>;
    async fn get_payload_v2(...) -> TransportResult<ExecutionPayloadEnvelopeV2>;
    async fn get_payload_v3(...) -> TransportResult<OpExecutionPayloadEnvelopeV3>;
    async fn get_payload_v4(...) -> TransportResult<OpExecutionPayloadEnvelopeV4>;
    async fn l2_block_by_label(...) -> Result<...>;
    async fn l2_block_info_by_label(...) -> Result<...>;
}
```

The 9 engine API methods the task queue actually calls are now explicit on the trait.
`OpEngineApi` is no longer a supertrait — it is kept intact on `OpEngineClient` for any
code that calls it directly.

**`impl EngineClient for OpEngineClient`** delegates each method to the same HTTP
provider as before. Zero behaviour change on the HTTP path.

---

### 2. `MockEngineClient` — one impl block, no HTTP types

**File:** `crates/node/engine/src/test_utils/engine_client.rs`

The mock previously had two impl blocks:
- `impl EngineClient for MockEngineClient` — kona-specific helpers
- `impl OpEngineApi<Optimism, Http<HyperAuthClient>> for MockEngineClient` — Engine API methods

With the supertrait removed, one block covers everything. Dead mock fields removed
(`get_payload_bodies`, `client_versions`, `capabilities`, `protocol_version` — none of
these are called by the task queue via `EngineClient`).

New test **`test_engine_client_is_transport_agnostic`**:
```rust
// Compile-time proof: a struct with zero HTTP types satisfies EngineClient.
fn assert_engine_client<C: EngineClient>(_: &C) {}
assert_engine_client(&mock);

// Object-safety check: can be used as dyn EngineClient.
let boxed: Box<dyn EngineClient> = Box::new(mock);
```

---

### 3. `RollupNode` — injection point for in-process client

**File:** `crates/node/service/src/service/node.rs`

New public method:
```rust
pub async fn start_with_client<E: EngineClient + 'static>(
    &self,
    engine_client: Arc<E>,
) -> Result<(), String>
```

The existing `start()` method is unchanged in behaviour — it still builds `OpEngineClient`
from `EngineConfig` and delegates to a shared private `start_with_engine` body.

`start_with_client` takes any `E: EngineClient`, passes `None` for RollupBoost
(no sidecar in the in-process topology), and delegates to the same shared body.

```
start()              ─┐
                       ├─► start_with_engine<E>(client, rollup_boost) ─► wires actors
start_with_client()  ─┘
```

No code duplication. No config flags. Two clean topologies.

---

### 4. `EngineRpcProcessor` — optional RollupBoost

**File:** `crates/node/service/src/actors/engine/rpc_request_processor.rs`

`rollup_boost_server: Arc<RollupBoostServer>` → `Option<Arc<RollupBoostServer>>`

When `None` (in-process path):
- Health query: responds `ServiceUnavailable` — accurate (service not present)
- Admin query (`SetExecutionMode`, `GetExecutionMode`): logs warning and returns

RollupBoost admin RPC endpoints will simply not be exposed in the xlayer-node binary.

---

## What kona does NOT need to know about

The in-process implementation (`RethInProcessClient`) lives in the xlayer integration
layer — not in kona. Kona only sees the `EngineClient` trait. It does not know or care
whether the implementation behind the trait makes HTTP calls or sends Rust structs over
a channel.

```
kona derivation pipeline
  └── calls EngineClient trait methods
       └── any of:
           ├── OpEngineClient       — HTTP + JWT  (standard deployment)
           └── RethInProcessClient  — channel     (xlayer-node binary)
```

The channel implementation is in `okx/xlayer crates/engine-bridge/`. Its details:
- Sends `BeaconEngineMessage` on a bounded tokio mpsc channel (cap 32)
- Each call creates a oneshot channel for the response
- op-reth's engine tree picks up the message, executes, replies via oneshot
- Total latency: ~200ns vs ~3ms HTTP

---

## What changes in each repo

| Repo | Change |
|------|--------|
| `okx/optimism` (this repo) | `EngineClient` transport-agnostic + `start_with_client` injection point |
| `okx/reth` | Add `RethInProcessClient` that implements `EngineClient` via channels |
| `okx/xlayer` | Binary: starts op-reth, builds `RethInProcessClient`, calls `start_with_client` |

---

## Questions from reviewers

**Q: Won't removing `OpEngineApi` as a supertrait break existing callers?**

A: No. `impl OpEngineApi<Optimism, Http<HyperAuthClient>> for OpEngineClient` is kept
intact. Code calling `engine_client.some_op_engine_api_method()` via that trait still
compiles. The task queue holds `Arc<dyn EngineClient>` — it calls via `EngineClient`, not
via `OpEngineApi`. The two impls are independent.

**Q: Is `EngineClient` object-safe now?**

A: Yes. The test creates `Box<dyn EngineClient>` — if object safety were broken, the test
would not compile. All 9 new methods use concrete types (no generic type parameters), so
they're object-safe.

**Q: What happens if someone calls `start_with_client` twice?**

A: Each call to `start_with_engine` creates fresh channels and actors. Two calls would
spawn two actor sets competing for the same P2P port and L1 provider. This is undefined
behaviour. `start_with_client` is designed as a one-shot startup entry point.

---

## Related commits

| Commit | Message |
|--------|---------|
| `abcc54c` | `refactor(kona-engine): make EngineClient transport-agnostic` |
| `7a3065b` | `feat(kona-node): expose injection point for pluggable engine client` |
