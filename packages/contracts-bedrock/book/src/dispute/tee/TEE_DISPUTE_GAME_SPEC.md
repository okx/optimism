# TEE Dispute Game 规格说明

## 1. 概述

TeeDisputeGame 是 OP Stack 的争议游戏合约，用 **TEE（可信执行环境）ECDSA 签名验证** 替代交互式二分法（FaultDisputeGame）和 ZK 证明验证（OPSuccinctFaultDisputeGame），实现批量状态转移证明。

**目标**：利用 AWS Nitro Enclave 远程证明，实现更快、更低成本的争议解决。TEE 执行器在 enclave 内运行状态转移，用注册的 enclave 密钥签署结果，链上合约验证 ECDSA 签名。

**在 OP Stack 中的定位**：
- 通过标准 `DisputeGameFactory` 创建游戏（Clone 模式）
- 与 `AnchorStateRegistry` 集成，管理锚定状态、最终化和有效性检查
- 使用共享 Types 库中的 `BondDistributionMode`（NORMAL/REFUND）
- 实现 `IDisputeGame` 接口，兼容 OP Stack 标准争议游戏框架（TZ 不使用 OptimismPortal，见 Section 12）
- 游戏类型常量：`1960`

---

## 2. 架构

### 合约关系图

```
                          +---------------------------+
                          |   DisputeGameFactory      |
                          |  (创建 Clone 代理)         |
                          +-----+----------+----------+
                                |          |
                       create() |          | gameAtIndex()
                                v          v
                    +--------------------------+
                    |     TeeDisputeGame       |
                    |   (Clone 代理实例)         |
                    +----+-------+-------+-----+
                         |       |       |
              +----------+  +----+----+  +----------+
              v             v         v             v
    +----------------+ +---------+ +------------------+ +---------------+
    | PROPOSER /     | | Anchor  | | TeeProofVerifier | | IDisputeGame  |
    | CHALLENGER     | | State   | | (enclave ECDSA   | | (接口)         |
    | (不可变地址)    | | Registry| |  签名验证)        | |               |
    +----------------+ +---------+ +-------+----------+ +---------------+
                                           v
                                  +------------------+
                                  | IRiscZeroVerifier|
                                  | (仅用于 enclave  |
                                  |  注册时的 ZK 验证)|
                                  +------------------+
```

### 不可变量（constructor 设置，所有 Clone 共享）

| 不可变量                 | 类型                    | 说明                                      |
|--------------------------|-------------------------|------------------------------------------|
| `GAME_TYPE`              | `GameType`              | 固定为 `GameType.wrap(1960)`              |
| `MAX_CHALLENGE_DURATION` | `Duration`              | 挑战者提交挑战的时间窗口                    |
| `MAX_PROVE_DURATION`     | `Duration`              | 挑战后证明者提交证明的时间窗口              |
| `DISPUTE_GAME_FACTORY`   | `IDisputeGameFactory`   | 创建此游戏的工厂合约                       |
| `TEE_PROOF_VERIFIER`     | `ITeeProofVerifier`     | TEE 签名验证合约                           |
| `CHALLENGER_BOND`        | `uint256`               | 挑战所需的固定保证金金额                    |
| `ANCHOR_STATE_REGISTRY`  | `IAnchorStateRegistry`  | 锚定状态管理合约                           |
| `PROPOSER`               | `address`               | 唯一允许的提议者地址                       |
| `CHALLENGER`             | `address`               | 唯一允许的挑战者地址                       |

### rootClaim 格式

```
rootClaim = keccak256(abi.encode(blockHash, stateHash))
```

blockHash 和 stateHash 通过 extraData 传入。这与 FaultDisputeGame 不同——FDG 的 rootClaim 直接是 output root hash。

### 关键设计说明

- `wasRespectedGameTypeWhenCreated`：仅为兼容 `IDisputeGame` 接口保留，TZ 不使用 OptimismPortal，此字段无实际消费方（见 Section 12）
- 每个游戏实例使用单个 `ClaimData` 结构体（非数组），区别于 FDG 的追加式 DAG

---

## 3. 游戏生命周期

`DisputeGameFactory.create()` 通过 Clone 模式创建游戏实例后立即调用 `initialize()`，进入状态机。

### 状态机

