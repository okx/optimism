#!/bin/bash

set -e

ROOT_DIR=$(git rev-parse --show-toplevel)
PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

cd $PWD_DIR

source .env

echo "🔧 Setting up Custom Gas Token (CGT) configuration..."
echo ""

# Run the Foundry script to deploy and configure CGT
echo "📝 Step 1: Running Foundry setup script..."
cd $ROOT_DIR/packages/contracts-bedrock
export SYSTEM_CONFIG_PROXY_ADDRESS=$SYSTEM_CONFIG_PROXY_ADDRESS
export OPTIMISM_PORTAL_PROXY_ADDRESS=$OPTIMISM_PORTAL_PROXY_ADDRESS

# Capture forge script output
# forge script scripts/SetupCustomGasToken.s.sol:SetupCustomGasToken \
#   --rpc-url "$L1_RPC_URL" \
#   --private-key "$DEPLOYER_PRIVATE_KEY"

FORGE_OUTPUT=$(forge script scripts/SetupCustomGasToken.s.sol:SetupCustomGasToken \
  --rpc-url "$L1_RPC_URL" \
  --private-key "$DEPLOYER_PRIVATE_KEY" \
  --broadcast 2>&1)

echo "$FORGE_OUTPUT"

# Extract contract addresses from forge output
OKB_TOKEN=$(echo "$FORGE_OUTPUT" | grep "MockOKB deployed at:" | awk '{print $NF}')
ADAPTER_ADDRESS=$(echo "$FORGE_OUTPUT" | grep "DepositedOKBAdapter deployed at:" | awk '{print $NF}')

echo ""
echo "✅ L1 Custom Gas Token setup complete!"
echo ""
echo "📋 Deployed Contract Addresses:"
echo "   OKB Token:          $OKB_TOKEN"
echo "   Adapter:            $ADAPTER_ADDRESS"
echo ""

# Check if L2 is running before verifying L2 configuration
if curl -s -X POST "$L2_RPC_URL" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
  > /dev/null 2>&1; then

  echo "📝 Step 2: Verifying L2 configuration..."
  echo ""

  # Call L1Block predeploy to check configuration
  L1_BLOCK_ADDR="0x4200000000000000000000000000000000000015"

  # Check isCustomGasToken
  IS_CUSTOM_GAS_TOKEN=$(cast call "$L1_BLOCK_ADDR" \
    "isCustomGasToken()(bool)" \
    --rpc-url "$L2_RPC_URL")

  echo "  L1Block.isCustomGasToken(): $IS_CUSTOM_GAS_TOKEN"

  # Check gasPayingTokenName
  TOKEN_NAME=$(cast call "$L1_BLOCK_ADDR" \
    "gasPayingTokenName()(string)" \
    --rpc-url "$L2_RPC_URL")

  echo "  L1Block.gasPayingTokenName(): $TOKEN_NAME"

  # Check gasPayingTokenSymbol
  TOKEN_SYMBOL=$(cast call "$L1_BLOCK_ADDR" \
    "gasPayingTokenSymbol()(string)" \
    --rpc-url "$L2_RPC_URL")

  echo "  L1Block.gasPayingTokenSymbol(): $TOKEN_SYMBOL"

  echo ""
  if [ "$IS_CUSTOM_GAS_TOKEN" = "true" ]; then
    echo "✅ L2 Custom Gas Token configuration verified!"
  else
    echo "⚠️  WARNING: L2 custom gas token not yet active"
    echo "   The L2 chain needs to process the setCustomGasToken transaction"
    echo "   This will happen automatically when the chain processes L1 data"
  fi
else
  echo "⚠️  L2 node is not running yet - skipping L2 verification"
  echo "   Please verify L2 configuration after the L2 node starts"
fi

echo ""
echo "🎉 Custom Gas Token setup script completed!"
echo ""

# Perform test deposit
if [ -n "$OKB_TOKEN" ] && [ -n "$ADAPTER_ADDRESS" ]; then
  echo "📝 Step 3: Performing test deposit..."
  echo ""

  # Amount to deposit: 1 OKB (1e18 wei)
  DEPOSIT_AMOUNT="1000000000000000000"

  # L2 recipient address (anvil dev account 2)
  L2_RECIPIENT=0x70997970C51812dc3A010C7d01b50e0d17dc79C8

  INIT_BALANCE=$(cast balance $L2_RECIPIENT --rpc-url $L2_RPC_URL)
  echo "  Deposit Amount: $DEPOSIT_AMOUNT"
  echo "  L2 Recipient:   $L2_RECIPIENT"
  echo "  Initial Balance: $INIT_BALANCE"
  echo ""

  # Step 3a: Approve the adapter to spend OKB
  echo "  📤 Approving adapter to spend OKB..."
  cast send "$OKB_TOKEN" \
    "approve(address,uint256)" \
    "$ADAPTER_ADDRESS" \
    "$DEPOSIT_AMOUNT" \
    --rpc-url "$L1_RPC_URL" \
    --private-key "$DEPLOYER_PRIVATE_KEY" \

  # Step 3b: Perform the deposit
  echo "  💰 Depositing 1 OKB to L2..."
  cast send "$ADAPTER_ADDRESS" \
    "deposit(address,uint256)" \
    "$L2_RECIPIENT" \
    "$DEPOSIT_AMOUNT" \
    --rpc-url "$L1_RPC_URL" \
    --private-key "$DEPLOYER_PRIVATE_KEY"

  echo ""
  echo "✅ Test deposit successful!"
  echo ""
  echo "📚 Next steps:"
  echo "   1. Wait for L2 to process the deposit (may take a few blocks)"
  echo "   2. Check L2 balance: cast balance $L2_RECIPIENT --rpc-url $L2_RPC_URL"
  echo "   3. Monitor TransactionDeposited events on OptimismPortal"
fi

