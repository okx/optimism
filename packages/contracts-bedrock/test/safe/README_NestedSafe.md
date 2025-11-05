# 嵌套 Safe 预批准哈希测试

## 概述

这个测试用例演示了如何使用**预批准哈希（Pre-Approved Hashes）**在嵌套的 Gnosis Safe 多签钱包中执行交易。

## 测试场景

### Safe 配置

```
Safe A (Parent Safe - 3/3 多签)
├── Owner 1: Alice (EOA)
├── Owner 2: Safe B (智能合约 Safe)
└── Owner 3: Carol (EOA)

Safe B (Nested Safe - 2/2 多签)
├── Owner 1: Bob1 (EOA)
└── Owner 2: Bob2 (EOA)
```

### 目标

Safe A 需要转账 **10 ETH** 到接收地址，所有所有者都使用**预批准哈希**方式授权。

## 执行流程

### Phase 1: 计算 Safe A 的交易哈希

```solidity
bytes32 txHashA = safeA.getTransactionHash(
    recipient,       // 接收地址
    10 ether,        // 转账金额
    "",              // 数据
    Call,            // 操作类型
    0, 0, 0,         // Gas 参数
    address(0),      // gasToken
    address(0),      // refundReceiver
    safeA.nonce()    // Safe A 当前 nonce
);
```

### Phase 2: Safe B 批准流程（嵌套批准）

#### 步骤 2.1: 计算 Safe B 的交易哈希
Safe B 需要执行调用 `safeA.approveHash(txHashA)`：

```solidity
bytes memory callData = abi.encodeWithSignature("approveHash(bytes32)", txHashA);
bytes32 txHashB = safeB.getTransactionHash(
    address(safeA),  // 目标：Safe A
    0,               // 无需转账
    callData,        // 调用 approveHash
    ...
);
```

#### 步骤 2.2: Bob1 和 Bob2 批准 Safe B 的交易

```solidity
// Bob1 批准
safeB.approveHash(txHashB);

// Bob2 批准
safeB.approveHash(txHashB);
```

#### 步骤 2.3: 执行 Safe B 的交易

```solidity
// 构建预批准签名（v=1）
bytes memory signatures = buildPreApprovedSignatures([bob1, bob2]);

// 执行 Safe B 的交易，调用 safeA.approveHash(txHashA)
safeB.execTransaction(..., signatures);

// 结果：approvedHashes[SafeB][txHashA] = 1 在 Safe A 中
```

### Phase 3: Alice 批准 Safe A 的交易

```solidity
// Alice 批准
safeA.approveHash(txHashA);

// 注意：Carol 不需要预先批准！
// 因为 Carol 将作为执行者调用 execTransaction
// 验证逻辑会自动通过：msg.sender == currentOwner ✓
```

### Phase 4: Carol 执行 Safe A 的交易

```solidity
// 构建预批准签名（所有所有者使用 v=1）
// 注意：地址必须按升序排列
bytes memory signatures = buildPreApprovedSignatures([alice, safeB, carol]);

// Carol 执行转账（Carol 不需要预先 approve）
vm.prank(carol);
safeA.execTransaction(
    recipient,
    10 ether,
    "",
    Call,
    ...
    signatures
);

// ✅ 成功转账 10 ETH
// Carol 的批准通过 msg.sender == carol 自动验证
```

## 关键技术点

### 1. 预批准哈希签名格式

```solidity
// 每个签名 65 字节：
// r (32 bytes): 所有者地址（右对齐）
// s (32 bytes): 0
// v (1 byte):   1（表示预批准哈希）

bytes memory signature = abi.encodePacked(
    bytes32(uint256(uint160(ownerAddress))),  // r = 地址
    bytes32(0),                                // s = 0
    uint8(1)                                   // v = 1
);
```

### 2. 验证逻辑（执行者优化）

在 `checkNSignatures` 中，当 `v=1` 时：