```
                              initialize()
                                  |
                                  v
                          +---------------+
                          |  Unchallenged  |  <-- deadline = now + MAX_CHALLENGE_DURATION
                          +-------+-------+
                                  |
               +------------------+------------------+
               |                                     |
          challenge()                         deadline 过期
               |                                     |
               v                                     v
        +-------------+                     resolve() -> DEFENDER_WINS
        |  Challenged  |  <-- deadline = now + MAX_PROVE_DURATION
        +------+------+
               |
    +----------+----------+
    |                     |
  prove()           deadline 过期
    |                     |
    v                     v
+---------------------------+    resolve() -> CHALLENGER_WINS
| ChallengedAndValid       |
| ProofProvided            |
+----------+---------------+
           |
           v
    resolve() -> DEFENDER_WINS
```

Unchallenged 状态下提前 prove 的路径：

```
  Unchallenged --> prove() --> UnchallengedAndValidProofProvided --> resolve() --> DEFENDER_WINS
```

**重要约束**：`prove()` 内部检查 `gameOver()`，如果 deadline 已过期，prove() 会 revert `GameOver()`。因此 Unchallenged 状态下 prove 只能在 challenge deadline 过期之前调用。

### ProposalStatus 转移表

| 起始状态                            | 动作         | 目标状态                               |
|--------------------------------------|-------------|---------------------------------------|
| `Unchallenged`                      | `challenge()`| `Challenged`                         |
| `Unchallenged`                      | `prove()`   | `UnchallengedAndValidProofProvided`   |
| `Challenged`                        | `prove()`   | `ChallengedAndValidProofProvided`     |
| 任意非 Resolved                      | `resolve()` | `Resolved`                           |

以上是全部合法转移路径，其他任何转移都不应发生。

### GameStatus 转移表

| 条件                                          | 结果             |
|-----------------------------------------------|------------------|
| 父游戏 resolve 为 CHALLENGER_WINS              | CHALLENGER_WINS  |
| Unchallenged + deadline 过期                   | DEFENDER_WINS    |
| Challenged + deadline 过期（无证明）            | CHALLENGER_WINS  |
| UnchallengedAndValidProofProvided              | DEFENDER_WINS    |
| ChallengedAndValidProofProvided                | DEFENDER_WINS    |

### gameOver 条件

当 deadline 过期（严格小于 `block.timestamp`）或有效证明已提交时，游戏"结束"——不再接受 challenge 或 prove。

---

## 4. 挑战-证明模型

### 与 FaultDisputeGame 的关键差异

| 维度 | TeeDisputeGame | FaultDisputeGame |
|------|---------------|------------------|
| 证明机制 | TEE ECDSA 签名（单轮） | 交互式二分法 + VM step（多轮） |
| 解决复杂度 | O(1) | O(n) |
| 保证金托管 | 原生 ETH（直接持有） | DelayedWETH（7 天延迟 + 紧急恢复） |
| 保证金模型 | 固定 CHALLENGER_BOND | 基于位置的 bond 曲线 |
| 时间模型 | 固定 deadline | 棋钟 + 延长 |
| 访问控制 | 白名单 proposer + challenger | 无权限（permissionless） |
| 父子链 | 显式 parentIndex | 无（仅 ASR 锚定） |
| Claim 结构 | 单个 ClaimData | 追加式 ClaimData[] DAG |

### challenge()

仅白名单 CHALLENGER 可调用。提交固定金额保证金（`CHALLENGER_BOND`），将游戏从 Unchallenged 转为 Challenged，并重置 deadline 为 prove 窗口。

### prove()

仅 proposer 可调用——防止第三方抢先提交观察到的证明数据窃取 prover 身份。

**关键设计决策**：
- **提前证明**：prove() 可在 Unchallenged 状态下调用（无需等待挑战），因为 TEE 被信任，有效证明即意味着 claim 正确
- **证明即终局**：一旦证明成功，gameOver() 立即为 true，阻止后续 challenge()——这是有意设计，不是 bug
- **无需保证金**：证明者不需要质押，激励及时响应挑战

---

## 5. TEE 证明安全模型

### 信任链

TEE 证明本质上是 **Owner 控制的备案制**：

```
Owner
  └─ register() ──→ TeeProofVerifier 备案 enclave EOA
                         └─ verifyBatch() ──→ 检查签名者是否已备案
                                                  └─ TeeDisputeGame.prove() 接受
```

**核心信任假设**：合约无条件信任 Owner 注册的 TEE EOA 签署的状态转移。ZK 证明（RISC Zero）仅用于注册环节验证 TEE attestation 的合法性，不参与运行时的 batch 验证。

