#!/bin/bash

source .env

# L2 ProxyAdmin is a predeploy at a fixed address
L2_PROXY_ADMIN="0x4200000000000000000000000000000000000018"

# L1 ProxyAdmin Owner Address
L1_PROXY_ADMIN_OWNER=$(cast call $PROXY_ADMIN "owner()" --rpc-url $L1_RPC_URL)
echo "L1 ProxyAdmin Owner Address: $L1_PROXY_ADMIN_OWNER"

# Compute the aliased address using cast
# The aliased address is what will own the L2 ProxyAdmin
compute_aliased_address() {
    local l1_addr=$1
    # Convert to decimal, add offset, convert back to hex
    python3 -c "print(hex(int('$l1_addr', 16) + 0x1111000000000000000000000000000000001111))"
}

ALIASED_L2_OWNER=$(compute_aliased_address "$L1_PROXY_ADMIN_OWNER")
echo "Aliased L2 Owner: $ALIASED_L2_OWNER"

echo ""
echo "=========================================="
echo "L2 ProxyAdmin Ownership Transfer"
echo "=========================================="

# 1. Check current L2 ProxyAdmin owner
echo ""
echo "1. Current L2 ProxyAdmin owner:"
cast call $L2_PROXY_ADMIN "owner()" --rpc-url $L2_RPC_URL

# 2. Verify current owner can sign (if using Transactor pattern on L1)
if [ ! -z "$L1_TRANSACTOR" ]; then
    echo ""
    echo "2. Current L1 Transactor owner:"
    cast call $L1_TRANSACTOR "owner()" --rpc-url $L1_RPC_URL
fi

# 3. Generate the transferOwnership calldata
TRANSFER_OWNERSHIP_CALLDATA=$(cast calldata "transferOwnership(address)" $ALIASED_L2_OWNER)
echo ""
echo "3. transferOwnership calldata: $TRANSFER_OWNERSHIP_CALLDATA"

# 4. Send the cross-domain message from L1 to L2
# This requires going through the L1CrossDomainMessenger
echo ""
echo "4. Sending cross-domain message to transfer L2 ProxyAdmin ownership..."
echo ""
echo "⚠️  IMPORTANT: This must be sent from the CURRENT L1 owner (likely a Safe)"
echo "⚠️  The message will be relayed to L2 via the L1CrossDomainMessenger"
echo ""

# Get L1CrossDomainMessengerProxy address from state.json or env
L1_CROSS_DOMAIN_MESSENGER=${L1_CROSS_DOMAIN_MESSENGER_PROXY:-$(jq -r '.opChainDeployments[0].L1CrossDomainMessengerProxy // .opChainDeployments.L1CrossDomainMessengerProxy // empty' ./config-op/state.json 2>/dev/null)}

if [ -z "$L1_CROSS_DOMAIN_MESSENGER" ]; then
    echo "❌ L1_CROSS_DOMAIN_MESSENGER not found. Please set it in .env"
    exit 1
fi

echo "L1CrossDomainMessenger: $L1_CROSS_DOMAIN_MESSENGER"

# Generate calldata for L1CrossDomainMessenger.sendMessage
# function sendMessage(address _target, bytes calldata _message, uint32 _minGasLimit)
L1XDM_CALLDATA=$(cast calldata "sendMessage(address,bytes,uint32)" \
    $L2_PROXY_ADMIN \
    $TRANSFER_OWNERSHIP_CALLDATA \
    100000)

echo ""
echo "L1CrossDomainMessenger.sendMessage calldata:"
echo "$L1XDM_CALLDATA"

# If using Transactor pattern, we need to wrap this in a Transactor.CALL
if [ ! -z "$L1_TRANSACTOR" ]; then
    echo ""
    echo "5. Calling L1CrossDomainMessenger through Transactor..."

    # Generate Transactor.CALL calldata
    cast send $L1_TRANSACTOR "CALL(address,bytes,uint256)" \
        $L1_CROSS_DOMAIN_MESSENGER \
        $L1XDM_CALLDATA \
        0 \
        --rpc-url $L1_RPC_URL \
        --private-key $DEPLOYER_PRIVATE_KEY
else
    echo ""
    echo "5. Direct call to L1CrossDomainMessenger (from Safe or EOA)..."
    echo ""
    echo "Execute this transaction from your L1 Safe:"
    echo "  to: $L1_CROSS_DOMAIN_MESSENGER"
    echo "  data: $L1XDM_CALLDATA"
    echo "  value: 0"
fi

echo ""
echo "=========================================="
echo "⏳ Waiting for cross-domain message relay..."
echo "=========================================="
echo ""
echo "The message needs to be:"
echo "  1. Included in an L1 block"
echo "  2. Picked up by the op-node"
echo "  3. Included in an L2 block (as a deposit transaction)"
echo ""
echo "This typically takes 1-2 minutes."
echo ""
echo "After the relay, verify with:"
echo "  cast call $L2_PROXY_ADMIN \"owner()\" --rpc-url $L2_RPC_URL"
echo ""
echo "Expected new owner: $ALIASED_L2_OWNER"

