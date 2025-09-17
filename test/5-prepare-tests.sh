#!/bin/bash

set -e

source .env


echo "=== Preparing test; Funding L1 Admin Address==="
L1_ADMIN_ADDRESS="0x8f8E2d6cF621f30e9a11309D6A56A876281Fd534"
L1_ADMIN_PRIVATE_KEY="0x815405dddb0e2a99b12af775fd2929e526704e1d1aea6a0b4e74dc33e2f7fcd2"
cast send --rpc-url $L1_RPC_URL $L1_ADMIN_ADDRESS --private-key $RICH_L1_PRIVATE_KEY --value 1000ether

echo "=== Bridging ETH from L1 to L2 ==="

# Bridge contract addresses
OPTIMISM_PORTAL=$(cast call --rpc-url $L1_RPC_URL $SYSTEM_CONFIG_PROXY_ADDRESS 'optimismPortal()(address)')
RECIPIENT="0x14dC79964da2C08b23698B3D3cc7Ca32193d9955"  # Default Rich Address
PRIVATE_KEY="0x4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356" # Default Rich Private Key

echo "OPTIMISM PORTAL Address: $OPTIMISM_PORTAL"
echo "Recipient: $RECIPIENT" 
cast balance $RECIPIENT --rpc-url http://localhost:8123
echo "Bridging 1 ETH from L1 to L2..."

# Bridge 1 ETH to L2
cast send $OPTIMISM_PORTAL \
  --rpc-url http://localhost:8545 \
  --private-key $PRIVATE_KEY \
  --value 100ether

cast send $OPTIMISM_PORTAL \
  --rpc-url http://localhost:8545 \
  --private-key $L1_ADMIN_PRIVATE_KEY \
  --value 100ether



echo -e "\nWaiting for bridging to complete..."

echo "Checking L2 balance for $RECIPIENT:"
BALANCE=$(cast balance $RECIPIENT --rpc-url http://localhost:8123)
ADMIN_BALANCE=$(cast balance $L1_ADMIN_ADDRESS --rpc-url http://localhost:8123)

while [ $BALANCE == 0 ] || [ $ADMIN_BALANCE == 0 ]; do
    echo "L2 account not funded or L1 admin account not funded"
    sleep 5
    BALANCE=$(cast balance $RECIPIENT --rpc-url http://localhost:8123)
    echo "Balance after additional wait: $(cast --to-unit $BALANCE ether) ETH"
    ADMIN_BALANCE=$(cast balance $L1_ADMIN_ADDRESS --rpc-url http://localhost:8123)
    echo "Admin balance after additional wait: $(cast --to-unit $ADMIN_BALANCE ether) ETH"
done

echo "Bridging complete"