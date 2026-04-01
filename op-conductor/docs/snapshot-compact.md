# Raft Snapshot 与 Compact 调优

本文档记录：snapshot/compact 导致 `CommitUnsafePayload` 超时（context deadline exceeded）的原因、两种缓解方案，以及 snapshot 频率与单次 compact 量的关系。

## 背景

- op-node 每 1s 出块时会调用 conductor 的 `CommitUnsafePayload`，该 RPC **会阻塞直到 Raft 侧 commit 完成**（不是仅入队）。
- Raft 主循环在做 `dispatchLogs` 时会调用 `r.logs.StoreLogs()`；snapshot 流程里 `takeSnapshot` 最后会调用 `r.compactLogs()` → `r.logs.DeleteRange()`。
- 两者共用同一 LogStore（BoltDB），写操作串行。当 compact 正在执行 `DeleteRange` 时，主循环的 `StoreLogs` 会被阻塞，导致 commit 迟迟不完成，op-node 侧 RPC 超时（默认 1s，可调大），出现空块。

## 方案一：分批延后 Compact（已实现）

**思路**：不改变 snapshot 触发逻辑，只把 **DeleteRange** 从「一次性做完」改成「后台分批执行」。

- **实现**：`consensus/deferred_compact_logstore.go` 中的 `DeferredCompactLogStore` 包装底层 LogStore。
- **行为**：`DeleteRange(min, max)` 立即返回，在后台 goroutine 中按批（默认每批 200 条）调用底层 `DeleteRange`，批与批之间 sleep 5ms，让出锁给主循环的 `StoreLogs`。
- **效果**：主循环每次最多被一小批 compact 占用几十～一两百毫秒，而不是整次 2s+，有利于 1s 出块内完成 commit。

**接入位置**：`consensus/raft.go` 中 `NewRaftConsensus` 里，在创建 bolt LogStore 后包一层：

```go
logStore := NewDeferredCompactLogStore(log, boltLogStore, 0, 0)
```

## 方案二：提高 Snapshot 频率以降低单次 Compact 量

**结论**：**提高 snapshot 频率（即更频繁地做 snapshot）可以降低单次 compact 要删除的 log 条数，从而缩短单次 compact 时间。**

### 逻辑简述

- Raft 触发 snapshot 的条件是：自上次 snapshot 以来新增的 log 条数 ≥ **SnapshotThreshold**（`shouldSnapshot`: `lastIdx - lastSnap >= SnapshotThreshold`）。
- 每次 snapshot 完成后会做一次 compact：删除 `[minLog, maxLog]`，其中 `maxLog = min(snapIdx, lastLogIdx - TrailingLogs)`，即保留「当前 snapshot 覆盖的 index」与「末尾 TrailingLogs 条」中更保守的那段，其余旧 log 删掉。
- 若 **SnapshotThreshold 较小**：每积累较少条 log 就做一次 snapshot → 单次 compact 要删的区间更短（约在 SnapshotThreshold 量级），单次 DeleteRange 工作量更小，耗时更短，对主循环的阻塞时间更短。

因此：**减小 SnapshotThreshold = 提高 snapshot 频率 = 降低单次 compact 量**。可配合方案一使用（小批量 + 单次量少，进一步减少阻塞）。

### 当前频率与单次 Compact 相关配置位置

| 配置项 | 含义 | 默认值 | 配置位置（op-conductor） |
|--------|------|--------|--------------------------|
| **raft.snapshot-interval** | 多久检查一次「是否该做 snapshot」 | 120s | `flags/flags.go` → `RaftSnapshotInterval` |
| **raft.snapshot-threshold** | 自上次 snapshot 以来新增多少条 log 即触发 snapshot | 8192 | `flags/flags.go` → `RaftSnapshotThreshold` |
| **raft.trailing-logs** | snapshot 后保留多少条 log 不删（用于复制等） | 10240 | `flags/flags.go` → `RaftTrailingLogs` |

**实际“频率”**：由 **SnapshotThreshold** 主导——每积累 **SnapshotThreshold** 条 log 就会在下次 `SnapshotInterval` 检查时触发一次 snapshot（并随之做一次 compact）。  
**单次 compact 量**：大致与「自上次 compact 以来新增的 log 条数」相关，减小 SnapshotThreshold 会减少单次 compact 的条数和耗时。

### 如何调整（提高频率、降低单次 compact 量）

- **减小 `raft.snapshot-threshold`**（例如从 8192 改为 2048 或 1024）：更频繁 snapshot，单次 compact 删的条数更少，单次 compact 时间更短。
- **适当减小 `raft.snapshot-interval`**（例如从 120s 改为 60s）：更频繁检查，不会改变「每多少条做一次」的逻辑，但能更快响应达到 threshold 的情况。
- **TrailingLogs**：影响保留多少条 log、进而影响 compact 的 `maxLog`；通常保持大于等于 SnapshotThreshold 即可，可按需微调。

环境变量对应（以 op-conductor 的 EnvVarPrefix 为准）：

- `RAFT_SNAPSHOT_INTERVAL`
- `RAFT_SNAPSHOT_THRESHOLD`
- `RAFT_TRAILING_LOGS`

## 相关代码索引

- Snapshot 触发与 compact 逻辑：hashicorp/raft `snapshot.go`（`runSnapshots`、`shouldSnapshot`、`takeSnapshot`、`compactLogsWithTrailing`）。
- op-conductor 传入 Raft 的配置：`conductor/service.go` 中 `SnapshotInterval`、`SnapshotThreshold`、`TrailingLogs` 来自 `config`；config 来自 `conductor/config.go` 与 `flags/flags.go`。
