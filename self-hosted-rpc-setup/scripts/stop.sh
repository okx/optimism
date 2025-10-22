#!/bin/bash
# scripts/stop.sh

set -e

echo "🛑 停止 X Layer RPC节点..."

# 停止Docker服务
docker compose down

echo "✅ X Layer RPC节点已停止"
