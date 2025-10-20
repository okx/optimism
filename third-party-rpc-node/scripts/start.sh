#!/bin/bash
# scripts/start.sh

set -e

echo "🚀 启动 X Layer 第三方RPC节点..."

# 检查环境变量文件
if [ ! -f .env ]; then
    echo "❌ 错误: .env 文件不存在"
    echo "请复制 env.example 到 .env 并填入正确的配置"
    exit 1
fi

# 加载环境变量
source .env

# 检查必要的环境变量
required_vars=("L1_RPC_URL" "L1_BEACON_URL" "OP_NODE_BOOTNODE" "OP_GETH_BOOTNODE")
for var in "${required_vars[@]}"; do
    if [ -z "${!var}" ]; then
        echo "❌ 错误: 环境变量 $var 未设置"
        exit 1
    fi
done

# 创建必要的目录
echo "📁 创建数据目录..."
mkdir -p data/op-geth
mkdir -p data/op-node/p2p
mkdir -p config

# 检查配置文件
echo "🔍 检查配置文件..."
config_files=("../config/rollup.json" "../config/jwt.txt" "../config/op-geth-config.toml" "../config/genesis.json")
for file in "${config_files[@]}"; do
    if [ ! -f "$file" ]; then
        echo "❌ 错误: 配置文件 $file 不存在"
        echo "请将X Layer测试网的配置文件放入 config/ 目录"
        exit 1
    fi
done

# 生成JWT密钥（如果不存在）
if [ ! -s config/jwt.txt ]; then
    echo "🔑 生成JWT密钥..."
    openssl rand -hex 32 > config/jwt.txt
fi

# 启动服务
echo "🐳 启动Docker服务..."
docker-compose up -d

# 等待服务启动
echo "⏳ 等待服务启动..."
sleep 10

# 检查服务状态
echo "🔍 检查服务状态..."
docker-compose ps

echo "✅ X Layer RPC节点启动完成!"
echo ""
echo "📡 服务端点:"
echo "  - op-geth RPC: http://localhost:8545"
echo "  - op-geth WebSocket: ws://localhost:8546"
echo "  - op-node RPC: http://localhost:9545"
echo ""
echo "📊 查看日志: ./scripts/logs.sh"
echo "🛑 停止服务: ./scripts/stop.sh"
echo "📈 检查状态: ./scripts/status.sh"