**信任边界**：
- Owner 有权注册恶意 enclave
- 已注册 enclave 签署的任何 batch digest 都会被接受
- 链上不验证状态转移的正确性，只验证"签名者是否在备案名单中"

### Enclave 生命周期

| 阶段 | 动作 | 控制方 |
|------|------|--------|
| 注册 | `register(seal, attestationData)` — ZK 证明验证 Nitro attestation 后备案 EOA | Owner |
| 运行 | `verifyBatch(digest, signature)` — ecrecover + 检查备案状态 | 任何人（view） |
| 单个撤销 | `revoke(address)` — 移除单个备案 | Owner |
| 批量撤销 | `revokeAll()` — 递增 generation，O(1) 撤销所有备案 | Owner |

### 批量证明验证概述

`prove()` 接受 `BatchProof[]` 数组，验证从 `startingOutputRoot` 到 `rootClaim` 的完整状态转移链：

1. 首个 batch 的起点必须等于锚定状态
2. 相邻 batch 首尾相连（链式连续性）
3. L2 区块号严格单调递增
4. 每个 batch 的 EIP-712 签名由已备案 enclave 签署
5. 末尾 batch 的终点必须等于 rootClaim，区块号等于 l2SequenceNumber

### EIP-712 签名方案

batchDigest 使用 EIP-712 typed data hash，domainSeparator 包含 `block.chainid` + `TEE_PROOF_VERIFIER` 地址，提供跨链和跨部署的 replay 保护。`verifyingContract` 使用 `TEE_PROOF_VERIFIER` 而非游戏实例地址，因为 verifier 是签名验证端点且每条链部署唯一。

---

## 6. 保证金经济学

### 保证金流向

| 角色       | 时机               | 金额                | 计入                              |
|-----------|-------------------|---------------------|----------------------------------|
| Proposer  | `initialize()`    | `msg.value`（任意，无最低限额）  | `refundModeCredit[proposer]`     |
| Challenger| `challenge()`     | `CHALLENGER_BOND`   | `refundModeCredit[challenger]`   |

### resolve() 时的保证金分配

| ProposalStatus                               | 赢家             | 分配方式                                              |
|-----------------------------------------------|------------------|------------------------------------------------------|
| Unchallenged（deadline 过期）                  | Proposer (DEFENDER_WINS) | `normalModeCredit[proposer] = balance`         |
| Challenged（deadline 过期，无证明）             | Challenger (CHALLENGER_WINS) | `normalModeCredit[challenger] = balance`   |
| UnchallengedAndValidProofProvided             | Proposer (DEFENDER_WINS) | `normalModeCredit[proposer] = balance`         |
| ChallengedAndValidProofProvided               | Proposer (DEFENDER_WINS) | `normalModeCredit[proposer] = balance`（proposer 获得全部保证金，因为只有 proposer 能 prove）|
| 父游戏 CHALLENGER_WINS（子游戏已被挑战）        | Challenger (CHALLENGER_WINS) | `normalModeCredit[challenger] = balance`   |
| 父游戏 CHALLENGER_WINS（子游戏未被挑战）        | Proposer 退款 (CHALLENGER_WINS) | `normalModeCredit[proposer] = balance` |

### closeGame() 和 BondDistributionMode

领取保证金前，`closeGame()` 根据 ASR 状态决定分配模式（幂等，只执行一次）：

- **NORMAL 模式**：ASR 判定游戏为 proper（已注册、未黑名单、未退休、未暂停）→ 赢家获得全部保证金
- **REFUND 模式**：ASR 判定游戏非 proper → 各方退还原始存入金额（安全兜底）

暂停期间 closeGame() 会 revert，防止游戏在临时暂停时被永久推入 REFUND 模式。

**与 FaultDisputeGame 的关键区别**：FDG 使用 `DelayedWETH`（deposit/unlock/withdraw 模式）托管保证金，owner 有 `hold()` 紧急恢复函数。TeeDisputeGame 直接在合约中持有 ETH（`address(this).balance`），finality delay 由 `ASR.isGameFinalized()` 强制。

**设计理由**：TZ 的 Proposer 和 Challenger 均为特权白名单地址（非 permissionless），不需要 DelayedWETH 的额外延迟和紧急恢复机制。直接持有 ETH 更简单，ASR 的 finality delay + REFUND 模式已提供足够的安全兜底。

---

## 7. 父子链式关联

### 设计概述

