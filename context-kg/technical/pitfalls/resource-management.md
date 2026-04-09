---
name: "resource-management"
description: "Pitfalls related to memory leaks, timeouts, cache sizing"
---
# Resource Management Pitfalls

[Pitfall] Memory leak in read handles — readHandle must be released or causes leak
- Trigger: readHandle not released after use
- Correct approach: Always call readHandle.Release() in defer block
- Module: op-supervisor (reads/iface.go)

[Pitfall] Rate limit token exhaustion — high rate-limit cost on error may never recover
- Trigger: Client hits error with high token cost
- Correct approach: Implement exponential backoff or peer disconnection
- Module: op-node/p2p (sync.go)

[Pitfall] Payload timeout without retry — fixed timeout without adaptive backoff
- Trigger: Network conditions vary but timeout is static
- Correct approach: Consider adaptive timeouts based on network conditions
- Module: op-node/rollup (engine/payload_process.go)

[Pitfall] L1 cache sizing must scale with sequencing window — ReceiptsCacheSize at 1.5x sequencing window
- Trigger: Undersized cache causes misses during normal derivation
- Correct approach: L1ClientConfig.ReceiptsCacheSize = seqWindowSize * 1.5
- Module: op-service (sources/l1_client.go)

[Pitfall] L2 cache sizing must account for block density — scale with (seqWindowSize * 3/2) * (12 / blockTime)
- Trigger: L2 block density higher than L1; undersized cache
- Correct approach: Scale cache to block time ratio
- Module: op-service (sources/l2_client.go)

[Pitfall] IterativeBatchCall size vs concurrency — batch too large blocks on semaphore
- Trigger: Batch size exceeds MaxConcurrentRequests
- Correct approach: Ensure batch size <= concurrent request limit
- Module: op-service (sources/batching/batching.go)

[Warning] LRU capacity should be by bytes not count — payload buffer uses count-based LRU
- Module: op-node/p2p (sync.go)

[Warning] Hardcoded timeout values — multiple timeouts without tuning mechanism throughout sync and RPC code
- Module: op-node (various)

[Warning] Compression memory not freed on channel close — accumulated compressed bytes in channelQueue
- Module: op-batcher (channel_manager.go)
