# op-rpc 重启条件计算

## 目标
通过重启 `op-rpc`，使 `FindL2Heads` 回退到合适的 safe head，从而能够接受包含分叉高度的 batch，与 `op-seq` 对齐。

## 符号定义
- `forkHeight`: 分叉 L2 高度
- `forkTimestamp`: 分叉高度的 timestamp
- `batchTimestamp`: batch 第一个 block 的 timestamp
- `batchHeight`: batch 第一个 block 的 L2 高度
- `batchBlockCount`: batch 包含的 block 数量
- `targetSafeHeight`: 目标 safe head 高度
- `targetSafeNextL1Origin`: `targetSafeHeight + 1` 的 L1 origin number
- `targetUnsafeL1Origin`: 重启时 unsafe head 的 L1 origin number
- `targetUnsafeHeight`: 重启时 unsafe head 的 L2 高度
- `seqWindowSize`: 序列窗口大小（L1 blocks）

## 计算步骤

### 1. 确定分叉点
- 分叉高度：`forkHeight`
- 分叉 timestamp：`forkTimestamp`

### 2. 查找包含分叉高度的 batch
从日志中找到所有 batch，使用以下约束条件筛选出可能包含 `forkTimestamp` 的 batch：
```
batchTimestamp <= forkTimestamp <= batchTimestamp + batchBlockCount - 1
```

从筛选出的 batch 中选择日志里最后出现的那个，然后通过以下公式将 batch timestamp 转换为 L2 高度：
```
batchHeight = forkHeight - (forkTimestamp - batchTimestamp)
```


### 3. 确定目标 safe head
通过查询 op-rpc，找到满足以下条件的最小 `targetSafeHeight`：
1. **Height 约束**：
   ```
   batchHeight - 1 <= targetSafeHeight <= batchHeight + batchBlockCount - 2
   ```
2. **SequenceNumber 约束**：
   ```
   (targetSafeHeight + 1).SequenceNumber == 0
   ```

查询命令：
```bash
cast rpc optimism_outputAtBlock $(cast to-hex <height>) -r http://localhost:9555 | jq -r '.blockRef'
```

查询 `targetSafeHeight + 1` 的 L1 origin number，作为 `targetSafeNextL1Origin`。

### 4. 计算重启条件
`FindL2Heads` 从 unsafe head 向后遍历，找到第一个满足条件的区块 `n`（`n.SequenceNumber == 0`），然后返回 `n` 的父区块作为 safe head。

判断条件：
```go
n.L1Origin.Number + seqWindowSize < unsafe_head.L1Origin.Number
```

要让 `FindL2Heads` 找到 `targetSafeHeight + 1` 并返回 `targetSafeHeight`，需要 `targetSafeHeight + 1` 是第一个满足条件的区块：
```
targetSafeNextL1Origin + seqWindowSize = targetUnsafeL1Origin - 1
```

因此，重启条件为：
```
targetUnsafeL1Origin = targetSafeNextL1Origin + seqWindowSize + 1
```

通过查询 op-rpc，找到最小的 `targetUnsafeHeight`，使得该 unsafe height 的 L1 origin = `targetUnsafeL1Origin`。

## 总结

1. **确定分叉点**：`forkHeight`, `forkTimestamp`
2. **查找 batch**：从日志中使用约束条件 `batchTimestamp <= forkTimestamp <= batchTimestamp + batchBlockCount - 1` 筛选 batch，选择最后出现的那个，通过公式计算 `batchHeight`
3. **确定目标 safe head**：通过查询 op-rpc，找到 `targetSafeHeight` 满足：
   - `batchHeight - 1 <= targetSafeHeight <= batchHeight + batchBlockCount - 2`
   - `(targetSafeHeight + 1).SequenceNumber == 0`
   - 查询 `targetSafeHeight + 1` 的 L1 origin number，作为 `targetSafeNextL1Origin`
4. **计算重启条件**：
   - `targetUnsafeL1Origin = targetSafeNextL1Origin + seqWindowSize + 1`
5. **查找最小的 targetUnsafeHeight**：通过查询 op-rpc，找到最小的 `targetUnsafeHeight`，使得该高度的 L1 origin = `targetUnsafeL1Origin`

## 示例（本地测试）

分叉高度：`forkHeight = 8594135`

### 1. 确定分叉点
- `forkHeight = 8594135`
- `forkTimestamp = 1762583920`（从 RPC 查询得到：`cast block 8594135 -r http://localhost:8124`）

### 2. 查找包含分叉高度的 batch
从 `op-rpc.log` 找到最后出现的包含 `forkTimestamp` 的 batch：
- `batchTimestamp = 1762583872`
- `batchBlockCount = 101`

计算 `batchHeight`：
```
batchHeight = 8594135 - (1762583920 - 1762583872) = 8594087
```