游戏通过 `parentIndex` 引用父游戏（`0xFFFFFFFF` 表示无父游戏，使用 ASR 锚定状态）。子游戏的 `startingOutputRoot` 继承自父游戏的 `rootClaim`。

### 跨类型隔离

父游戏的 GameType 必须与当前游戏一致。TEE 游戏只能链接到其他 TEE 游戏，防止被攻破的 FaultDisputeGame 被用作 TEE 链的起点。

### 父游戏失效级联

- 父游戏未 resolve 时，子游戏不能 resolve（阻塞等待）
- 父游戏 resolve 为 CHALLENGER_WINS → 子游戏自动 CHALLENGER_WINS
  - 子游戏已被挑战：challenger 获得全部保证金
  - 子游戏未被挑战：proposer 保证金被退还（不惩罚无辜 proposer）
- 父游戏 resolve 为 DEFENDER_WINS（或无父游戏）→ 正常解决逻辑

---

## 8. 访问控制

### 角色总览

| 角色 | 合约 | 能力 | 说明 |
|------|------|------|------|
| PROPOSER | TeeDisputeGame（immutable） | 创建游戏（tx.origin）、调用 prove() | 单地址，不可变 |
| CHALLENGER | TeeDisputeGame（immutable） | 调用 challenge() | 单地址，不可变 |
| Owner | TeeProofVerifier（Ownable） | 注册/撤销 enclave | 信任根，详见 Section 5 |
| Guardian | ASR（来自 SystemConfig） | pause/blacklist/retire 游戏 | 间接影响 bond 分配，详见 Section 10 |

**设计说明**：
- Proposer 使用 `tx.origin` 检查（与 PermissionedDisputeGame 一致），Challenger 使用 `msg.sender` 检查
- 两个地址均在 constructor 设置，所有 Clone 实例共享，更改需部署新 implementation
- TeeProofVerifier 的 Owner 可注册任意 Enclave，只要是 AWS Enclave 就可以注册，是整个系统的信任根

---

## 9. 全系统不变量

以下不变量必须在所有状态下成立。审计和测试应围绕证伪这些性质展开。

### 资金安全

**INV-1**: 合约持有的 ETH ≥ sum(normalModeCredit) + sum(refundModeCredit)
— 任何时刻，合约余额不低于所有未领取 credit 之和

**INV-2**: claimCredit() 转出的 ETH 总量 ≤ initialize() 和 challenge() 收到的 ETH 总量
— 合约不会凭空多发 ETH

**INV-3**: REFUND 模式下，每个参与者领取的金额 == 其原始存入金额
— refundModeCredit 精确等于存入时的 msg.value

### 状态机完整性

**INV-4**: ProposalStatus 转移只有以下合法路径：
- Unchallenged → Challenged（仅 challenge()）
- Unchallenged → UnchallengedAndValidProofProvided（仅 prove()）
- Challenged → ChallengedAndValidProofProvided（仅 prove()）
- 任意非 Resolved → Resolved（仅 resolve()）

其他任何转移都不应发生。

**INV-5**: GameStatus 一旦从 IN_PROGRESS 变为 DEFENDER_WINS 或 CHALLENGER_WINS，不可逆转
— status 只能被 resolve() 修改一次

**INV-6**: resolve() 之后，claimData.prover / claimData.counteredBy 不可再变更

### 访问控制

**INV-7**: 只有 PROPOSER（tx.origin）能通过 Factory 创建游戏

**INV-8**: 只有 CHALLENGER（msg.sender）能调用 challenge()

**INV-9**: 只有 proposer（msg.sender，即 initialize 时记录的地址）能调用 prove()

### 证明完整性

**INV-10**: prove() 成功 ⟹ 存在从 startingOutputRoot 到 rootClaim 的完整、连续、单调递增的 batch proof 链，且每个 batch 由当前 generation 的注册 enclave 签名

**INV-11**: 已 resolve 为 DEFENDER_WINS 的游戏必须满足以下之一：
- (a) 未被挑战且 challenge deadline 已过
- (b) 提供了有效 TEE 证明（prover != address(0)）

### Bond 分配

**INV-12**: NORMAL 模式下，恰好一个地址的 normalModeCredit == address(this).balance（resolve 时刻），其余为 0

**INV-13**: bondDistributionMode 一旦从 UNDECIDED 变为 NORMAL 或 REFUND，不可再变更

---

## 10. 外部依赖与信任假设

### 10.1 DisputeGameFactory

**信任级别**：高度信任

