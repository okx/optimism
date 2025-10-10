# Bridge Upgrade Proof of Concept (POC)

This directory contains scripts and tests to demonstrate the bridge upgrade functionality and revert mechanism in Optimism.

## Overview

The POC demonstrates a scenario where:
1. The deployed bridge contracts have revert functionality enabled (blocking bridge operations)
2. Tests verify that bridge operations are properly blocked
3. Upgrade scripts can be used to deploy new implementations without revert functionality
4. Post-upgrade tests confirm that bridge operations work normally

## Directory Structure

```
test/POC/
├── README.md                    # This file
├── L1UpgradeBridge.s.sol       # L1 bridge upgrade script
├── L2UpgradeBridge.s.sol       # L2 bridge upgrade script
├── CrossChainERC20.t.sol       # L1 bridge revert tests
├── L2CrossChainERC20.t.sol     # L2 bridge revert tests
└── run_bridge_tests.sh         # Test runner script
```

## Prerequisites

1. **Running L1 and L2 networks**
   - L1 RPC: `http://127.0.0.1:8545`
   - L2 RPC: `http://127.0.0.1:8123`

2. **Environment setup**
   - Create a `.env` file in the POC directory
   - Add the required private key: `TRANSACTOR_PRIVATE_KEY=<your_private_key>`

3. **Native tokens for L2 operations**
   - Ensure the deployer account has sufficient native tokens on L2 for upgrade transactions

## Testing Flow

### Step 1: Verify Bridge Revert Functionality

First, test that the deployed bridge contracts properly revert bridge operations:

```bash
# Run the bridge tests to verify revert functionality
./run_bridge_tests.sh
```

This script will:
- Execute `CrossChainERC20.t.sol` tests on L1 fork
- Execute `L2CrossChainERC20.t.sol` tests on L2 fork
- Verify that all bridge operations revert with "not allow bridge" message

**Expected Result**: All tests should pass, confirming that bridge operations are blocked.

### Step 2: Environment Configuration

Create the environment file with the required private key:

```bash
# Create .env file in POC directory
echo "TRANSACTOR_PRIVATE_KEY=xxxxxxxxx" > .env
```

**Note**: Replace with your actual private key. The provided key is the standard Anvil/Hardhat test key.

### Step 3: Fund L2 Deployer Account

Before running the L2 upgrade, ensure the deployer account has native tokens on L2:

```bash
# Example: Transfer native tokens to deployer address
cast send <DEPLOYER_ADDRESS> --value 1ether --private-key <PRIVATE_KEY> --rpc-url http://127.0.0.1:8123
```

### Step 4: Upgrade L1 Bridge

Run the L1 bridge upgrade script to deploy the new implementation without revert functionality:

```bash
# Upgrade L1StandardBridge
forge script L1UpgradeBridge.s.sol:L1UpgradeBridge --rpc-url http://127.0.0.1:8545 --broadcast
```

This script will:
- Load contract addresses from `state.json`
- Deploy new `L1StandardBridgeNew` implementation (without revert)
- Upgrade the proxy through the Transactor contract
- Verify the upgrade was successful

### Step 5: Upgrade L2 Bridge

Run the L2 bridge upgrade script:

```bash
# Upgrade L2StandardBridge
forge script L2UpgradeBridge.s.sol:L2UpgradeBridge --rpc-url http://127.0.0.1:8123 --broadcast
```

This script will:
- Deploy new `L2StandardBridgeNew` implementation (without revert)
- Upgrade the L2 proxy directly (if deployer owns the ProxyAdmin)
- Verify the upgrade was successful

### Step 6: Verify Bridge Functionality

After both upgrades, run the tests again to verify that bridge operations now work:

```bash
# Re-run tests to verify bridge functionality is restored
./run_bridge_tests.sh
```

**Expected Result**: Tests should now fail (in a good way) because the bridge operations no longer revert.

## Key Components

### Test Contracts

- **`CrossChainERC20.t.sol`**: Tests L1 bridge operations (ETH and ERC20 bridging)
- **`L2CrossChainERC20.t.sol`**: Tests L2 bridge operations

### Upgrade Scripts

- **`L1UpgradeBridge.s.sol`**: Handles L1 bridge upgrade through Transactor contract
- **`L2UpgradeBridge.s.sol`**: Handles L2 bridge upgrade directly

### New Implementations

- **`L1StandardBridgeNew.sol`**: L1 bridge implementation without revert functionality
- **`L2StandardBridgeNew.sol`**: L2 bridge implementation without revert functionality

## Architecture Notes

### L1 Upgrade Path
```
EOA (your account) → Transactor Contract → ProxyAdmin → L1StandardBridge Proxy
```

### L2 Upgrade Path
```
EOA (your account) → ProxyAdmin → L2StandardBridge Proxy
```

### Dynamic Address Loading

All scripts automatically load contract addresses from `../../test/config-op/state.json`, ensuring they work with the current deployment state.

## Troubleshooting

### Common Issues

1. **"TRANSACTOR_PRIVATE_KEY not found"**
   - Ensure `.env` file exists in POC directory
   - Verify the private key is correctly formatted with `0x` prefix

2. **"Insufficient funds for gas"**
   - Ensure deployer account has enough ETH on both L1 and L2
   - Check gas price and limit settings

3. **"Ownable: caller is not the owner"**
   - Verify the private key corresponds to the expected owner address
   - Check the ownership chain for L1 (should go through Transactor contract)

4. **"vm.readFile: path not allowed"**
   - Ensure `foundry.toml` has proper file permissions configured
   - Verify the `state.json` file exists at the expected location

### Verification Commands

Check current bridge state:
```bash
# Check L1 bridge state
forge script L1UpgradeBridge.s.sol:L1UpgradeBridge --sig "checkCurrentState()" --rpc-url http://127.0.0.1:8545

# Check L2 bridge state
forge script L2UpgradeBridge.s.sol:L2UpgradeBridge --sig "checkCurrentState()" --rpc-url http://127.0.0.1:8123
```

## Security Considerations

- Private keys in `.env` files should never be committed to version control
- Test private keys should only be used in development/testing environments
- Verify all addresses and implementations before running upgrade scripts on mainnet
- Always test upgrades on testnets before production deployment

## Additional Resources

- [Optimism Bridge Architecture](https://docs.optimism.io/builders/dapp-developers/bridging/standard-bridge)
- [Foundry Scripting Guide](https://book.getfoundry.sh/tutorials/solidity-scripting)
- [Proxy Upgrade Patterns](https://docs.openzeppelin.com/upgrades-plugins/1.x/proxies)