### 3. 确定目标 safe head
通过查询 op-rpc（端口 9555），找到满足条件的 `targetSafeHeight`：
- 约束：`8594086 <= targetSafeHeight <= 8594186`，且 `(targetSafeHeight + 1).SequenceNumber == 0`
- 查询命令：`cast rpc optimism_outputAtBlock $(cast to-hex <height>) -r http://localhost:9555 | jq -r '.blockRef | "\(.l1origin.number) \(.sequenceNumber)"'`
- 查询结果：8594087 的 SequenceNumber = 0，L1 origin = 103
- `targetSafeHeight = 8594086`
- `targetSafeNextL1Origin = 103`

### 4. 计算重启条件
```
targetUnsafeL1Origin = 103 + 100 + 1 = 204
```

### 5. 查找最小的 targetUnsafeHeight
通过查询 op-rpc（端口 9555），找到最小的 `targetUnsafeHeight`，使得该高度的 L1 origin = `targetUnsafeL1Origin`。

- 查询命令：`cast rpc optimism_outputAtBlock $(cast to-hex <height>) -r http://localhost:9555 | jq -r '.blockRef.l1origin.number'`
- 查询结果：8594289 的 L1 origin = 204
- `targetUnsafeHeight = 8594289`

**重启时机**：当 `op-rpc` 的 unsafe head 达到高度 8594289 且 L1 origin = 204 时重启。


## 测试网实际计算数据

### 输入参数
- `forkHeight = 12821075`
- `forkTimestamp = 1761279912`
- `batchTimestamp = 1761279302`, `batchBlockCount = 8622`
- `batchHeight = 12820465`

### 计算结果
- `targetSafeHeight = 12820474` (timestamp = 1761279311)
- `targetSafeNextL1Origin = 9477640`
- `targetUnsafeL1Origin = 9481241`
- `targetUnsafeHeight = 12863927`

## 补充：Finalized Height 限制

### 问题
在测试网上，按照以上逻辑计算出 `targetUnsafeHeight` 后重启 `op-rpc`，发现 safe height 并没有回退到 `targetSafeHeight`。

**原因**：`FindL2Heads` 在回退时遇到 finalized height，就会停止回退。如果 finalized height 已经超过了 `targetSafeHeight`，safe head 最多只能回退到 finalized height，无法继续回退到 `targetSafeHeight`。

相关代码逻辑（`op-node/rollup/sync/start.go:244-248`）：
```go
// Don't traverse further than the finalized head to find a safe head
if n.Number == result.Finalized.Number {
    lgr.Info("Hit finalized L2 head, returning immediately", ...)
    result.Safe = n
    return result, nil
}
```

### 解决方案
采用回滚 EL（Execution Layer）的方法：

1. **启动 op-rpc**，等待 finalized height 达到或超过 `targetSafeHeight`
2. **停止 op-rpc**，此时 EL（op-geth 或 op-reth）会停止出块
3. **使用工具回滚 EL** 到 `targetSafeHeight`，此操作会同时回滚 finalized height
4. **重启 op-rpc**，`FindL2Heads` 执行后 safe head 将回退到 `targetSafeHeight`
5. **op-rpc 处理 batch** 的逻辑与线上节点保持一致

### 回滚 op-geth

官方提供了 `op-wheel` 工具来回滚 op-geth。该工具通过 RPC 接口调用 EL，原理如下：
1. 调用 `debug_setHead` RPC 接口回滚数据库
2. 调用 Engine API 的 `forkchoiceUpdated` 接口设置 unsafe、safe 和 finalized 高度

**命令示例**：
```bash
op-wheel engine rewind \
  --engine http://localhost:8553 \
  --engine.jwt-secret-path ./config-op/jwt.txt \
  --to 12820474 \
  --set-head \
  --engine.open http://localhost:8124
```

**参数说明**：
- `--engine`: Engine API 端点（需要 JWT 认证）
- `--engine.jwt-secret-path`: JWT 密钥文件路径
- `--to`: 目标回滚高度
- `--set-head`: 同时回滚数据库（调用 `debug_setHead`）
- `--engine.open`: 开放的 RPC 端点（用于调用 `debug_setHead`）

### 回滚 op-reth

op-reth 没有实现 `debug_setHead` 接口（可以调用，但内部逻辑为空），因此无法使用 `op-wheel` 回滚。但是 reth 本身提供了回滚命令。此时需要先停掉op-reth，然后用如下命令回滚数据库后再启动op-reth

**命令示例**：
```bash
op-reth stage unwind \
  --datadir=/datadir \
  --chain=/genesis.json \
  --config=/config.toml \
  to-block 12820474
```

**参数说明**：
- `--datadir`: 数据目录路径
- `--chain`: Genesis 文件路径
- `--config`: 配置文件路径
- `to-block`: 目标回滚高度

**效果**：数据库会回滚到指定高度，同时 finalized height 也会设置为该高度。






