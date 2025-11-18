# OP Stack 升级指南（sync-1161）

## 📋 升级全流程表

| 阶段 | 组件 | 风险 | 职责 | 关键检查点 | 观察期 | 停机影响 | 致命风险 | 回滚时间 |
|:---:|------|:----:|------|-----------|:----:|---------|---------|:------:|
| **0** | **op-program/prestate** | 🔴🔴🔴 | 争议证明 | 对比 prestate hash 与 L1 合约 | - | 无 | hash 不匹配导致争议失效 | 需 L1 操作 |
| **1** | op-dispute-mon | 🟢 | 监控面板 | 服务状态、日志无错误 | 2h | ✅ 无影响 | 无 | 1min |
| **1** | **RPC 节点** (①op-geth-rpc ②op-rpc) | 🟢 | 只读查询 | 区块同步、RPC 响应正常 | 2h | ⚠️ 查询暂时失败 | 连接失败 | 5min |
| **2** | op-batcher | 🟡 | L1 数据提交 | L1 RPC 连通、余额充足 | 2h | ⚠️ 数据缓存 10-30min | Gas 不足、L1 合约拒绝 | 5min |
| **2** | op-challenger | 🟡🟡 | 争议监控 | prestate hash 正确、监控 games | 2h | ⚠️ 短期无监控 | prestate 配置错误 | 10min |
| **2** | op-proposer | 🟡🟡 | L1 创建 game | game-type 已注册、创建成功 | 4h | ⚠️ 提现延迟 1-2h | game-type 未注册 | 10min |
| **3a** | **Sequencer-3** (①conductor ②op-seq ③op-geth) | 🟡 | 备份节点 | 重新加入 Raft、Engine API 连接 | 1h | ✅ Seq1/2 继续 **(Raft: 3→2)** | 无法加入 Raft | 10min |
| **3b** | **Sequencer-2** (①conductor ②op-seq ③op-geth) | 🔴 | 备份节点 | 重新加入 Raft、区块同步正常 | 1h | ⚠️ **Raft: 2→1 ⚡零容错** | 无法加入 Raft 导致停链 | 10min |
| **3c** | **Sequencer-1** (①conductor ②op-seq ③op-geth) | 🔴🔴🔴 | 主节点 Leader | 触发选举、新 Leader 出块 | 2h | 🚨 **Leader 切换中 💥 可能停链** | 数据库不兼容、无新 Leader | ⚡ **2min** |

---

## 🎯 关键决策流程

```
升级前
  ↓
检查 op-program 是否改动？
  ├─ 否 → ✅ 安全，继续
  └─ 是 → 检查 prestate hash
           ├─ 相同 → ✅ 安全，继续
           └─ 不同 → 🛑 停止！
                    └─ 必须先在 L1 添加新 game type
                       └─ 更新 op-proposer/challenger 配置
                          └─ 测试验证后，继续升级

升级顺序
  ↓
1. 边缘服务（2-4 小时）
   └─ op-dispute-mon → op-geth-rpc
  ↓
2. L1 服务（6-8 小时）
   └─ op-batcher → op-challenger → op-proposer
  ↓
3. Sequencer 集群（4-6 小时）
   └─ Seq3 → Seq2 → Seq1
      每个内部：conductor → op-seq → op-geth
  ↓
最终验证
  └─ 所有服务健康运行 2+ 小时
```

---

## ⚠️ 致命风险速查

| 风险 | 检测方法 | 紧急响应 |
|------|---------|---------|
| **prestate hash 不匹配** | 对比本地 hash 与 L1 合约 | 🛑 停止升级，先处理 L1 |
| **op-geth 数据库不兼容** | 启动日志：`incompatible database` | ⚡ 立即回滚 |
| **Engine API 版本冲突** | op-seq 日志：`engine api error` | ⚡ 立即回滚 |
| **Leader 切换失败** | 区块高度停止增长 | 🚨 2 分钟内紧急回滚 Seq1 |
| **Raft 集群无法达成共识** | 少于 2 个节点在线 | 🚨 立即恢复至少 1 个节点 |

---

## 📊 预计总时间

- **保守升级**：3-4 天（每阶段充分观察）
- **激进升级**：1-2 天（测试网可接受）
- **最小窗口**：12-16 小时（高风险，不推荐生产环境）

---

## 🎓 升级原则

1. ✅ **从边缘到核心**：先升级不影响共识的组件
2. ✅ **从只读到读写**：先升级查询服务，后升级写入服务
3. ✅ **从备份到主节点**：先升级 Follower，最后升级 Leader
4. ✅ **充分观察**：每步观察足够时间，确认稳定再继续
5. ✅ **准备回滚**：随时准备快速回滚，尤其是 Sequencer
6. 🚨 **保持警惕**：升级 Seq2 后零容错，升级 Seq1 时实时监控

---
