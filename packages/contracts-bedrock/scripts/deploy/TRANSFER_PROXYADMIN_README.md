# ProxyAdmin Ownership Transfer Scripts

This directory contains scripts to transfer L1 and L2 ProxyAdmin ownership from the deployer to a multi-sig governance structure.

## Step 1: Transfer L1 ProxyAdmin

This script will:
1. Deploy FoundationUpgradeSafe (5/7 multisig)
2. Deploy SecurityCouncilSafe (10/13 multisig) with LivenessModule/Guard
3. Deploy ProxyAdminOwnerSafe (2/2 multisig)
4. Transfer L1 ProxyAdmin ownership via Transactor

```bash
cd packages/contracts-bedrock

PROXY_ADMIN=<l1_proxy_admin_address> \
TRANSACTOR=<transactor_address> \
forge script scripts/deploy/TransferProxyAdminL1.s.sol:TransferProxyAdminL1 \
  --rpc-url $L1_RPC_URL \
  --private-key $DEPLOYER_PRIVATE_KEY \
  --broadcast
```

**After successful execution, note the `ProxyAdminOwnerSafe` address from the output.**

## Step 2: Transfer L2 ProxyAdmin

This script will:
1. Check current L2 ProxyAdmin owner (should be deployer)
2. Compute aliased L1 ProxyAdminOwnerSafe address
3. Transfer L2 ProxyAdmin ownership to aliased address

```bash
PROXY_ADMIN_OWNER_SAFE=<address_from_step1> \
forge script scripts/deploy/TransferProxyAdminL2.s.sol:TransferProxyAdminL2 \
  --rpc-url $L2_RPC_URL \
  --private-key $DEPLOYER_PRIVATE_KEY \
  --broadcast
```

## Verification

After both scripts complete:

```bash
# Check L1 ProxyAdmin owner
cast call $PROXY_ADMIN "owner()" --rpc-url $L1_RPC_URL
# Should return: ProxyAdminOwnerSafe address

# Check L2 ProxyAdmin owner
cast call 0x4200000000000000000000000000000000000018 "owner()" --rpc-url $L2_RPC_URL
# Should return: Aliased ProxyAdminOwnerSafe address
```

## Understanding Address Aliasing

### Why Aliasing?

When an L1 contract sends a cross-domain message to L2, the sender address on L2 is "aliased" by adding an offset:

```
aliased_address = l1_address + 0x1111000000000000000000000000000000001111
```

### Why L2 ProxyAdmin Needs an Aliased Owner

The L2 ProxyAdmin owner is set to an **aliased address** so that:
1. The L1 ProxyAdminOwnerSafe can send cross-domain messages
2. On L2, those messages appear to come from the aliased address
3. The aliased address matches the L2 ProxyAdmin owner
4. The ownership transfer succeeds

### Initial State vs Final State

**Initial State (after genesis):**
- L1 ProxyAdmin owner: `Transactor` (or deployer)
- L2 ProxyAdmin owner: `Deployer` (NOT aliased)

**Final State (after scripts):**
- L1 ProxyAdmin owner: `ProxyAdminOwnerSafe`
- L2 ProxyAdmin owner: `aliased(ProxyAdminOwnerSafe)`

**Why L2 owner is NOT aliased initially:**
- During genesis, the L2 ProxyAdmin owner is set to a regular address (deployer)
- This requires a direct L2 transaction (not cross-domain) to transfer ownership
- After transfer, the new owner is aliased so future changes can be made via L1

---

## Governance Structure

After the transfer completes, the governance structure is:

```
ProxyAdminOwnerSafe (2/2 multisig)
├── SecurityCouncilSafe (10/13 multisig with liveness protection)
└── FoundationUpgradeSafe (5/7 multisig)
```

**To upgrade a proxy:**
1. Both SecurityCouncilSafe AND FoundationUpgradeSafe must approve
2. Threshold: 2/2 (both safes must sign)
3. For L2 upgrades: Use cross-domain messaging from L1

---

## Troubleshooting

### Error: "Insufficient L2 balance for gas fees"

**Solution**: Fund the deployer address on L2:
```bash
cast send $DEPLOYER_ADDRESS --value 0.01ether --private-key $FUNDER_KEY --rpc-url $L2_RPC_URL
```

### Error: "msg.sender is not L2 ProxyAdmin owner"

**Solution**: Verify the deployer private key matches the current L2 ProxyAdmin owner:
```bash
# Check current owner
cast call 0x4200000000000000000000000000000000000018 "owner()" --rpc-url $L2_RPC_URL

# Check deployer address
cast wallet address --private-key $DEPLOYER_PRIVATE_KEY
```

### Error: "already owned by target address"

The transfer has already been completed. Verify with:
```bash
cast call 0x4200000000000000000000000000000000000018 "owner()" --rpc-url $L2_RPC_URL
```

---

## Security Considerations

1. **Verify addresses**: Double-check all safe addresses before executing
2. **Test first**: Run without `--broadcast` flag to simulate
3. **Sequential execution**: For separate scripts, wait for L1 confirmation before L2
4. **Backup plan**: Ensure you have access to all safe owners' keys
5. **Monitor transactions**: Watch for transaction confirmation on both chains
