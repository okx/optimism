# EIP-8130 Native AA — 上线前安全审计最终结论

- **日期**: 2026-05-06
- **审计范围**: 我们的 optimism fork (ours) vs https://github.com/base/base/tree/a33ab4d09 (上游) vs https://github.com/tempoxyz/tempo/tree/7b619478e0 (设计参考)
- **审计方法**: 6 路并行 (5 Claude rust-reviewer 专项 + 1 Codex 独立二评), 7271 LoC consensus + 6667 LoC revm + Go derivation
- **总裁定**: 🔴 **RED / NO-GO**, 严禁上生产环境承载真实资产

---

## TL;DR

发现 **3 个 CRITICAL、12 个 HIGH、9 个 MEDIUM、6 个 LOW** 必须修复或显式记录的问题。

CRITICAL 三连击都集中在**签名授权边界**：
1. **WebAuthn 钓鱼**: 没有 RP ID / origin 绑定 → 钓鱼站让用户点一次指纹/Face ID 就能清空账户 (Codex 独立挖到，5 路 Claude 没看出来)
2. **WebAuthn 注册凭证误用**: `type` 字段不校验 → `webauthn.create` 注册时的签名能当作认证 replay (signatures 路)
3. **WebAuthn UP 不强制**: 后台脚本/被劫持 authenticator 可静默签名 (signatures 路)

最严重的**架构级**问题是 op-node Go derivation 与 Rust 执行层的 fork 边界假设不一致 — base 那边修了，我们这边漏了 (Codex 独家发现)。

---

## CRITICAL — 上线前必须修

### CRIT-01 — WebAuthn 缺 RP ID / origin 绑定 → 钓鱼直接清空账户
- **文件**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/native_verifier.rs:261-275, 277-284`
- **来源**: Codex (C-CODEX-01)
- **攻击链**:
  1. 受害者注册了 WebAuthn passkey 作为 AA owner
  2. 攻击者构造 drain tx，算出 `sender_signature_hash(tx)`
  3. 攻击者钓鱼网站调 `navigator.credentials.get()` 用该 hash 当 challenge，**RP ID 是攻击者域名**
  4. 受害者看到熟悉的指纹/Face ID 弹窗点了同意
  5. 协议层不校验 `authenticatorData[0..32] == sha256(rp_id)`, 不校验 `clientDataJSON.origin`, 不校验 `crossOrigin`
  6. 资产被 drain
- **修复**: account configuration 模型中存储 `rp_id_hash` + 允许 origin 集合，verify_webauthn 中显式拒绝不匹配
- **delta**: 与 base 上游共享。Tempo 设计文档实际有提到该需求 (`xlayer-native-aa-requirements.md:63`) 但 base/我们都没实现

### CRIT-02 — WebAuthn `type` 字段不校验 → `webauthn.create` 注册凭证被 replay 为认证
- **文件**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/native_verifier.rs:232-291`
- **来源**: signatures 路 (C-01)
- **攻击模型**: 攻击者拿到受害者注册某站点 passkey 时的 attestation (challenge 恰好等于受害者签名 hash 的场景)，replay 为 AA 认证
- **修复**: 显式 assert `clientDataJSON.type == "webauthn.get"` (WebAuthn spec §7.2 step 11)
- **delta**: 与 base 共享

