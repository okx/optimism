# L1 and L2 ProxyAdmin Ownership Transfer

Comprehensive solution for transferring ProxyAdmin ownership on both L1 and L2 with proper governance structure.

## Overview

This solution combines:
1. **L1 ProxyAdmin Transfer**: Via Transactor pattern to a governance multisig
2. **L2 ProxyAdmin Transfer**: Via cross-domain messaging to an aliased L1 Safe
3. **Temporary Deployer Access**: Deployer remains in 1/3 multisig until L2 transfer confirms
4. **Final Cleanup**: Remove deployer and transition to 2/2 governance

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        L1 Ethereum                          │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌──────────────────┐    ┌─────────────────────┐          │
│  │ SecurityCouncil  │    │ FoundationUpgrade   │          │
│  │ Safe (10/13)     │    │ Safe (5/7)          │          │
│  └────────┬─────────┘    └──────────┬──────────┘          │
│           │                          │                      │
│           └──────────┬───────────────┘                      │
│                      │                                      │
│              ┌───────▼────────────┐                        │
│              │ ProxyAdminOwner    │                        │
│              │ Safe (1/3 → 2/2)   │◄─── Deployer (temp)   │
│              └───────┬────────────┘                        │
│                      │                                      │
│                      │ owns                                 │
│              ┌───────▼────────────┐                        │
│              │ L1 ProxyAdmin      │                        │
│              │ (via Transactor)   │                        │
│              └────────────────────┘                        │
│                      │                                      │
│                      │ sendMessage                          │
│              ┌───────▼────────────┐                        │
│              │ L1XDomainMessenger │                        │
│              └───────┬────────────┘                        │
└──────────────────────┼──────────────────────────────────────┘
                       │
                       │ Cross-Domain Message
                       │
┌──────────────────────▼──────────────────────────────────────┐
│                        L2 OP Chain                          │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│              ┌────────────────────┐                        │
│              │ L2 ProxyAdmin      │                        │
│              │ (Predeploy)        │                        │
│              │ 0x4200...0018      │                        │
│              └────────┬───────────┘                        │
│                       │                                      │
│                       │ owned by (aliased)                   │
│                       │                                      │
│              ┌────────▼───────────┐                        │
│              │ ProxyAdminOwner    │                        │
│              │ Safe (aliased)     │                        │
│              │ + 0x1111...1111    │                        │
│              └────────────────────┘                        │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## Files

### Foundry Script
- **`packages/contracts-bedrock/scripts/deploy/TransferProxyAdminL1AndL2.s.sol`**
  - Main Solidity script that handles entire workflow
  - Deploys Safe multisigs with Liveness protection
  - Transfers L1 ProxyAdmin ownership via Transactor
  - Sends cross-domain message for L2 transfer
  - Provides finalization function to remove deployer

### Shell Scripts
- **`test/transfer-proxyadmin-l1-and-l2.sh`**
  - Wrapper script to execute the main Foundry script
  - Validates environment variables
  - Provides clear status updates

- **`test/finalize-proxyadmin-ownership.sh`**
  - Run AFTER L2 transfer is confirmed
  - Removes deployer from ProxyAdminOwnerSafe
  - Changes threshold from 1/3 to 2/2

## Prerequisites

1. **Deployed Contracts**:
   - `PROXY_ADMIN`: L1 ProxyAdmin contract
   - `TRANSACTOR`: Transactor contract (owns ProxyAdmin)
   - `L1_CROSS_DOMAIN_MESSENGER_PROXY`: L1CrossDomainMessenger

2. **Environment Variables** (in `.env`):
   ```bash
   # RPC URLs
   L1_RPC_URL=https://your-l1-rpc
   L2_RPC_URL=https://your-l2-rpc

   # Contract Addresses
   PROXY_ADMIN=0x...
   TRANSACTOR=0x...
   L1_CROSS_DOMAIN_MESSENGER_PROXY=0x...

   # Keys
   DEPLOYER_PRIVATE_KEY=0x...
   ETHERSCAN_API_KEY=your_key  # Optional, for verification
   ```

3. **Requirements**:
   - Deployer must own the Transactor contract
   - Transactor must own the L1 ProxyAdmin
   - Foundry (forge) installed

## Usage

### Step 1: Transfer Both L1 and L2 ProxyAdmin Ownership

```bash
cd /Users/jasonhuang/code/op/optimism
./test/transfer-proxyadmin-l1-and-l2.sh
```

This will:
1. ✅ Deploy FoundationUpgradeSafe (5/7 multisig)
2. ✅ Deploy SecurityCouncilSafe (10/13 multisig)
3. ✅ Configure LivenessModule and LivenessGuard
4. ✅ Deploy ProxyAdminOwnerSafe (1/3 multisig with deployer, SecurityCouncil, Foundation)
5. ✅ Transfer L1 ProxyAdmin ownership to ProxyAdminOwnerSafe
6. ✅ Send cross-domain message to transfer L2 ProxyAdmin ownership

**Result**:
- L1 ProxyAdmin owned by ProxyAdminOwnerSafe (1/3 threshold)
- L2 ProxyAdmin transfer pending (waiting for cross-domain relay)
- Deployer can still act on behalf of the Safe

### Step 2: Wait for L2 Transfer (~1-2 minutes)

