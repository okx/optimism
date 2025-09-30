#!/bin/bash
# This demonstrates how to transfer ProxyAdmin ownership to a new contract

source .env
# 1. Prepare a Safe wallet that will be the new owner
# ProxyAdmin owner should be a contract address
SAFE_WALLET_ADDR=0xa0Ee7A142d267C1f36714E4a8F75612F20a79720

# 2. Get current ProxyAdmin owner (should be Transactor contract)
echo "Current ProxyAdmin owner:"
cast call $PROXY_ADMIN "owner()" --rpc-url $L1_RPC_URL

# 3. Verify Transactor contract owner (should be deployer)
echo "Current Transactor owner:"
cast call $TRANSACTOR "owner()" --rpc-url $L1_RPC_URL

# 4. Generate calldata for transferOwnership call
TRANSFER_OWNERSHIP_CALLDATA=$(cast calldata "transferOwnership(address)" $SAFE_WALLET_ADDR)
echo "transferOwnership calldata: $TRANSFER_OWNERSHIP_CALLDATA"

# 5. Call transferOwnership through Transactor.CALL
# Transactor.CALL(address _target, bytes memory _data, uint256 _value)
echo "Transferring ProxyAdmin ownership to Safe wallet via Transactor..."
cast send $TRANSACTOR "CALL(address,bytes,uint256)" \
  $PROXY_ADMIN \
  $TRANSFER_OWNERSHIP_CALLDATA \
  0 \
  --rpc-url $L1_RPC_URL \
  --private-key $DEPLOYER_PRIVATE_KEY

# 6. Verify ProxyAdmin owner is now the Safe wallet
echo "New ProxyAdmin owner:"
cast call $PROXY_ADMIN "owner()" --rpc-url $L1_RPC_URL
