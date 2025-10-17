#!/bin/bash
# scripts/status.sh

set -e

echo "📊 X Layer RPC节点状态检查"
echo "================================"

# 检查Docker服务状态
echo "🐳 Docker服务状态:"
docker-compose ps

echo ""
echo "🔍 服务健康检查:"

# 检查op-geth RPC
echo -n "op-geth RPC: "
if curl -s -f http://localhost:8545 > /dev/null 2>&1; then
    echo "✅ 正常"
else
    echo "❌ 异常"
fi

# 检查op-node RPC
echo -n "op-node RPC: "
if curl -s -f http://localhost:9545 > /dev/null 2>&1; then
    echo "✅ 正常"
else
    echo "❌ 异常"
fi

echo ""
echo "📈 资源使用情况:"
docker stats --no-stream --format "table {{.Container}}\t{{.CPUPerc}}\t{{.MemUsage}}\t{{.NetIO}}\t{{.BlockIO}}"

echo ""
echo "🔗 网络连接:"
echo "P2P端口状态:"
netstat -tuln | grep -E ":(30303|9223)" || echo "P2P端口未监听"
