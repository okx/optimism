#!/bin/bash
# scripts/logs.sh

set -e

# 检查参数
if [ $# -eq 0 ]; then
    echo "📋 查看所有服务日志"
    echo "使用方法: $0 [service_name] [lines]"
    echo ""
    echo "可用服务:"
    echo "  op-geth  - op-geth执行层日志"
    echo "  op-node  - op-node共识层日志"
    echo "  all      - 所有服务日志"
    echo ""
    echo "示例:"
    echo "  $0 op-geth 100  # 查看op-geth最近100行日志"
    echo "  $0 all          # 查看所有服务日志"
    exit 1
fi

SERVICE=${1:-all}
LINES=${2:-50}

echo "📋 查看 $SERVICE 服务日志 (最近 $LINES 行)"
echo "================================"

if [ "$SERVICE" = "all" ]; then
    docker-compose logs --tail=$LINES --follow
else
    docker-compose logs --tail=$LINES --follow $SERVICE
fi
