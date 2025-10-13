#!/bin/bash
echo "Custom Gas Token Demo: only modify contract, without modifying sequencer code"
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

# Query initial OKB total supply
INIT_TOTAL_SUPPLY=$(cast call "$OKB_TOKEN" "totalSupply()(uint256)" --rpc-url "$L1_RPC_URL")
INIT_TOTAL_SUPPLY_FORMATTED=$((INIT_TOTAL_SUPPLY / 10**18))
echo ""
echo "📊 Initial OKB Total Supply: $INIT_TOTAL_SUPPLY ($INIT_TOTAL_SUPPLY_FORMATTED OKB)"

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

  DEPOSIT_AMOUNT="7999000000000000"

  # L2 recipient address
  L2_RECIPIENT=0x70997970C51812dc3A010C7d01b50e0d17dc79C9

  INIT_BALANCE=$(cast balance $L2_RECIPIENT --rpc-url $L2_RPC_URL)
  echo "  Deposit Amount: $DEPOSIT_AMOUNT"
  echo "  L2 Recipient:   $L2_RECIPIENT"
  echo "  Initial Balance: $INIT_BALANCE"
  echo ""

  # Step 3a: Approve the adapter to spend OKB (no detail output)
  cast send "$OKB_TOKEN" \
    "approve(address,uint256)" \
    "$ADAPTER_ADDRESS" \
    "$DEPOSIT_AMOUNT" \
    --rpc-url "$L1_RPC_URL" \
    --private-key "$DEPLOYER_PRIVATE_KEY"

  # Step 3b: Perform the deposit (no detail output)
  cast send "$ADAPTER_ADDRESS" \
    "deposit(address,uint256)" \
    "$L2_RECIPIENT" \
    "$DEPOSIT_AMOUNT" \
    --rpc-url "$L1_RPC_URL" \
    --private-key "$DEPLOYER_PRIVATE_KEY"

  echo ""
  echo "✅ Test deposit transaction sent!"
  echo ""
  echo "⏳ Waiting for L2 to process the deposit..."
  echo "   Checking balance every 5 seconds..."
  echo ""

  # Expected final balance
  EXPECTED_BALANCE=$((INIT_BALANCE + DEPOSIT_AMOUNT))

  # Timeout after 5 minutes (60 attempts * 5 seconds)
  MAX_ATTEMPTS=60
  ATTEMPT=0

  while [ $ATTEMPT -lt $MAX_ATTEMPTS ]; do
    CURRENT_BALANCE=$(cast balance $L2_RECIPIENT --rpc-url $L2_RPC_URL)

    echo "  [Attempt $((ATTEMPT + 1))/$MAX_ATTEMPTS] Current Balance: $CURRENT_BALANCE (Expected: $EXPECTED_BALANCE)"

    if [ "$CURRENT_BALANCE" = "$EXPECTED_BALANCE" ]; then
      echo ""
      echo "🎉 Deposit processed successfully!"
      echo ""

      # Query OKB total supply after successful deposit
      DEPOSIT_FINAL_TOTAL_SUPPLY=$(cast call "$OKB_TOKEN" "totalSupply()(uint256)" --rpc-url "$L1_RPC_URL")
      DEPOSIT_FINAL_TOTAL_SUPPLY_FORMATTED=$((DEPOSIT_FINAL_TOTAL_SUPPLY / 10**18))
      DEPOSIT_BURNED_AMOUNT=$((INIT_TOTAL_SUPPLY - DEPOSIT_FINAL_TOTAL_SUPPLY))
      DEPOSIT_BURNED_AMOUNT_FORMATTED=$((DEPOSIT_BURNED_AMOUNT / 10**18))

      echo "📊 Final Status:"
      echo "   Initial Balance:  $INIT_BALANCE"
      echo "   Deposit Amount:   $DEPOSIT_AMOUNT"
      echo "   Final Balance:    $CURRENT_BALANCE"
      echo "   L2 Recipient:     $L2_RECIPIENT"
      echo ""
      echo "🔥 OKB Token Supply Status:"
      echo "   Initial Total Supply: $INIT_TOTAL_SUPPLY ($INIT_TOTAL_SUPPLY_FORMATTED OKB)"
      echo "   Final Total Supply:   $DEPOSIT_FINAL_TOTAL_SUPPLY ($DEPOSIT_FINAL_TOTAL_SUPPLY_FORMATTED OKB)"
      if [ "$DEPOSIT_BURNED_AMOUNT" -gt 0 ]; then
        echo "   Tokens Burned:        $DEPOSIT_BURNED_AMOUNT ($DEPOSIT_BURNED_AMOUNT_FORMATTED OKB)"
      else
        echo "   Tokens Burned:        0 (0 OKB)"
      fi
      echo ""
      break
    fi

    ATTEMPT=$((ATTEMPT + 1))

    if [ $ATTEMPT -lt $MAX_ATTEMPTS ]; then
      sleep 5
    fi
  done

  if [ $ATTEMPT -eq $MAX_ATTEMPTS ]; then
    echo ""
    echo "⚠️  WARNING: Deposit not processed within timeout period (5 minutes)"
    echo "   Current Balance:  $CURRENT_BALANCE"
    echo "   Expected Balance: $EXPECTED_BALANCE"
    echo ""
    echo "📚 Troubleshooting:"
    echo "   1. Check if L2 node is running and syncing"
    echo "   2. Check L1 transaction status"
    echo "   3. Monitor TransactionDeposited events on OptimismPortal: $OPTIMISM_PORTAL_PROXY_ADDRESS"
    echo "   4. Manually check balance: cast balance $L2_RECIPIENT --rpc-url $L2_RPC_URL"
  fi
fi
