#!/bin/bash

# 详细追踪 Deposit 交易流程
L1_TX_HASH="0xf85538540478354aae0287263b291d72182666ce262f75a87bba238463f647ac"
L1_BLOCK="924"
L1_BLOCK_HASH="0xa02fe9078317e8dcc7c4a8fa9893f17ecb498b02655a92a58add4c6002743244"

source .env

echo "═══════════════════════════════════════════════════════════════════════════"
echo "                    Deposit 交易完整流程日志追踪"
echo "═══════════════════════════════════════════════════════════════════════════"
echo ""
echo "交易信息:"
echo "  L1 TX Hash: $L1_TX_HASH"
echo "  L1 Block:   #$L1_BLOCK"
echo "  L1 Hash:    $L1_BLOCK_HASH"
echo ""

echo "═══════════════════════════════════════════════════════════════════════════"
echo "阶段 1: L1 Deposit 交易提交与确认"
echo "═══════════════════════════════════════════════════════════════════════════"
echo ""

echo "【1.1】用户提交 Deposit 交易到 L1"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix l1-geth 2>&1 | grep "0xf85538540478" | head -3
echo ""

echo "【1.2】L1 Geth 处理交易"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix l1-geth 2>&1 | grep -A2 "block.*924" | grep -E "(Updated payload|Imported new)" | head -2
echo ""

echo "【1.3】L1 Beacon Chain 封装区块"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix l1-beacon-chain 2>&1 | grep "slot=935" | grep -E "(building|Finished|Synced)" | head -3
echo ""

echo "═══════════════════════════════════════════════════════════════════════════"
echo "阶段 2: Sequencer 检测 L1 区块并准备处理 Deposit"
echo "═══════════════════════════════════════════════════════════════════════════"
echo ""

echo "【2.1】Sequencer 推进 L1 Origin"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix op-seq 2>&1 | grep -E "l1_origin.*:924" | grep "Built new L2 block" | head -3
echo ""

echo "【2.2】查找包含 Deposit 的 L2 区块"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix op-seq 2>&1 | grep -E "deposits=[1-9]|deposit" | tail -5
echo ""

echo "═══════════════════════════════════════════════════════════════════════════"
echo "阶段 3: Sequencer 构建并封装 L2 区块"
echo "═══════════════════════════════════════════════════════════════════════════"
echo ""

echo "【3.1】Sequencer 构建 L2 区块"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix op-seq 2>&1 | grep "Started sequencing new block" | tail -3
echo ""

echo "【3.2】op-geth 执行区块"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix op-geth-seq 2>&1 | grep "Starting work on payload" | tail -2
echo ""

echo "【3.3】区块封装完成"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix op-seq 2>&1 | grep "Sequencer sealed block" | tail -3
echo ""

echo "═══════════════════════════════════════════════════════════════════════════"
echo "阶段 4: Conductor 协调与 P2P 广播"
echo "═══════════════════════════════════════════════════════════════════════════"
echo ""

echo "【4.1】Conductor 提交 Unsafe Payload"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix op-conductor 2>&1 | grep "committing unsafe payload" | tail -3
echo ""

echo "【4.2】P2P 广播到 Replicas"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix op-seq 2>&1 | grep "Publishing signed execution payload" | tail -3
echo ""

echo "【4.3】Replicas 接收并验证"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix op-rpc 2>&1 | grep "Received signed execution payload" | tail -3
echo ""

echo "═══════════════════════════════════════════════════════════════════════════"
echo "阶段 5: Batcher 收集 L2 区块数据"
echo "═══════════════════════════════════════════════════════════════════════════"
echo ""

echo "【5.1】Batcher 加载 L2 区块"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix op-batcher 2>&1 | grep "Added L2 block to local state" | tail -5
echo ""

echo "【5.2】创建并压缩 Channel"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix op-batcher 2>&1 | grep -E "Created channel|Added blocks to channel" | tail -5
echo ""

echo "═══════════════════════════════════════════════════════════════════════════"
echo "阶段 6: Batcher 提交 L2 数据到 L1"
echo "═══════════════════════════════════════════════════════════════════════════"
echo ""

echo "【6.1】发布 Blob 交易到 L1"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix op-batcher 2>&1 | grep "Publishing transaction" | tail -5
echo ""

echo "【6.2】等待 L1 确认"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix op-batcher 2>&1 | grep "Transaction confirmed" | tail -5
echo ""

echo "【6.3】Channel 完全提交"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix op-batcher 2>&1 | grep "Channel is fully submitted" | tail -3
echo ""

echo "═══════════════════════════════════════════════════════════════════════════"
echo "阶段 7: Replicas 从 L1 同步并派生 L2 状态"
echo "═══════════════════════════════════════════════════════════════════════════"
echo ""

echo "【7.1】检测 L1 更新（可能重组）"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix op-rpc 2>&1 | grep "L1 head signal.*924" | head -2
echo ""

echo "【7.2】推进 Batch Queue Origin 到 L1 #924"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix op-rpc 2>&1 | grep "Advancing bq origin.*:924" | head -3
echo ""

echo "【7.3】解码 Batch 数据"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix op-rpc 2>&1 | grep "decoded singular batch.*origin.*:9" | tail -10
echo ""

echo "【7.4】生成 Payload Attributes"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix op-rpc 2>&1 | grep "generated attributes in payload queue" | tail -5
echo ""

echo "【7.5】更新 Safe Head"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix op-rpc 2>&1 | grep "Record safe head" | tail -10
echo ""

echo "═══════════════════════════════════════════════════════════════════════════"
echo "阶段 8: 最终状态确认"
echo "═══════════════════════════════════════════════════════════════════════════"
echo ""

echo "【8.1】Forkchoice 更新"
echo "──────────────────────────────────────────────────────────────────────────"
docker compose logs --no-log-prefix op-rpc 2>&1 | grep "Forkchoice update" | tail -5
echo ""

echo "【8.2】当前链状态"
echo "──────────────────────────────────────────────────────────────────────────"
echo "L1 区块高度: $(cast block-number --rpc-url $L1_RPC_URL)"
echo "L2 区块高度: $(cast block-number --rpc-url $L2_RPC_URL)"
echo ""

echo "【8.3】Recipient 余额验证"
echo "──────────────────────────────────────────────────────────────────────────"
RECIPIENT=0x70997970C51812dc3A010C7d01b50e0d17dc79C8
echo "Recipient: $RECIPIENT"
echo "余额 (L2): $(cast balance $RECIPIENT --rpc-url $L2_RPC_URL --ether) ETH"
echo ""

echo "═══════════════════════════════════════════════════════════════════════════"
echo "                              追踪完成"
echo "═══════════════════════════════════════════════════════════════════════════"