### CRIT-03 — WebAuthn UP (User Presence) 不强制 → 后台脚本静默签名
- **文件**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/native_verifier.rs`
- **来源**: signatures 路 (C-02)
- **修复**: 校验 `authenticatorData[32] & 0x01 == 1` (UP bit)
- **delta**: 与 base 共享

---

## HIGH — 上线前必须修 (12 项)

### Op-stack 跨语言协同 (Codex 独家挖到)

| ID | 文件 | 问题 | 后果 |
|---|---|---|---|
| **H-CODEX-01** | `op-node/rollup/derive/batches.go:175-188, 384-397` | Go derivation 不拒绝 pre-fork `0x7B` AA tx, base 拒绝 | EL/CL 共识分裂或 safe-head stall |
| **H-CODEX-02** | `op-node/rollup/types.go:595-599` + `attributes.go:187-198` | NativeAA 在 genesis 激活时 `IsNativeAAActivationBlock=false`, 6 个 Solidity 合约从未部署 | AA 半瘫痪、配置写入打到空地址、wallet 假设破裂 |

### 签名 / 授权 / 协议层

| ID | 文件 | 问题 | 来源 |
|---|---|---|---|
| **H-SIG-01** | `eip8130/signature.rs:14-39` | `config_change_digest` 没有 EIP-712 domain prefix → 跨合约部署 replay | sigs |
| **H-SIG-02** | `eip8130/native_verifier.rs:497-544` | Delegate scheme 总返回 `bytes32(delegate_address)` 作为 owner_id, 内部 signer 身份被丢 → scope 升权 | sigs |
| **H-SIG-03** | `eip8130/constants.rs:97` + `validation.rs` | `NONCE_FREE_MAX_EXPIRY_WINDOW` 常量已定义但**无任何调用点强制** → nonce-free tx 可携带任意远期 expiry | sigs |
| **H-SIG-04** | `eip8130/signature.rs` payer_signature_hash | 我们绑了 `resolved_sender`, base 没绑 → 与 base/Solidity payer verifier **wire 不兼容** | sigs |
| **H-AUTH-01** | `eip8130/accessors.rs:63` | `is_owner_authorized()` 用 `verifier != Address::ZERO` → 把 `REVOKED_VERIFIER` 当合法 | auth |
| **H-AUTH-02** ⚠️ port-only | `predeploys.rs:50` + `handler_aa_helpers.rs:84` | 双 `AtomicBool` 跨 crate 分裂 (base 是单 static) | auth |
| **H-AUTH-03** | `handler.rs:489-500` + `eip8130_compat.rs:343-358` | `code_placements` 应用前不查 codehash, 缺 collision guard | auth |
| **H-NR-01** | `op-reth/.../base_pool.rs:560-579` | re-org 后 nonce-free tx **永久从 mempool 消失** (没有 reverted-tx re-add 路径) | nonce |
| **H-NR-02** | `eip8130_pool.rs:797` | `slot_to_seq.entry(...).or_insert(seq_id)` 静默 orphan 后续注册 | nonce |
| **H-GAS-01** ⚠️ 多路命中 | `aa_precompiles.rs:132` + `op-revm/precompiles.rs:117` | `spec == OpSpecId::NATIVE_AA` 用了**精确等值**, 任何后续 fork 一加 AA 全挂 | gas + revm 双确认 |
| **H-GAS-02** | `handler.rs:287-741` | payer auth 仅在 mempool 验签, 入块时只做 SLOAD 重读 → 撤销时序假设未文档化 | gas |

⚠️ 标志含义:
- **port-only** = 我们 port 时引入的退化, 不是 base 上游的问题, **优先**修
- **多路命中** = 多个独立 reviewer 命中同一个 bug, 强信号

---

## MEDIUM (9 项)

| ID | 描述 | 来源 |
|---|---|---|
| M-SIG-01 | Delegate auth blob 尾部多余字节静默接受 (允许 padding 到 MAX_SIGNATURE_SIZE) | sigs |
| M-SIG-02 | `config_change_digest` 不绑 `ACCOUNT_CONFIG_ADDRESS` | sigs |
| M-SIG-03 | K1 low-s 由 k256 lib 隐式保证, 未显式 assert | sigs |
| M-SIG-04 | **P-256 high-s 不归一化** → tx hash malleability ((r,s) 与 (r,n-s) 都有效) | sigs |
| M-SIG-05 | `OwnerScope::UNRESTRICTED=0x00` 注册时不校验未定义 scope bits | sigs |
| M-AUTH-04 | estimation 模式下 config_writes 仍被应用 (但走临时 state, 无持久污染) | auth |
| M-AUTH-05 | DELEGATE 内部 slot 检查实际是 tautological, 注释误导 | auth |
| **M-AUTH-06** | **自毁最后一把钥匙能 brick 账户**, 也可被社工诱导签 "rogue config change" | auth (spec 级) |
| M-NR-03 | nonce-free dedup key 不含 `payer_auth` → sponsor 前置/抢跑 | nonce |
| M-NR-04 ⚠️ 双确认 | `nonce_sequence + 1` u64 wrap (release silently 归零) | nonce + revm |
| M-REVM-02 | `ACCOUNT_CONFIG_DEPLOYED` 进程级 AtomicBool 不 reset → 测试/devnet re-genesis 后写到 EOA | revm |
| M-REVM-03 | `catch_error` 不 clear thread-local TX_CONTEXT | revm |
| M-GAS-01 | nonce-free 环形缓冲区缺 per-sender 入池上限 | gas |
| M-GAS-02 | Delegation 计费缺冷码读 ~2100 gas | gas |
| M-GAS-03 | `eth_estimateGas` calldata 估算偏低最高 ~31600 gas | gas |
| **M-CODEX-01** | NativeAA 激活块缺 derivation 端 NoTxPool 强制, 与 sequencer 端不一致 → first-block MEV | codex |

---

## LOW + INFO (主要是 hygiene, 上线后修也来得及)

略 — 详见各路报告。

---

## 必修清单 (按优先级排序，方便 PM/dev 跟进)

### P0 — 阻塞上线
1. CRIT-01..03: 三个 WebAuthn 漏洞 (RP/origin、type、UP)
2. H-CODEX-01..02: op-node Go derivation 的 fork 边界 (跟上 base 上游修复)
3. H-AUTH-02: 双 AtomicBool 跨 crate 分裂 (port-only 退化, 必须修回单 static)
4. H-GAS-01: `spec == NATIVE_AA` → `is_enabled_in` (双 reviewer 命中, 改一行)
5. H-SIG-04: payer_signature_hash 与 base/Solidity 兼容性 — 立刻和合约方对齐

### P1 — 上线前修
6. H-SIG-01..03: EIP-712 domain、Delegate 内部 owner_id、NONCE_FREE expiry 强制
7. H-AUTH-01: `is_owner_authorized` REVOKED_VERIFIER 检查
8. H-AUTH-03: code_placements collision guard
9. H-NR-01..02: re-org nonce-free 复活、`or_insert` 静默 orphan
10. M-SIG-04: P-256 high-s 归一化
11. M-AUTH-06: 最后一把钥匙保护 (spec committee 决策)
12. M-CODEX-01: NativeAA 激活块 derivation drop 非空 batch

### P2 — 上线后近期跟进
13. 其他 MEDIUM
14. clippy 60+ 错 (CI gate 实际会卡)
15. 39 个 disabled 的 consensus tests (mechanical sed migration ~20 行)

---

## 与 BASE 上游的关系总结

- **大多数 HIGH 是 base 上游也有的问题** — 既要修自己, 也要给 base 上游提 patch (我们的 fork 应当向上反哺)
- **port-only 退化** 只有 1 处 (H-AUTH-02 双 AtomicBool), 其余移植高度保真
- **base 修了我们没修的** 关键点: H-CODEX-01 (Go batch validation drop pre-fork 0x7B) — base 已经有 `test_check_batch_drop_8130_pre_base_v1`, 我们没 port

---

## 与 TEMPO 设计的差距

- Tempo `xlayer-native-aa-requirements.md:63` 显式提到 WebAuthn 需要 RP ID / origin 绑定 → CRIT-01
- Tempo 多次强调 multi-scheme 下游 downgrade 攻击, base/我们目前只靠 sender 自己选 scheme, 没有协议层 minimum-strength 锁
- Tempo 的 sponsor flow 拆 phase 0 (paymaster fee) / phase 1 (user op), 我们的实现实际正确闭合了 cross-sender payer replay (gas 路 I-005 confirmed)
- Tempo 没有给出 cross-client fork activation invariant 规范 → 这正是 H-CODEX-01..02 + M-CODEX-01 的根因

---

## 建议下一步行动

1. **冻结代码合并**, 在 P0 全部修完前不要进 mainnet 部署分支
2. **拉一个 SecOps fix-week sprint**: 每个 P0/P1 都对应一个独立 PR + 测试用例
3. **建立 cross-client 测试矩阵**: op-node Go × op-reth Rust × Solidity contracts × base 上游, 共享同一组 EIP-8130 签名向量与 fork 激活向量
4. **WebAuthn 部分单独做正式安全评审**: 包括 attestation chain、authenticator metadata、recovery flow
5. **审计完成后**: 把所有 fix 反向贡献回 base 上游 (我们这次审计的副产品对 OP Stack 生态都有价值)

---

## 参考报告

详细分项报告位于子目录:

### `audit/security-review/` — 本次安全审计 6 路报告
- `security-review-signatures.md` — 签名/加密 (32K)
- `security-review-authorization.md` — 授权/所有权 (25K)
- `security-review-nonce-replay.md` — nonce/replay (26K)
- `security-review-gas-dos.md` — gas/DoS/sponsor (27K)
- `security-review-revm.md` — revm 状态机 (20K)
- `security-review-codex.md` — Codex 独立二评 (17K)

### `audit/phase-b/` — Phase B 符号级移植对齐 (前置审计)
- `phase-b-summary.md` — Phase B 综合
- `phase-b-consensus-types.md`, `phase-b-evm-hardforks-rpc.md`, `phase-b-revm.md`, `phase-b-txpool-receipts.md`
- `tempo-vs-ours-2026-05-06.md` — Tempo 参考实现对比

### `audit/phase-a/` — Phase A 文件级清单 (最早的 baseline)
- `inventory.md`, `integration-matrix.md`, `eip8130-port-audit-4c80fe5b17.md`
