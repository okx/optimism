# X Layer 第三方RPC节点启动脚本

这个目录包含了启动X Layer第三方RPC节点的完整脚本和配置文件。

## 目录结构

```
third-party-rpc-node/
├── README.md                 # 本文档
├── docker-compose.yml        # Docker Compose配置文件
├── env.example              # 环境变量示例文件
├── config/                   # 配置文件目录
│   ├── rollup.json          # Rollup配置文件
│   ├── jwt.txt              # JWT密钥文件
│   ├── op-geth-config.toml  # op-geth配置文件
│   └── genesis.json         # L1 Genesis配置
├── scripts/                  # 启动脚本目录
│   ├── start.sh             # 主启动脚本
│   ├── stop.sh              # 停止脚本
│   ├── status.sh            # 状态检查脚本
│   └── logs.sh              # 日志查看脚本
└── data/                     # 数据目录（自动创建）
    ├── op-geth/             # op-geth数据
    └── op-node/             # op-node数据
```

## 快速开始

### 1. 配置环境变量

复制并编辑环境变量文件：

```bash
cp env.example .env
```

编辑 `.env` 文件，填入X Layer测试网的相关信息：

```bash
# X Layer测试网配置
L1_RPC_URL=https://your-l1-rpc-endpoint
L1_BEACON_URL=https://your-l1-beacon-endpoint
NETWORK_ID=your-network-id
CHAIN_ID=your-chain-id

# Bootnode配置
OP_NODE_BOOTNODE=enr:-J24QMKrCjD9yO_etOJmxFNAJdJayoNLzx2SKRD4amiEtYHSAgaQIheHntQ8hQLby7ejqwPQzMuT6nys9MBfU0yeVZaGAZnq6jJvgmlkgnY0gmlwhAjStTKHb3BzdGFja4OgDwCJc2VjcDI1NmsxoQPqrp_i_HWK3WX-TP1CkY6JjharIylNuI8NzbyrJ3PnW4N0Y3CCJAeDdWRwgiQH
OP_GETH_BOOTNODE=enode://49d192820108e631fcca90eea2dad846e0de2a451cd8c131b659e7cfc50c6c54e4b99267b2b9889dc4e6df73547039c7fb726d09c8ba31e91cd3b5f497d291f7@8.210.181.50:30303
```

### 2. 准备配置文件

将X Layer测试网的配置文件放入 `config/` 目录：

- `rollup.json` - Rollup配置（需要从X Layer官方获取）
- `genesis.json` - L1 Genesis配置（需要从X Layer官方获取）
- `jwt.txt` - JWT密钥（32字节十六进制）

### 3. 启动节点

```bash
# 启动所有服务
./scripts/start.sh

# 检查状态
./scripts/status.sh

# 查看日志
./scripts/logs.sh
```

### 4. 停止节点

```bash
./scripts/stop.sh
```

## 服务端口

启动后，以下端口将可用：

- **op-geth RPC**: `http://localhost:8545`
- **op-geth WebSocket**: `ws://localhost:8546`
- **op-node RPC**: `http://localhost:9545`
- **op-geth P2P**: `30303` (TCP/UDP)
- **op-node P2P**: `9223` (TCP/UDP)

## 配置说明

### op-geth配置

op-geth作为执行层客户端，主要配置：
- 连接到L1网络
- 启用RPC和WebSocket接口
- 配置P2P网络和bootnode
- 设置数据目录和日志级别

### op-node配置

op-node作为共识层客户端，主要配置：
- 连接到L1 RPC和Beacon API
- 连接到op-geth执行层
- 启用RPC接口
- 配置P2P网络和bootnode
- 设置同步模式

## 需要从X Layer官方获取的信息

要完成配置，您需要从X Layer官方获取以下信息：

1. **L1 RPC端点** - X Layer测试网的L1 RPC地址
2. **L1 Beacon端点** - X Layer测试网的L1 Beacon API地址
3. **Rollup配置** - X Layer测试网的rollup.json文件
4. **网络ID** - X Layer测试网的网络ID
5. **链ID** - X Layer测试网的链ID
6. **Genesis配置** - L1的genesis.json文件

## 故障排除

### 常见问题

1. **连接失败**: 检查L1 RPC和Beacon API地址是否正确
2. **同步问题**: 确认网络ID和链ID配置正确
3. **P2P连接**: 检查防火墙设置，确保P2P端口开放

### 日志查看

```bash
# 查看所有服务日志
./scripts/logs.sh

# 查看特定服务日志
./scripts/logs.sh op-geth 100
./scripts/logs.sh op-node 100
```

### 数据重置

如果需要重新同步：

```bash
# 停止服务
./scripts/stop.sh

# 删除数据目录
rm -rf data/

# 重新启动
./scripts/start.sh
```

## 安全注意事项

1. **JWT密钥**: 确保JWT密钥文件安全，不要泄露
2. **RPC访问**: 生产环境中应该限制RPC访问权限
3. **防火墙**: 根据需要配置防火墙规则
4. **资源监控**: 监控节点资源使用情况

## 支持

如有问题，请参考：
- [OP Stack文档](https://docs.optimism.io/)
- [X Layer文档](https://docs.xlayer.tech/)