**假设**：
- Factory 忠实地调用 initialize()，不会注入恶意 calldata
- Factory 的 gameAtIndex() 返回正确的游戏记录
- Factory 是可升级合约（由 L1 admin 控制），如果被恶意升级：
  - 可伪造 parentIndex 对应的游戏记录
  - 可创建任意 rootClaim 的游戏实例

**风险缓解**：Factory 的升级由 L1 multisig + timelock 控制

### 10.2 AnchorStateRegistry (ASR)

**信任级别**：高度信任

**间接依赖链**：ASR 并非独立合约，其关键能力来自 SystemConfig / SuperchainConfig：

```
ASR.paused()              → SystemConfig.paused() → SuperchainConfig.paused()
ASR._assertOnlyGuardian() → SystemConfig.guardian()
ASR.initialize()          → ProxyAdminOwnedBase（需要 ProxyAdmin 授权）
ASR 升级                   → ProxyAdmin.upgrade()
```

**假设**：
- ASR 的 guardian（来自 SystemConfig）可以 pause / unpause / blacklist / retire 游戏
- ASR pause 期间，closeGame() 会 revert（`TeeDisputeGame.sol:431`），意味着所有进行中游戏的 bond 领取被暂停
- ASR 的 isGameProper() 判定直接决定 NORMAL vs REFUND 模式
- 如果 ASR guardian 被恶意控制：
  - 可通过 blacklist 强制所有游戏进入 REFUND 模式
  - 可通过 pause 永久冻结所有 bond（但不能直接盗取）

**风险缓解**：guardian 由 multisig 控制；REFUND 模式是安全兜底

#### ⏳ 待决策：TZ 的 SystemConfig / SuperchainConfig 部署方案

TZ 自身不使用 SystemConfig 和 SuperchainConfig，但 ASR 依赖它们。两个候选方案：

| 维度 | 方案 A：统一管理（复用 XL） | 方案 B：最小化 Stub |
|------|-----------------------------------|-------------------|
| **方案描述** | TZ 的 ASR `systemConfig` 指向 XL 已部署的 SystemConfig | TZ 部署仅实现 `paused()` + `guardian()` 的轻量合约 |
| **部署成本** | 零额外合约 | 需部署 + 测试一个 stub 合约 |
| **运维成本** | 零（XL 团队统一运维） | 低（功能极简，但需关注上游兼容性） |
| **操作独立性** | ✗ — XL pause = TZ pause；XL guardian 控制 TZ 游戏 | ✓ — TZ 有独立的 pause 开关和 guardian |
| **安全隔离** | ✗ — XL guardian 被攻破时 TZ 同时受影响 | ✓ — TZ 和 XL 完全隔离 |
| **风险耦合** | 高 — XL 因自身原因 pause 时，TZ bond 领取也被冻结 | 无 |
| **上游兼容性** | ✓ — 使用标准 SystemConfig，上游升级无影响 | ⚠️ — 上游 ASR 若调用更多 SystemConfig 函数，stub 可能不兼容 |
| **适用前提** | TZ 和 XL 同一团队运营 | TZ 需要独立控制权，但不想 fork ASR |

### 10.3 IRiscZeroVerifier

**信任级别**：信任其正确性

**假设**：
- verify(seal, imageId, journalDigest) 正确验证 RISC Zero Groth16 证明
- 如果 verifier 有 bug：可注册非法 enclave 地址 → 伪造状态转移

**风险缓解**：
- `riscZeroVerifier` 为 immutable，部署后不可替换。如需更换 verifier 需部署新的 TeeProofVerifier 合约
- `imageId` 为 immutable，部署后不可更改。如需更换 guest image 需部署新合约

### 10.4 AWS Nitro Root Key

**信任级别**：信任 AWS 硬件安全

**假设**：
- expectedRootKey 是 AWS Nitro 的官方 P384 公钥
- AWS 可能轮换 root key（历史上未发生，但理论上可能）

**生命周期管理**：
- `expectedRootKey` 在构造器中设置，部署后不可更改（无 setter 函数）
- 如果 AWS 轮换 root key：需部署新的 TeeProofVerifier → 部署新的 TeeDisputeGame implementation 指向新 verifier
- 此设计牺牲了运行时灵活性，换取了更小的 Owner 攻击面（Owner 无法在运行时替换 verifier/imageId/rootKey）

### 10.5 TEE Enclave 硬件

**信任级别**：信任 enclave 在未被攻破时正确执行

