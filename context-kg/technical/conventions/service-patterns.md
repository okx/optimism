---
name: "service-patterns"
description: "Idempotency, lock, cache, event, transaction management patterns"
---
# Service Patterns

## Transaction Management (TxManager)

[Reuse] TxManager — reliable L1 transaction submission with gas bumping and receipt handling
- Send/SendAsync manage nonces internally
- SuggestGasPriceCaps for fee estimation
- [Rule] 10% price bump for regular tx, 100% for blob tx (geth mempool requirement)
- [Rule] TxManager requires ETHBackend interface; cannot directly use ethclient.Client
- SendState tracks minedTxs, nonceTooLowCount, bumpCount; deadline-based abort

## Event System

[Reuse] Event.System — pub-sub event distribution with priority scheduling
- Register(name, deriver, opts) returns Emitter
- [Rule] Events from same module arrive in order; no cross-emitter ordering guarantee
- [Rule] CriticalErrorEvent sets abort flag; subsequent events skipped
- [Rule] Event.String() returns simple name for metrics labeling
- [Rule] Deriver.OnEvent returns bool — true if event recognized as processed

## Retry Framework

[Reuse] retry.Do[T] / Do0 / Do2 — generic retry with configurable strategy
- ExponentialStrategy: Min, Max, MaxJitter for exponential backoff
- Fixed strategy: constant interval retry
- [Rule] Check ctx.Err() before sleep on retry delay
- [Rule] ErrFailedPermanently returned on max attempts

## Locking Primitives

[Reuse] RWMap[K, V] — thread-safe generic map with RWMutex
- CreateIfMissing for lazy init on first write
- [Rule] Must defer unlock after Lock/RLock; inner map can be nil initially

[Reuse] RWValue[E] — deconflicts reads/writes for single value
- Expose RWMutex and Value for direct access

[Reuse] Watch[E] — watchable value with broadcast notification
- [Rule] Use buffered channel subscribers; Set() blocks until all accept
- Catch() for condition-based waiting with context

## Caching

[Reuse] LRU Cache (hashicorp) — in EthClient, L1Client, L2Client
- [Rule] Size ReceiptsCacheSize to 1.5x sequencing window for L1
- [Rule] Scale L2 cache with (seqWindowSize * 3/2) * (12 / blockTime)
- [Rule] Never cache by block number; only by hash (reorg safety)
- [Rule] Never use cache when querying by label (latest/safe/finalized)

## Signing

[Reuse] SignerFactory — produces SignerFn bound to specific ChainID
- Priority: XLayer signer > remote signer > mnemonic/private key
- [Rule] Never mix private key and mnemonic; error if both provided
- [Rule] Set privKey.PublicKey.Curve = crypto.S256() for geth compatibility

## Safe Math

[Reuse] SafeAdd/SaturatingAdd — overflow-protected arithmetic
- Returns (value, bool) for overflow checks
- SaturatingAdd caps at min/max using saturating variants
