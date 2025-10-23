set -e
set -x
source .env
source tools.sh
source utils.sh
#
## fund the oracle
#cast send -f $DEPLOYER_ADDRESS --private-key $DEPLOYER_PRIVATE_KEY --value 1ether $ORACLE_ADDRESS --rpc-url $L2_SEQ_URL
#
#cd $PWD_DIR
#
#sed_inplace 's/http:\/\/xlayer-rpc:8545/http:\/\/op-geth-seq:8545/' config/agglayer-config.toml
#sed_inplace 's/http:\/\/xlayer-rpc:8545/http:\/\/op-geth-seq:8545/' config/aggkit.toml
#sed_inplace '/\[BridgeL2Sync\]/a\
#InitialBlockNum = '$FORK_BLOCK'
#' config/aggkit.toml
#sed_inplace 's/http:\/\/xlayer-rpc:8545/http:\/\/op-geth-seq:8545/' config/test.bridge.config.toml
#sed_inplace 's/RequireSovereignChainSmcs = \[false\]/RequireSovereignChainSmcs = \[true\]/' config/test.bridge.config.toml
#sed_inplace '/\[NetworkConfig\]/a\
#L2GenBlockNumber = '$FORK_BLOCK'
#' config/test.bridge.config.toml
#
#while true; do
#  sleep 10
#  L2_BLOCK_HEIGHT=$(cast block-number --rpc-url $L2_SEQ_URL)
#  if [ "$L2_BLOCK_HEIGHT" -ge "$FORK_BLOCK" ]; then
#    echo "L2 block height $L2_BLOCK_HEIGHT has reached fork block $FORK_BLOCK"
#    sleep 10
#    break
#  fi
#  echo "Waiting for L2 block height to reach $FORK_BLOCK (current: $L2_BLOCK_HEIGHT)"
#done
#
#${DOCKER_COMPOSE_CMD} up -d xlayer-oracle
#sleep 5
#${DOCKER_COMPOSE_CMD} up -d xlayer-bridge-service
#sleep 10
#docker rm xlayer-bridge-ui
#${DOCKER_COMPOSE_CMD} up -d xlayer-bridge-ui
${DOCKER_COMPOSE_CMD} up -d xlayer-agg-sender