**假设**：
- 注册的 enclave 忠实执行状态转移并签名
- 如果 enclave 被攻破（旁路攻击、供应链攻击等）：
  - 可签署任意虚假状态转移
  - 需要 Owner 通过 revoke() 或 revokeAll() 撤销
  - 在撤销前，已签名的虚假 proof 可能已被提交

---

## 11. 应急机制与升级路径

### 11.1 Enclave 撤销机制

**单个撤销**：
- `TeeProofVerifier.revoke(address)` — Owner 移除单个 enclave

**批量撤销（Generation 机制）**：
- `TeeProofVerifier.revokeAll()` — Owner 递增 enclaveGeneration
- 效果：所有当前 generation 的 enclave 立即失效（O(1)）
- ⚠️ 已提交的 proof 不受影响 — prove() 在调用时验证签名，如果 enclave 在 prove() 调用之前被撤销，该 proof 将失败
- ⚠️ 已 resolve 的游戏不受影响 — 即使事后发现 enclave 被攻破，已完成的游戏状态不可逆转。应通过 ASR blacklist 处理

### 11.2 ASR Pause 对进行中游戏的影响

| 游戏阶段 | Pause 影响 |
|----------|-----------|
| Unchallenged（等待挑战） | challenge() 不受影响（无 pause 检查）|
| Challenged（等待证明） | prove() 不受影响（无 pause 检查）|
| 等待 resolve | resolve() 不受影响（无 pause 检查）|
| 已 resolve，等待 closeGame | closeGame() 被阻塞 → bond 无法领取 |
| 已 close，等待 claimCredit | claimCredit() 不受影响（close 是幂等的）|

**关键结论**：pause 只影响 bond 领取，不影响游戏逻辑本身。长时间 pause 不会导致资金丢失，但会延迟资金释放。

### 11.3 升级路径

**TeeDisputeGame**：
- Clone proxy 模式，implementation 不可升级
- 如需修复漏洞：部署新 implementation → Factory 注册新 gameType → ASR retire 旧 gameType
- 已创建的旧游戏继续运行，但无法作为新游戏的 parent（因为 parentGameType != 新 GAME_TYPE）

**TeeProofVerifier**：
- 非 proxy，不可升级
- riscZeroVerifier / imageId / expectedRootKey 均为不可变参数，部署后无法更改
- Owner 仅保留 enclave 注册/撤销权限
- 如需更换 verifier / imageId / rootKey：部署新的 TeeProofVerifier → 部署新的 TeeDisputeGame implementation 指向新 verifier

### 11.4 应急 SOP（建议）

如果发现 TEE enclave 被攻破：
1. Owner 调用 `revokeAll()` 撤销所有 enclave
2. Guardian 通过 ASR blacklist 被攻破 enclave 签名的游戏
3. 排查受影响游戏，blacklist 后这些游戏进入 REFUND 模式
4. 重新注册可信 enclave
5. 新游戏从 anchor state 继续

---

## 12. 超出范围的威胁

以下威胁被认为超出本合约系统的防御范围：

1. **TEE 硬件级攻击**：旁路攻击、电压故障注入等物理攻击。缓解依赖 AWS Nitro 硬件安全保证。

2. **L1 Reorg**：深度 L1 重组可能导致已 resolve 的游戏状态回滚。这是 L1 共识层风险，非合约层可防御。

3. **Owner / Guardian 密钥泄露**：如果 Owner multisig 被完全攻破，攻击者可注册恶意 enclave（但无法替换 verifier、imageId 或 rootKey，因为这些是不可变参数）。缓解依赖密钥管理实践和 multisig/timelock 配置。

4. **L1 Gas Price 攻击**：攻击者通过操纵 L1 gas price 阻止 challenger/prover 在 deadline 内提交交易。缓解依赖合理设置 MAX_CHALLENGE_DURATION 和 MAX_PROVE_DURATION。

5. **跨链 MEV**：利用 L1/L2 之间的信息不对称进行的套利。不在合约层面防御。

6. **DisputeGameFactory 升级攻击**：Factory 由 L1 governance 控制，恶意升级可绕过所有游戏安全假设。依赖治理安全。

7. **OptimismPortal 提款证明**：TZ 不使用 OptimismPortal 进行 L1↔L2 提款。TZ 的 dispute game 仅用于将 state root 和 TEE proof 公布在 L1 上，跨链桥和提款机制不依赖游戏结果。因此 OptimismPortal 相关的安全假设（`wasRespectedGameTypeWhenCreated`、withdrawal finality 等）不在 TZ 的审计范围内。