```solidity
if (v == 1) {
    currentOwner = address(uint160(uint256(r)));  // 从 r 中提取地址

    // 两种批准方式（OR 逻辑）：
    // 1. msg.sender == currentOwner（执行者即所有者）⭐ 不需要预批准！
    // 2. approvedHashes[currentOwner][dataHash] != 0（预批准）
    require(
        msg.sender == currentOwner ||
        approvedHashes[currentOwner][dataHash] != 0,
        "GS025"
    );
}
```

**优化要点**：执行者不需要调用 `approveHash()`，直接执行即可节省一笔交易和 Gas！

### 3. 地址排序要求

**签名必须按所有者地址升序排列**，以防止签名重放：

```solidity
require(
    currentOwner > lastOwner &&
    owners[currentOwner] != address(0),
    "GS026"
);
```

## 运行测试

```bash
# 进入 contracts-bedrock 目录
cd packages/contracts-bedrock

# 运行所有测试
forge test --match-path test/safe/NestedSafePreApprovedHash.t.sol --offline -vv

# 运行主测试用例（详细输出）
forge test --match-test test_nestedSafe_preApprovedHashes_succeeds --offline -vvv

# 运行失败场景测试
forge test --match-test test_nestedSafe_insufficientApprovals_reverts --offline -vv
forge test --match-test test_nestedSafe_wrongOrder_reverts --offline -vv
```

## 测试用例说明

### ✅ `test_nestedSafe_preApprovedHashes_succeeds`
验证完整的嵌套 Safe 预批准流程是否正常工作。

**验证内容：**
- Safe B 的两个所有者（Bob1, Bob2）都批准
- Safe B 执行交易批准 Safe A 的哈希
- Safe A 的两个 EOA 所有者（Alice, Carol）批准
- Safe A 成功执行转账
- 接收者收到正确金额

### ❌ `test_nestedSafe_insufficientApprovals_reverts`
验证当批准数量不足时交易会失败。

**场景：** 只有 Alice 批准（需要 3/3），尝试执行会失败。

### ❌ `test_nestedSafe_wrongOrder_reverts`
验证签名顺序错误时交易会失败。

**场景：** 签名地址未按升序排列会被拒绝。

## Gas 消耗分析

```
Phase              | Transaction        | Gas Cost | 执行者
-------------------|-------------------|----------|-------
Phase 2.2          | safeB.approveHash | ~45k     | Bob1
Phase 2.2          | safeB.approveHash | ~45k     | Bob2
Phase 2.3          | safeB.execTx      | ~150k    | Bob1
Phase 3            | safeA.approveHash | ~45k     | Alice
Phase 4            | safeA.execTx      | ~100k    | Carol ⭐
-------------------|-------------------|----------|-------
Total (优化后)     |                   | ~430k    | 5 txs ✅
Total (优化前)     |                   | ~475k    | 6 txs
节省               |                   | ~45k     | -1 tx
```

**优化**: Carol 作为执行者不需要预先 approve，节省了一笔交易和约 45k Gas！

## 优势与权衡

### ✅ 优势

1. **简单性**: 无需复杂的签名构造
2. **异步性**: 所有者可以在不同时间批准
3. **透明性**: 所有批准都在链上可见
4. **合约兼容**: 完美支持合约所有者（如 Safe B）
5. **灵活性**: 批准后任何人都可以执行

### ⚠️ 权衡

1. **多笔交易**: 需要多次链上交易（vs 一次）
2. **总 Gas 成本**: 累计 Gas 可能更高
3. **时间成本**: 需要等待所有批准完成

## 实际应用场景

1. **DAO 治理**: 成员异步投票批准提案
2. **多组织协调**: 不同组织（各自是 Safe）协同决策
3. **应急响应**: 预批准紧急恢复交易
4. **定时操作**: 提前批准未来交易
5. **合规审计**: 链上可审计的完整批准记录

## 相关文件

- 测试文件: `test/safe/NestedSafePreApprovedHash.t.sol`
- GnosisSafe 合约: `lib/safe-contracts/contracts/GnosisSafe.sol`
- 部署参考: `scripts/deploy/DeployOwnership.s.sol`
