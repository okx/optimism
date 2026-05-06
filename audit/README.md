# EIP-8130 Native AA Port — 审计文档目录

本目录归档 EIP-8130 (Native Account Abstraction) 从 https://github.com/base/base 移植到本 fork 过程中的全部审计材料。

## 目录结构

```
audit/
├── SECURITY-FINAL-VERDICT.md     ← 上线决策最终结论 (P0/P1/P2 修复清单)
├── README.md                     ← 本文件
│
├── security-review/              ← 6 路并行安全审计 (2026-05-06)
│   ├── security-review-signatures.md       签名 / 加密路径
│   ├── security-review-authorization.md    授权 / 所有权
│   ├── security-review-nonce-replay.md     nonce / replay
│   ├── security-review-gas-dos.md          gas / DoS / sponsor
│   ├── security-review-revm.md             revm 状态机
│   └── security-review-codex.md            Codex 独立二评
│
├── phase-b/                      ← Phase B 符号级移植对齐 (2026-05-05)
│   ├── phase-b-summary.md                  综合
│   ├── phase-b-consensus-types.md          consensus types
│   ├── phase-b-evm-hardforks-rpc.md        EVM / hardforks / RPC
│   ├── phase-b-revm.md                     revm extensions
│   ├── phase-b-txpool-receipts.md          txpool / receipts
│   └── tempo-vs-ours-2026-05-06.md         Tempo 实现对比
│
└── phase-a/                      ← Phase A 文件级清单 (2026-05-05)
    ├── inventory.md                        文件目录映射
    ├── integration-matrix.md               集成矩阵
    └── eip8130-port-audit-4c80fe5b17.md    早期 commit 审计
```

## 审计方法说明

为了高效定位 port 过程中的退化, 整个审计被拆成 Phase A → Phase B → 安全审计 三个阶段, 每阶段定位的"漏网类型"不同, 后一阶段建立在前一阶段的结论之上。

### Phase A — 文件级清点 (Structural Audit)

**问题**: 我们 port 时**漏文件了吗**?
**方法**: 把 ours 和 base 的目录树并排列出, 按"层"分类 (consensus types / EVM 工厂 / revm / txpool / RPC / hardforks / payload / engine / genesis 等), 数文件、对文件名、标记缺失或多余的整文件单元。
**典型发现**: "base 在 `crates/consensus/upgrades/` 有 2 个文件, 我们的 `alloy-op-hardforks/src/` 只有 1 个 → 风险点"。
**不能发现**: 文件存在但内部少了几行、函数签名一样但实现退化、跨文件 wiring 没接上。
**产出**: `phase-a/inventory.md` (文件目录映射), `phase-a/integration-matrix.md` (集成矩阵)

### Phase B — 符号级对比 (Semantic Audit)

**问题**: 文件都在, 但**内部代码有偏差吗**?
**方法**: 在 Phase A 已对齐的文件对上, 用 4 路并行 subagent 做 symbol-level diff: 函数签名、常量值、控制流、wiring (谁调谁)、错误处理、注释里的不变量。每路 agent 负责一个 layer。
**典型发现**:
- BUG-008: TX_CONTEXT/NONCE_MANAGER 的地址常量都对, 但**没接到 `PrecompilesMap`** (跨文件 wiring 漏了)
- BUG-CAND-A: `system_call_one_with_caller` 函数名和签名都对, 但 base 的实现里多了 2 行 `load_account_with_code_mut(caller)?`
- BUG-CAND-B: `OpSpecId::INTEROP` 与 `KARST` / `NATIVE_AA` 的 enum 顺序与 `into_eth_spec()` 映射不一致
**不能发现**: 协议层面的设计漏洞 (即使 ours 和 base 一模一样, 上游本身就有的安全问题), 跨语言 (Go op-node ↔ Rust op-reth) 协同问题, 真实攻击者视角的 exploit chain。
**产出**: `phase-b/phase-b-summary.md` 等 5 份 + Tempo 对比 `tempo-vs-ours-*.md`

### 安全审计 (Security Audit)

**问题**: 协议本身**有可被利用的 bug 吗**? 真实攻击者能不能偷资产?
**方法**: 6 路并行 (5 Claude rust-reviewer 专项 + 1 Codex 独立二评), 每路一个 attack surface (签名 / 授权 / nonce / gas / revm / 跨文件 exploit chain), 用 Trail-of-Bits / Spearbit 标准, 强制要求 file:line + 攻击者模型 + concrete exploit + fix。
**典型发现**:
- CRIT-01 WebAuthn 缺 RP ID/origin 绑定 → 钓鱼网站点一次 Face ID 就能 drain 账户
- H-CODEX-01 op-node Go derivation 不拒 pre-fork `0x7B` AA tx → EL/CL 共识分裂
- H-AUTH-02 双 `AtomicBool` 跨 crate 分裂 (port-only 退化, 我们引入的)
**产出**: `security-review/` 6 份 + 顶层 `SECURITY-FINAL-VERDICT.md`

## 阅读顺序建议

- **赶时间**: 只读 `SECURITY-FINAL-VERDICT.md` (P0/P1/P2 清单)
- **修复 P0**: 看 `security-review/` 中对应分项报告 (file:line + 攻击者模型 + 修复)
- **理解上下文**: Phase A → Phase B → 安全审计 顺序读, 体会从"文件清点"到"符号比对"到"协议级 exploit"的递进
- **回溯历史**: `phase-a/eip8130-port-audit-4c80fe5b17.md` 是最早的初始审计

## 各阶段产出汇总

| 阶段 | 时间 | 方法 | 关注层次 | 输出 |
|---|---|---|---|---|
| Phase A | 2026-05-05 | 文件级 ours vs base 清点 | 是否漏文件 | 77 vs 96 文件映射 |
| Phase B | 2026-05-05 | 4 路并行符号级对比 + Tempo 5 路对比 | 是否漏代码 | BUG-001..008 + 9 BUG-CAND |
| 安全审计 | 2026-05-06 | 6 路并行 (5 Claude + 1 Codex) | 是否有可利用漏洞 | 3 CRIT + 12 HIGH + 9 MED + LOW |

## 当前状态

🔴 **RED / NO-GO** — 见 `SECURITY-FINAL-VERDICT.md`

P0 阻塞项 5 个, 必须全部修复后方可考虑上生产。
