# TEE Dispute Game 本地部署联调指南

> 供 TEE ZK Prover 对接联调使用。全部通过 forge script / cast 命令行操作。

## 目录

- [架构概览](#架构概览)
- [真实 vs Mock 对照](#真实-vs-mock-对照)
- [脚本一览](#脚本一览)
- [前置条件](#前置条件)
- [Mock 模式（快速验证）](#mock-模式快速验证)
  - [Step 1: 启动 Anvil](#step-1-启动-anvil)
  - [Step 2: 部署合约](#step-2-部署合约)
  - [Step 3: 运行 E2E](#step-3-运行-e2e)
  - [Step 4: 领取 Bond](#step-4-领取-bond)
- [Fork 模式（真实 ZK Proof 验证）](#fork-模式真实-zk-proof-验证)
  - [概述](#概述)
  - [Step 1: Fork 主网启动 Anvil](#step-1-fork-主网启动-anvil)
  - [Step 2: 部署合约（真实 RiscZero Verifier）](#step-2-部署合约真实-risczero-verifier)
  - [Step 3: 运行 E2E（真实 seal + 外部签名）](#step-3-运行-e2e真实-seal--外部签名)
  - [Journal 字段解析](#journal-字段解析)
- [Prover 对接核心概念](#prover-对接核心概念)
  - [注册 Enclave](#注册-enclave)
  - [prove() 输入格式](#prove-输入格式)
  - [从外部传入 prove 输入](#从外部传入-prove-输入)
  - [EIP-712 签名规范](#eip-712-签名规范)
  - [多 Batch 链式证明](#多-batch-链式证明)
- [单步 cast 调用参考](#单步-cast-调用参考)
- [数据结构参考](#数据结构参考)
- [常见问题排查](#常见问题排查)

---

## 架构概览

```
+-------------------------------------------------------------+
|                    TEE ZK Prover (你的服务)                    |
|                                                              |
|  1. 生成 Nitro Attestation (mock)                             |
|  2. 生成 ZK Proof of Attestation (mock -> 空 seal)            |
|  3. 调用 register() 注册 enclave                              |
|  4. 用 enclave 私钥对 batch 数据做 EIP-712 ECDSA 签名          |
|  5. 调用 prove() 提交 batch proof                             |
+-------------+--------------------------------+---------------+
              |                                |
              v                                v
+------------------------+      +----------------------------+
|  TeeProofVerifier      |      |   TeeDisputeGame           |
|                        |      |                            |
|  register(seal, att)   |<-----|  prove(batchProofs)        |
|    -> ZK 验证 (mock)    |      |    -> verifyBatch(digest,  |
|    -> 存储 enclave      |      |       signature)           |
|                        |      |                            |
|  verifyBatch(digest,   |<-----|  (ECDSA recover ->         |
|    signature)          |      |   检查是否已注册)            |
+----------+-------------+      +----------------------------+
           |
           v
+------------------------+
| MockRiscZeroVerifier   |
|  (verify -> 直接通过)    |
+------------------------+
```

## 真实 vs Mock 对照

**合约层面：**

| 合约 | 真实 / Mock | 说明 |
|---|---|---|
| `MockRiscZeroVerifier` | **Mock** | `verify()` 直接通过，接受任意 seal |
| `TeeProofVerifier` | **真实** | 真实的 enclave 注册 + ECDSA batch 验证逻辑 |
| `DisputeGameFactory` | **真实** | 通过 Proxy 部署，创建 game 实例 |
| `AnchorStateRegistry` | **真实** | 通过 Proxy 部署，管理 anchor state |
| `TeeDisputeGame` | **真实** | 完整 game 逻辑：initialize, challenge, prove, resolve |
| `MockSystemConfig` | **Mock** | 返回 guardian 地址和 pause 状态 |

**`prove()` 流程中的各部分：**

| 部分 | 真实 / Mock | 生产环境对应 |
|---|---|---|
| `startBlockHash/stateHash` | Mock 数据（可外部传入） | TEE prover 从 L2 链上读取 |
| `endBlockHash/stateHash` | Mock 数据（可外部传入） | TEE prover 执行后计算得到 |
| `l2Block` | Mock 数据（可外部传入） | 真实 L2 区块号 |
| EIP-712 digest 计算 | **真实** | 链上合约用相同逻辑重算 |
| ECDSA 签名 | **真实** | enclave 私钥签署 EIP-712 digest |
| `verifyBatch()` ecrecover | **真实** | 恢复 signer 地址，检查注册状态 |

整个 prove 流程中唯一 mock 的是**被签名的数据**（block/state hash 默认是假值，但可以通过环境变量替换为真实数据）。签名的生成和验证链路与生产环境完全一致。

## 脚本一览

| 脚本 | 用途 | RiscZero Verifier |
|---|---|---|
| `scripts/tee/DeployTeeMock.s.sol` | Mock 模式部署 | MockRiscZeroVerifier（任意 seal 通过） |
| `scripts/tee/TeeProveE2E.s.sol` | Mock E2E（本地签名） | 同上 |
| `scripts/tee/DeployTeeFork.s.sol` | Fork 模式部署 | 主网真实 RiscZeroVerifierRouter |
| `scripts/tee/TeeProveE2EFork.s.sol` | Fork E2E（真实 seal + 外部签名） | 同上 |

---

## 前置条件

- 已安装 [Foundry](https://book.getfoundry.sh/getting-started/installation)（`forge`、`cast`、`anvil`）
- 已 clone 仓库并安装依赖

## Mock 模式（快速验证）

### 快速开始

```bash
# Terminal 1: 启动 Anvil
anvil --block-time 1

# Terminal 2: 部署全部合约
PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
PROPOSER=0x70997970C51812dc3A010C7d01b50e0d17dc79C8 \
CHALLENGER=0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC \
forge script scripts/tee/DeployTeeMock.s.sol \
  --rpc-url http://localhost:8545 --broadcast

# 从输出中复制 TEE_PROOF_VERIFIER 和 DISPUTE_GAME_FACTORY 地址，然后运行 E2E：
PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
PROPOSER_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d \
CHALLENGER_KEY=0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a \
ENCLAVE_KEY=0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6 \
TEE_PROOF_VERIFIER=<部署输出的地址> \
DISPUTE_GAME_FACTORY=<部署输出的地址> \
forge script scripts/tee/TeeProveE2E.s.sol \
  --rpc-url http://localhost:8545 --broadcast
```

预期输出：

```
=== Step 1: Register Enclave (mock attestation + mock ZK proof) ===
  Enclave registered: 0x90F79bf6EB2c4f870365E785982E1f101E93b906

=== Step 2: Create Game (proposer) ===
  Game created: 0xd8058efe0198ae9dD7D563e1b4938Dcbc86A1F81
  l2SequenceNumber: 100
  proposer: 0x70997970C51812dc3A010C7d01b50e0d17dc79C8

=== Step 3: Challenge (challenger) ===
  Game challenged by: 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC

=== Step 4: Prove - single batch (proposer submits, enclave signs) ===
  Domain separator from game: 0x7d2b73...
  Batch signed, signature length: 65
  Proof submitted successfully!

=== Step 5: Resolve ===
  Game resolved: DEFENDER_WINS

=== E2E Complete (steps 1-5 passed) ===
```

### Step 1: 启动 Anvil

```bash
anvil --block-time 1
```

默认账户（每个预充 10000 ETH）：

| 角色 | 私钥 | 地址 |
|---|---|---|
| Deployer / Owner | `0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80` | `0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266` |
| Proposer | `0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d` | `0x70997970C51812dc3A010C7d01b50e0d17dc79C8` |
| Challenger | `0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a` | `0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC` |
| Enclave (TEE Prover) | `0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6` | `0x90F79bf6EB2c4f870365E785982E1f101E93b906` |

### Step 2: 部署合约

脚本：`scripts/tee/DeployTeeMock.s.sol`

部署内容：
1. `MockRiscZeroVerifier` -- `verify()` 直接通过
2. `TeeProofVerifier` -- 使用 mock verifier，但注册和 ECDSA 验证逻辑是真实的
3. `DisputeGameFactory` -- 通过 Proxy 部署
4. `AnchorStateRegistry` -- 通过 Proxy 部署，finality delay = 0
5. `TeeDisputeGame` 实现合约 -- 注册为 game type 1960

测试用配置（脚本内硬编码）：

| 参数 | 值 |
|---|---|
| `DEFENDER_BOND` | 0.1 ETH |
| `CHALLENGER_BOND` | 0.2 ETH |
| `MAX_CHALLENGE_DURATION` | 300 秒（5 分钟） |
| `MAX_PROVE_DURATION` | 300 秒（5 分钟） |
| `TEE_GAME_TYPE` | 1960 |
| `ANCHOR_L2_BLOCK` | 0 |

```bash
PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
PROPOSER=0x70997970C51812dc3A010C7d01b50e0d17dc79C8 \
CHALLENGER=0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC \
forge script scripts/tee/DeployTeeMock.s.sol \
  --rpc-url http://localhost:8545 --broadcast
```

保存输出的地址，下一步需要用到：

```
=== Deployed Addresses ===
MockRiscZeroVerifier : 0x5FbDB2315678afecb367f032d93F642f64180aa3
TeeProofVerifier     : 0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512   <-- 需要
DisputeGameFactory   : 0xCf7Ed3AccA5a467e9e704C703E8D87F634fB0Fc9   <-- 需要
AnchorStateRegistry  : 0xa513E6E4b8f2a923D98304ec87F64353C4D5C853
TeeDisputeGame impl  : 0x8A791620dd6260079BF849Dc5567aDC3F2FdC318
```

### Step 3: 运行 E2E

脚本：`scripts/tee/TeeProveE2E.s.sol`

依次执行 5 个步骤：

1. **注册 Enclave** -- `register("", attestationData)`，seal 传空字节（mock ZK proof）
2. **创建 Game** -- `factory.create()`，proposer 存入 defender bond
3. **挑战** -- `game.challenge()`，challenger 存入 challenger bond
4. **提交证明** -- 构造 EIP-712 digest，用 enclave 私钥签名，调用 `game.prove()`
5. **解决** -- `game.resolve()` 返回 `DEFENDER_WINS`

```bash
PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
PROPOSER_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d \
CHALLENGER_KEY=0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a \
ENCLAVE_KEY=0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6 \
TEE_PROOF_VERIFIER=0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512 \
DISPUTE_GAME_FACTORY=0xCf7Ed3AccA5a467e9e704C703E8D87F634fB0Fc9 \
forge script scripts/tee/TeeProveE2E.s.sol \
  --rpc-url http://localhost:8545 --broadcast
```

### Step 4: 领取 Bond

`claimCredit` 需要满足 `resolvedAt + finalityDelay < block.timestamp`。由于 forge script 中所有交易在同一个区块执行，必须单独调用。等待至少 1 秒后：

```bash
# 将 <GAME_ADDRESS> 替换为 Step 3 输出的 Game created 地址
cast send <GAME_ADDRESS> 'claimCredit(address)' \
  0x70997970C51812dc3A010C7d01b50e0d17dc79C8 \
  --private-key 0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d \
  --rpc-url http://localhost:8545
```

---

## Fork 模式（真实 ZK Proof 验证）

### 概述

Fork 模式通过 `anvil --fork-url` fork 以太坊主网，使用链上已部署的 **RiscZeroVerifierRouter** (`0x8EaB2D97Dfce405A1692a21b3ff3A172d593D319`) 进行真实的 Groth16 ZK proof 验证。

与 mock 模式的区别：

| | Mock 模式 | Fork 模式 |
|---|---|---|
| RiscZero Verifier | MockRiscZeroVerifier（任意 seal 通过） | 主网真实 RiscZeroVerifierRouter |
| `register()` seal | 空字节 `0x` | 真实 Groth16 seal（如 Boundless 返回的） |
| imageId | 假值 | 真实 guest image ID |
| expectedRootKey | 假值 | 真实 AWS Nitro P384 root key（96 字节） |
| prove() 签名 | ENCLAVE_KEY 本地签名 | 外部传入 `BATCH_SIGNATURE` |

### Step 1: Fork 主网启动 Anvil

```bash
# 需要以太坊主网 RPC（Alchemy / Infura / 自建节点）
anvil --fork-url $ETH_RPC_URL --block-time 1
```

> 注意：fork 主网后 chainId 为 1（非 mock 模式的 31337）。Anvil 默认账户同样预充 10000 ETH。prover 构造 EIP-712 签名时 chainId 必须使用 1。

### Step 2: 部署合约（真实 RiscZero Verifier）

脚本：`scripts/tee/DeployTeeFork.s.sol`

```bash
PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
PROPOSER=0x70997970C51812dc3A010C7d01b50e0d17dc79C8 \
CHALLENGER=0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC \
RISC_ZERO_VERIFIER=0x8EaB2D97Dfce405A1692a21b3ff3A172d593D319 \
RISC_ZERO_IMAGE_ID=0x<guest image id bytes32> \
NITRO_ROOT_KEY=0x<96 字节 AWS Nitro P384 root key hex> \
forge script scripts/tee/DeployTeeFork.s.sol \
  --rpc-url http://localhost:8545 --broadcast
```

**环境变量说明：**

| 变量 | 说明 |
|---|---|
| `RISC_ZERO_VERIFIER` | 主网 RiscZeroVerifierRouter 地址 |
| `RISC_ZERO_IMAGE_ID` | RISC Zero guest program 的 image ID（ELF hash） |
| `NITRO_ROOT_KEY` | AWS Nitro Enclave P384 root public key，96 字节（不含 0x04 前缀） |

保存输出的 `TeeProofVerifier` 和 `DisputeGameFactory` 地址。

### Step 3: 运行 E2E（真实 seal + 外部签名）

脚本：`scripts/tee/TeeProveE2EFork.s.sol`

```bash
PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
PROPOSER_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d \
CHALLENGER_KEY=0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a \
TEE_PROOF_VERIFIER=<部署输出的地址> \
DISPUTE_GAME_FACTORY=<部署输出的地址> \
SEAL=0x<Boundless 返回的 groth16 seal hex> \
ATT_TIMESTAMP_MS=<attestation 中的 timestampMs> \
ATT_PCR_HASH=0x<attestation 中的 pcrHash> \
ATT_PUBLIC_KEY=0x<65 字节未压缩公钥 hex> \
ATT_USER_DATA=0x<userData hex，可为空 0x> \
BATCH_SIGNATURE=0x<TEE prover 签名的 65 字节 r+s+v hex> \
END_BLOCK_HASH=0x<终态 block hash> \
END_STATE_HASH=0x<终态 state hash> \
L2_SEQUENCE_NUMBER=<L2 区块号> \
forge script scripts/tee/TeeProveE2EFork.s.sol \
  --rpc-url http://localhost:8545 --broadcast
```

**环境变量说明：**

| 变量 | 说明 |
|---|---|
| `SEAL` | Boundless 返回的 Groth16 proof seal（hex 编码） |
| `ATT_TIMESTAMP_MS` | attestation 中的 Unix 时间戳（毫秒） |
| `ATT_PCR_HASH` | PCR0 hash（bytes32） |
| `ATT_PUBLIC_KEY` | enclave 的 65 字节未压缩 secp256k1 公钥 |
| `ATT_USER_DATA` | attestation 中的附加数据（可为空 `0x`） |
| `BATCH_SIGNATURE` | TEE prover 对 EIP-712 batch digest 的签名（65 字节） |

### Journal 字段解析

`register()` 时合约会用 `attestationData` + `expectedRootKey` 重建 journal，然后计算 `journalDigest = SHA256(journal)` 交给 RiscZero verifier 验证。

如果你有 Boundless 返回的原始 journal（hex），按以下顺序拆解为环境变量：

```
journal = timestampMs (8 bytes)       --> ATT_TIMESTAMP_MS（转为 uint64 十进制）
       || pcrHash     (32 bytes)      --> ATT_PCR_HASH（0x 前缀 hex）
       || rootKey     (96 bytes)      --> 跳过（合约从 expectedRootKey 取，部署时通过 NITRO_ROOT_KEY 设置）
       || pubKeyLen   (1 byte = 0x41) --> 跳过
       || publicKey   (65 bytes)      --> ATT_PUBLIC_KEY（0x 前缀 hex）
       || userDataLen (2 bytes)       --> 跳过
       || userData    (variable)      --> ATT_USER_DATA（0x 前缀 hex，可为空 0x）
```

**示例：用 cast 从 journal hex 中提取字段**

```bash
JOURNAL=0x<完整 journal hex>

# timestampMs: 前 8 字节 = 16 hex chars
ATT_TIMESTAMP_MS=$(cast to-dec $(echo $JOURNAL | cut -c3-18))

# pcrHash: 接下来 32 字节 = 64 hex chars (offset 18)
ATT_PCR_HASH=0x$(echo $JOURNAL | cut -c19-82)

# 跳过 rootKey: 96 字节 = 192 hex chars (offset 82)
# pubKeyLen: 1 字节 = 2 hex chars (offset 274), 值应为 0x41 = 65
# publicKey: 65 字节 = 130 hex chars (offset 276)
ATT_PUBLIC_KEY=0x$(echo $JOURNAL | cut -c277-406)

# userDataLen: 2 字节 = 4 hex chars (offset 406)
USER_DATA_LEN=$(cast to-dec 0x$(echo $JOURNAL | cut -c407-410))
# userData: 从 offset 410 开始
if [ "$USER_DATA_LEN" -gt 0 ]; then
  ATT_USER_DATA=0x$(echo $JOURNAL | cut -c411-$((410 + USER_DATA_LEN * 2)))
else
  ATT_USER_DATA=0x
fi
```

---

## Prover 对接核心概念

### 注册 Enclave

```solidity
function register(bytes calldata seal, AttestationData calldata attestationData) external onlyOwner
```

Mock 模式下使用 `MockRiscZeroVerifier`，ZK proof 验证被跳过。只需提供：

- `seal`：空字节 `0x`
- `attestationData.publicKey`：**65 字节 secp256k1 未压缩公钥**（`0x04` + 32 字节 x + 32 字节 y）
- `attestationData.timestampMs`：任意 uint64
- `attestationData.pcrHash`：任意 bytes32
- `attestationData.userData`：可为空

合约通过 `keccak256(x || y)` 从公钥中提取 Ethereum 地址。后续 `verifyBatch()` 会通过 ECDSA recover 得到 signer 地址，与这个注册地址进行比对。

**关键**：用于签名 batch 的私钥必须与注册时提供的公钥是同一对密钥。

Fork 模式下需要提供真实的 seal 和 attestation data，详见 [Fork 模式](#fork-模式真实-zk-proof-验证)。

### prove() 输入格式

```solidity
function prove(bytes calldata proofBytes) external returns (ProposalStatus)
```

`proofBytes` = `abi.encode(BatchProof[])`：

```solidity
struct BatchProof {
    bytes32 startBlockHash;
    bytes32 startStateHash;
    bytes32 endBlockHash;
    bytes32 endStateHash;
    uint256 l2Block;
    bytes   signature;       // 65 字节：r(32) + s(32) + v(1)
}
```

链上对每个 batch 的验证逻辑：

1. `keccak256(abi.encode(proofs[0].startBlockHash, startStateHash))` == `startingOutputRoot.root`（起始状态匹配 anchor）
2. `proofs[i].end == proofs[i+1].start`（链式连续性）
3. `proofs[i].l2Block < proofs[i+1].l2Block`（单调递增）
4. 链上重算 EIP-712 digest + signature，通过 `TeeProofVerifier.verifyBatch()` 验证
5. `keccak256(abi.encode(proofs[last].endBlockHash, endStateHash))` == `rootClaim`（终态匹配 rootClaim）
6. `proofs[last].l2Block` == `l2SequenceNumber`（最终区块号匹配）

### 从外部传入 prove 输入

两套 E2E 脚本均支持通过环境变量覆盖 batch 数据（不设时使用 mock 默认值）：

| 变量 | 默认值 | 说明 |
|---|---|---|
| `START_BLOCK_HASH` | `keccak256("genesis-block")` | batch 起始 block hash，必须匹配 anchor |
| `START_STATE_HASH` | `keccak256("genesis-state")` | batch 起始 state hash，必须匹配 anchor |
| `END_BLOCK_HASH` | `keccak256("end-block-100")` | batch 终态 block hash |
| `END_STATE_HASH` | `keccak256("end-state-100")` | batch 终态 state hash |
| `L2_SEQUENCE_NUMBER` | `100` | L2 区块号 |

**签名来源的区别：**

| 脚本 | 签名方式 | 说明 |
|---|---|---|
| `TeeProveE2E.s.sol` | `ENCLAVE_KEY` 本地签名 | mock 模式，私钥在本地 |
| `TeeProveE2EFork.s.sol` | `BATCH_SIGNATURE` 外部传入 | fork 模式，私钥留在 TEE 内 |

**查询当前 anchor state（用于确定 START_BLOCK_HASH / START_STATE_HASH）：**

```bash
cast call $ANCHOR_STATE_REGISTRY 'getAnchorRoot()(bytes32,uint256)' \
  --rpc-url http://localhost:8545
```

默认部署的 anchor root = `keccak256(abi.encode(keccak256("genesis-block"), keccak256("genesis-state")))`，l2SequenceNumber = 0。

**注意事项：**
- `START_BLOCK_HASH` / `START_STATE_HASH` 必须满足 `keccak256(abi.encode(startBlockHash, startStateHash))` 等于链上 anchor root。
- `END_BLOCK_HASH` / `END_STATE_HASH` 的 `keccak256(abi.encode(...))` 会作为 `rootClaim` 写入 game。
- `L2_SEQUENCE_NUMBER` 必须大于 anchor 的 l2SequenceNumber。
- `BATCH_SIGNATURE`（仅 fork 模式）必须是对正确 EIP-712 digest 的签名，签名者必须是已注册的 enclave，格式：`abi.encodePacked(r, s, v)` = 65 字节。

### EIP-712 签名规范

这是 prover 对接最关键的部分。domain、types、字段顺序有任何偏差都会导致 `verifyBatch()` revert。

**Domain：**

```
name:              "TeeDisputeGame"
version:           "1"
chainId:           <当前链 ID>   (mock 模式 = 31337, fork 主网 = 1)
verifyingContract: <TeeProofVerifier 地址>   (注意：不是 game 地址！)
```

**Type：**

```
BatchProof(bytes32 startBlockHash,bytes32 startStateHash,bytes32 endBlockHash,bytes32 endStateHash,uint256 l2Block)
```

**Domain separator（链上计算方式）：**

```
domainSeparator = keccak256(abi.encode(
    keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
    keccak256("TeeDisputeGame"),
    keccak256("1"),
    chainId,
    address(TeeProofVerifier)
))
```

**Struct hash：**

```
structHash = keccak256(abi.encode(
    keccak256("BatchProof(bytes32 startBlockHash,bytes32 startStateHash,bytes32 endBlockHash,bytes32 endStateHash,uint256 l2Block)"),
    startBlockHash,
    startStateHash,
    endBlockHash,
    endStateHash,
    l2Block
))
```

**最终 digest：**

```
digest = keccak256(abi.encodePacked("\x19\x01", domainSeparator, structHash))
```

**签名格式：** `abi.encodePacked(r, s, v)` = 32 + 32 + 1 = **65 字节**

可以通过调用 `game.domainSeparator()` 读取链上的 domain separator，与链下计算结果进行比对验证：

```bash
cast call <GAME_ADDRESS> 'domainSeparator()(bytes32)' --rpc-url http://localhost:8545
```

### 多 Batch 链式证明

多个 batch 覆盖不同的子范围，适用于不同的 TEE executor 处理不同的 L2 区块范围：

```
batch[0]: anchor      -> mid1       (l2Block = 50)
batch[1]: mid1        -> mid2       (l2Block = 80)
batch[2]: mid2        -> endState   (l2Block = 100)
```

规则：
- `batch[i].startBlockHash == batch[i-1].endBlockHash` 且 `batch[i].startStateHash == batch[i-1].endStateHash`
- `batch[i].l2Block > batch[i-1].l2Block`
- 每个 batch 可以由**不同的**已注册 enclave 签名
- `batch[0].start` 必须匹配 anchor state
- `batch[last].end` 必须匹配 `rootClaim`，`batch[last].l2Block` 必须等于 `l2SequenceNumber`

> 注意：当前 `TeeProveE2E.s.sol` 仅支持单 batch。如需测试多 batch，可参考 `test/dispute/tee/TeeDisputeGameIntegration.t.sol` 中的多 batch 测试用例自行扩展。

---

## 单步 cast 调用参考

如果需要脱离 E2E 脚本，单独调用各步骤（例如只测 prove 对接），可参考以下 cast 命令。

### 查询 enclave 注册状态

```bash
cast call $TEE_PROOF_VERIFIER \
  'isRegistered(address)(bool)' $ENCLAVE_ADDR \
  --rpc-url http://localhost:8545
```

### 查询 anchor state

```bash
cast call $ANCHOR_STATE_REGISTRY \
  'getAnchorRoot()(bytes32,uint256)' \
  --rpc-url http://localhost:8545
```

### 查询 game 状态

```bash
# game status: 0=IN_PROGRESS, 1=CHALLENGER_WINS, 2=DEFENDER_WINS
cast call <GAME_ADDRESS> 'status()(uint8)' --rpc-url http://localhost:8545

# domain separator（用于验证链下 EIP-712 计算是否正确）
cast call <GAME_ADDRESS> 'domainSeparator()(bytes32)' --rpc-url http://localhost:8545

# l2SequenceNumber
cast call <GAME_ADDRESS> 'l2SequenceNumber()(uint256)' --rpc-url http://localhost:8545

# rootClaim
cast call <GAME_ADDRESS> 'rootClaim()(bytes32)' --rpc-url http://localhost:8545

# proposer
cast call <GAME_ADDRESS> 'proposer()(address)' --rpc-url http://localhost:8545
```

### 领取 bond

```bash
# 等 resolve 后至少 1 秒
cast send <GAME_ADDRESS> 'claimCredit(address)' <RECIPIENT> \
  --private-key <KEY> \
  --rpc-url http://localhost:8545
```

---

## 数据结构参考

### AttestationData（注册用）

```solidity
struct AttestationData {
    uint64  timestampMs;    // Unix 时间戳（毫秒）
    bytes32 pcrHash;        // PCR hash（mock 时可填任意值）
    bytes   publicKey;      // 65 字节：0x04 + x(32) + y(32)
    bytes   userData;       // 附加数据（可为空）
}
```

### Game ExtraData（创建 game 用）

```
extraData = abi.encodePacked(
    uint256 l2SequenceNumber,   // L2 区块号
    uint32  parentIndex,        // 无父 game = 0xFFFFFFFF
    bytes32 endBlockHash,       // 终态 block hash
    bytes32 endStateHash        // 终态 state hash
)
```

### Root Claim

```
rootClaim = keccak256(abi.encode(endBlockHash, endStateHash))
```

### Game 生命周期

```
               create（proposer 存入 DEFENDER_BOND）
                  |
                  v
  +---------- IN_PROGRESS ----------+
  |                                 |
  |  challenge（可选）               |  prove（可选，EIP-712 签名的 batch）
  |  challenger 存入                |  proposer 提交
  |  CHALLENGER_BOND                |  enclave 签名的 proof
  |                                 |
  +--------+----------------+------+
           |                |
           v                v
   deadline 过期        proof 已提交
           |                |
           v                v
       resolve()        resolve()
           |                |
    +------+------+    DEFENDER_WINS
    |             |    （proposer 获得全部 bond）
    v             v
 无 proof      已 prove
    |             |
    v             v
CHALLENGER    DEFENDER
  _WINS        _WINS
（challenger   （proposer
 获得全部       获得全部
  bond）        bond）
```

---

## 常见问题排查

### `register()` 报 `InvalidProof` 错误

**Mock 模式**：确认部署的是 `MockRiscZeroVerifier` 并传给了 `TeeProofVerifier` 构造函数。mock 的 `shouldRevert` 默认为 `false`。

**Fork 模式**：说明 seal 或 attestation data 与链上验证不匹配。检查：
1. `RISC_ZERO_IMAGE_ID` 是否与生成 seal 时使用的 guest program 一致
2. `NITRO_ROOT_KEY` 是否与 attestation 中的 root key 一致（96 字节，P384）
3. `ATT_*` 字段是否与 journal 中的值完全对应（参考 [Journal 字段解析](#journal-字段解析)）
4. `SEAL` 是否完整、未被截断

### `verifyBatch()` 报 `EnclaveNotRegistered` 错误

1. 确认 `register()` 已成功执行：`cast call $TEE_PROOF_VERIFIER 'isRegistered(address)(bool)' $ENCLAVE_ADDR`
2. 确认签名用的私钥与注册时的公钥是同一对
3. 确认没有调用过 `revokeAll()`（会使所有注册失效）

### `prove()` 报 `InvalidSignature` 错误

说明 ecrecover 恢复出的地址与预期不一致，检查以下几点：

1. EIP-712 domain 中的 **`verifyingContract`** 必须是 `TeeProofVerifier` 地址（不是 game 地址）
2. **`chainId`** 必须匹配当前链（mock 模式 = 31337，fork 主网 = 1）
3. **签名格式**必须是 `r(32) + s(32) + v(1)` = 65 字节，用 `abi.encodePacked(r, s, v)` 打包
4. 读取 `game.domainSeparator()` 与你的链下计算结果对比

### `prove()` 报 `StartHashMismatch` 错误

`batch[0].startBlockHash/startStateHash` 的组合 hash 必须等于 anchor state：

```
keccak256(abi.encode(startBlockHash, startStateHash)) == startingOutputRoot.root
```

对于首个 game（无父 game），anchor 来自 `AnchorStateRegistry`，可查询：

```bash
cast call $ANCHOR_STATE_REGISTRY 'getAnchorRoot()(bytes32,uint256)' --rpc-url http://localhost:8545
```

### `prove()` 报 `FinalHashMismatch` 或 `FinalBlockMismatch` 错误

- 最后一个 batch 的 `endBlockHash/endStateHash` 必须满足：`keccak256(abi.encode(endBlockHash, endStateHash)) == rootClaim`
- 最后一个 batch 的 `l2Block` 必须等于 `game.l2SequenceNumber()`

### `prove()` 报 `BatchChainBreak(i)` 错误

`batch[i].startBlockHash != batch[i-1].endBlockHash` 或 `startStateHash != endStateHash`。每个 batch 必须从上一个 batch 的终态开始。

### `prove()` 报 `BadAuth` 错误

`prove()` 只能由 proposer 调用（创建 game 时的 `tx.origin`）。

### `claimCredit()` 报 `GameNotFinalized` 错误

game 必须已 resolve 且 finality delay 已过：`resolvedAt + finalityDelay < block.timestamp`。mock 环境下 finality delay 为 0，但仍需等待至少 1 秒。在 forge script 中所有交易在同一个区块执行，所以需要单独用 `cast send` 调用。

### 如何获取 enclave 的未压缩公钥？

**Foundry (Solidity 中)：**
```solidity
Vm.Wallet memory wallet = vm.createWallet(privateKey, "label");
bytes memory pubKey = abi.encodePacked(
    bytes1(0x04),
    bytes32(wallet.publicKeyX),
    bytes32(wallet.publicKeyY)
);
```

**cast 命令行：**
```bash
# 获取地址
cast wallet address $ENCLAVE_KEY
```

> 注意：`cast` 目前不直接输出未压缩公钥。E2E 脚本 (`TeeProveE2E.s.sol`) 内部通过 `vm.createWallet()` 自动处理了公钥的构造和注册。
