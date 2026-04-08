---
name: "concurrency"
description: "Pitfalls related to race conditions, deadlocks, goroutine leaks"
---
# Concurrency Pitfalls

[Pitfall] Mutex deadlock in result buffering — buffered results channel can deadlock if full during promotion
- Trigger: Channel full when trying to promote result
- Correct approach: Skip promotion if channel full, use non-blocking semantics
- Module: op-node/p2p (sync.go)

[Pitfall] Goroutine leak on channel close — indefinite loop if node-event channel closed without reinitialization
- Trigger: Channel close without cleanup
- Correct approach: Detect closed channel and reinitialize immediately
- Module: op-supervisor (syncnode/node.go)

[Pitfall] LimitRPC semaphore leak — semaphore must be released even on error paths
- Trigger: Error return without defer release
- Correct approach: Always use defer for semaphore release
- Module: op-service (sources/limit.go)

[Pitfall] Event system drain blocking — event drain may block if consumers don't read fast enough
- Trigger: High event volume with slow consumers
- Correct approach: Bounded queue with non-blocking dispatch
- Module: op-node (node.go)

[Pitfall] TxManager re-entrance — Send must never be called concurrently for same account; nonces managed internally
- Trigger: Concurrent Send calls
- Correct approach: Sequential nonce management; concurrent calls allowed but serialize internally
- Module: op-service (txmgr/txmgr.go)

[Pitfall] Watch subscriber blocking — Set() blocks until all subscribers accept
- Trigger: Slow or stuck subscriber
- Correct approach: Use buffered channels for Watch subscribers
- Module: op-service (locks/watch.go)

[Pitfall] Disk cleanup race on resolved games — concurrent access during game directory cleanup
- Trigger: Job in queue or worker acting when cleanup runs
- Correct approach: state.inflight gate prevents premature cleanup; serialize with resultQueue pump
- Module: op-challenger (coordinator.go)