Monitor L2 ProxyAdmin ownership:
```bash
cast call 0x4200000000000000000000000000000000000018 "owner()" \
  --rpc-url $L2_RPC_URL
```

Expected owner will be the **aliased** ProxyAdminOwnerSafe address:
```
L1 Address:     0xYourProxyAdminOwnerSafe
Aliased L2:     0x{L1Address + 0x1111000000000000000000000000000000001111}
```

### Step 3: Finalize Governance Structure

After confirming L2 ownership transfer:
```bash
cd /Users/jasonhuang/code/op/optimism
./test/finalize-proxyadmin-ownership.sh
```

This will:
1. ✅ Remove deployer from ProxyAdminOwnerSafe
2. ✅ Change threshold from 1/3 to 2/2
3. ✅ Finalize governance (SecurityCouncil + Foundation required)

**Final Result**:
- L1 ProxyAdmin: Owned by ProxyAdminOwnerSafe (2/2)
- L2 ProxyAdmin: Owned by aliased ProxyAdminOwnerSafe (2/2)
- Both require SecurityCouncilSafe + FoundationUpgradeSafe approval

## Manual Verification

### Check L1 ProxyAdmin Owner
```bash
cast call $PROXY_ADMIN "owner()" --rpc-url $L1_RPC_URL
```

### Check L2 ProxyAdmin Owner
```bash
cast call 0x4200000000000000000000000000000000000018 "owner()" \
  --rpc-url $L2_RPC_URL
```

### Check ProxyAdminOwnerSafe Configuration
```bash
# Get owners
cast call $PROXY_ADMIN_OWNER_SAFE "getOwners()" --rpc-url $L1_RPC_URL

# Get threshold
cast call $PROXY_ADMIN_OWNER_SAFE "getThreshold()" --rpc-url $L1_RPC_URL
```

## Security Considerations

### Why 1/3 Initially?
- Allows deployer to send cross-domain message on behalf of Safe
- L2 ProxyAdmin requires message from L1 contract (not EOA)
- Deployer can act alone for initial L2 transfer
- Prevents deadlock if L2 transfer fails

### Why Remove Deployer After?
- Deployer is hot wallet (higher risk)
- Production governance should require multisig approval
- 2/2 between SecurityCouncil and Foundation = proper checks and balances

### Liveness Protection
- **LivenessModule**: Removes inactive Security Council members
- **LivenessGuard**: Monitors and enforces activity requirements
- **Fallback**: FoundationUpgradeSafe can take over if council inactive

## Troubleshooting

### L2 Transfer Not Happening?
1. Check L1 transaction was mined
2. Verify op-node is running and synced
3. Check op-node logs for deposit event processing
4. May take up to 5 minutes in congested networks

### Can't Finalize (Remove Deployer)?
1. Ensure L2 ownership has transferred first
2. Verify you're using the correct deployer private key
3. Check deployer is still an owner of ProxyAdminOwnerSafe

### Wrong Address on L2?
The L2 owner must be the **aliased** L1 address:
```python
aliased_address = hex(int(l1_address, 16) + 0x1111000000000000000000000000000000001111)
```

## Advanced: Manual Execution

If you need more control, run the Foundry script directly:

```bash
cd packages/contracts-bedrock

# Main transfer
forge script scripts/deploy/TransferProxyAdminL1AndL2.s.sol:TransferProxyAdminL1AndL2 \
  --rpc-url $L1_RPC_URL \
  --private-key $DEPLOYER_PRIVATE_KEY \
  --broadcast \
  --verify \
  -vvv

# Finalization (after L2 confirms)
forge script scripts/deploy/TransferProxyAdminL1AndL2.s.sol:TransferProxyAdminL1AndL2 \
  --sig "finalizeProxyAdminOwnerSafe()" \
  --rpc-url $L1_RPC_URL \
  --private-key $DEPLOYER_PRIVATE_KEY \
  --broadcast \
  -vvv
```

## Production Checklist

- [ ] Test on testnet first (Sepolia)
- [ ] Verify all environment variables are correct
- [ ] Ensure Transactor owns ProxyAdmin
- [ ] Ensure deployer owns Transactor
- [ ] Have Security Council multisig signers ready
- [ ] Have Foundation multisig signers ready
- [ ] Backup all contract addresses
- [ ] Document the ProxyAdminOwnerSafe address
- [ ] Document the aliased L2 address
- [ ] Wait for L2 confirmation before finalizing
- [ ] Verify both L1 and L2 ownership after completion

## References

- **L1 ProxyAdmin**: Standard Ownable contract for proxy upgrades
- **L2 ProxyAdmin**: Predeploy at `0x4200000000000000000000000000000000000018`
- **Address Aliasing**: Prevents address collision attacks in cross-domain messaging
- **Transactor Pattern**: Enables delegatecall for OPCM upgrades
- **Liveness Protection**: Ensures active governance (Security Council specific)

## Support

For issues or questions:
1. Check the console output for detailed error messages
2. Verify all environment variables are set correctly
3. Ensure contracts are deployed at expected addresses
4. Review the Foundry script logs with `-vvvv` for maximum verbosity

---

**Note**: This is a critical operation that affects the security and upgradeability of both L1 and L2 contracts. Always test thoroughly on testnet before executing on mainnet.
