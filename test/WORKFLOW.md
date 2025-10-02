# ProxyAdmin Ownership Transfer Workflow

## Timeline

```
┌─────────────────────────────────────────────────────────────────────────┐
│ Phase 1: Initial Setup (1 transaction)                                 │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  1. Deploy FoundationUpgradeSafe (5/7)                                 │
│  2. Deploy SecurityCouncilSafe (10/13) + Liveness                      │
│  3. Deploy ProxyAdminOwnerSafe (1/3)                                   │
│      Owners: [deployer, SecurityCouncil, Foundation]                   │
│  4. Transfer L1 ProxyAdmin → ProxyAdminOwnerSafe                       │
│  5. Send XDomain Message for L2 ProxyAdmin                             │
│                                                                         │
│  Status: L1 ✅ Transferred | L2 ⏳ Pending                            │
└─────────────────────────────────────────────────────────────────────────┘
                              │
                              │ Wait 1-2 minutes
                              ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ Phase 2: Verification                                                   │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  1. L1 transaction mined                                               │
│  2. op-node picks up deposit event                                     │
│  3. Message relayed to L2                                              │
│  4. L2 ProxyAdmin.transferOwnership() executed                         │
│                                                                         │
│  Verify with:                                                          │
│    cast call 0x4200...0018 "owner()" --rpc-url $L2_RPC_URL           │
│                                                                         │
│  Status: L1 ✅ Transferred | L2 ✅ Transferred                        │
└─────────────────────────────────────────────────────────────────────────┘
                              │
                              │ Manual confirmation
                              ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ Phase 3: Finalization (1 transaction)                                  │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  1. Remove deployer from ProxyAdminOwnerSafe                           │
│  2. Change threshold 1/3 → 2/2                                         │
│                                                                         │
│  Final State:                                                          │
│    ProxyAdminOwnerSafe = [SecurityCouncil, Foundation]                │
│    Threshold = 2/2                                                     │
│                                                                         │
│  Status: L1 ✅ Finalized | L2 ✅ Finalized                            │
└─────────────────────────────────────────────────────────────────────────┘
```

## State Transitions

### ProxyAdminOwnerSafe Ownership

```
Initial State (Before Script)
┌──────────────────────────┐
│ Does not exist           │
└──────────────────────────┘

After Phase 1
┌──────────────────────────┐
│ Owners (1/3 threshold):  │
│  1. msg.sender (deployer)│
│  2. SecurityCouncilSafe  │
│  3. FoundationUpgradeSafe│
└──────────────────────────┘

After Phase 3
┌──────────────────────────┐
│ Owners (2/2 threshold):  │
│  1. SecurityCouncilSafe  │
│  2. FoundationUpgradeSafe│
└──────────────────────────┘
```

### L1 ProxyAdmin Owner

```
Before                After Phase 1
┌──────────┐         ┌────────────────────────┐
│Transactor│ ──────→ │ ProxyAdminOwnerSafe    │
└──────────┘         │ (1/3 with deployer)    │
                     └────────────────────────┘
```

### L2 ProxyAdmin Owner

```
Before                After Phase 2
┌──────────────┐     ┌────────────────────────────┐
│ Aliased L1   │ ──→ │ Aliased                    │
│ Deployer or  │     │ ProxyAdminOwnerSafe        │
│ Genesis Addr │     │ (L1 addr + 0x1111...1111)  │
└──────────────┘     └────────────────────────────┘
```

## Commands Quick Reference

### Run Full Transfer
```bash
./test/transfer-proxyadmin-l1-and-l2.sh
```

### Check L2 Status
```bash
cast call 0x4200000000000000000000000000000000000018 "owner()" \
  --rpc-url $L2_RPC_URL
```

### Finalize (Remove Deployer)
```bash
./test/finalize-proxyadmin-ownership.sh
```

## Decision Points

### When to Finalize?

```
┌──────────────────────────────────────┐
│ Has L2 ProxyAdmin owner changed?    │
└───────────┬──────────────────────────┘
            │
    ┌───────┴────────┐
    │                │
   YES               NO
    │                │
    ▼                ▼
┌────────┐    ┌────────────┐
│ READY  │    │ WAIT       │
│ TO     │    │ - Check L1 │
│ FINALIZE│   │ - Check op-│
└────────┘    │   node logs│
              └────────────┘
```

### What if L2 Transfer Fails?

```
┌──────────────────────────────────────┐
│ L2 Transfer Failed or Stuck?        │
└───────────┬──────────────────────────┘
            │
    ┌───────┴────────┐
    │                │
  L1 TX             L2 NOT
  FAILED           PROCESSING
    │                │
    ▼                ▼
┌────────┐      ┌─────────────┐
│ Retry  │      │ Wait longer │
│ Full   │      │ Check op-   │
│ Script │      │ node status │
└────────┘      └─────────────┘
    │                │
    │         ┌──────┴──────┐
    │        YES            NO
    │         │              │
    │    ┌────▼────┐    ┌────▼────┐
    │    │ Wait    │    │ Debug   │
    │    │ for     │    │ op-node │
    │    │ relay   │    └─────────┘
    │    └─────────┘
    │
    └──► Can retry with deployer still in Safe
         No need to finalize until L2 confirms
```

## Safety Features

### Why This Design?

```
┌─────────────────────────────────────────────────┐
│ Problem: L2 ProxyAdmin needs message from       │
│          L1 contract, not EOA                   │
├─────────────────────────────────────────────────┤
│                                                 │
│ Solution: Deployer in 1/3 Safe temporarily     │
│                                                 │
│ Benefits:                                       │
│  ✅ Deployer can send L1XDomain message       │
│  ✅ No deadlock if L2 transfer fails          │
│  ✅ Can retry without needing multisig        │
│  ✅ Remove deployer after L2 confirms         │
│  ✅ Final state = secure 2/2 governance       │
└─────────────────────────────────────────────────┘
```

### Rollback Plan

```
If something goes wrong BEFORE finalization:

┌──────────────────────────────────────┐
│ Deployer still in ProxyAdminOwnerSafe│
│ (1/3 threshold)                      │
└──────────────────────────────────────┘
         │
         ▼
┌──────────────────────────────────────┐
│ Deployer can:                        │
│  - Send new L1XDomain messages       │
│  - Retry L2 transfer                 │
│  - Update configurations             │
└──────────────────────────────────────┘

If something goes wrong AFTER finalization:

┌──────────────────────────────────────┐
│ Deployer removed                     │
│ Requires SecurityCouncil + Foundation│
└──────────────────────────────────────┘
         │
         ▼
┌──────────────────────────────────────┐
│ Must coordinate multisig approvals   │
│ for any recovery actions             │
└──────────────────────────────────────┘
```

## Integration with Existing Infrastructure

### If You Already Have Safes

```bash
# Option 1: Use existing Safes
# Modify the script to use your existing Safe addresses
# instead of deploying new ones

# Option 2: Deploy new Safes and transfer ownership later
# This is the default behavior
```

### If ProxyAdmin Has Different Owner

```bash
# The script assumes Transactor owns ProxyAdmin
# If your setup is different, you need to:
# 1. Transfer to Transactor first, OR
# 2. Modify the script to use your pattern
```

## Monitoring

### What to Watch

```
Phase 1 (Initial Transfer):
  Monitor: L1 transaction confirmation
  Tools:   Etherscan, forge logs

Phase 2 (L2 Relay):
  Monitor: op-node logs, L2 transaction
  Tools:   op-node logs, L2 block explorer

Phase 3 (Finalization):
  Monitor: L1 transaction confirmation
  Tools:   Etherscan, Safe UI
```

### Alerts

```
Critical Events to Monitor:
  ✅ L1 ProxyAdmin ownership changed
  ✅ L1XDomain message sent
  ⏳ L2 deposit event detected
  ⏳ L2 message processing
  ✅ L2 ProxyAdmin ownership changed
  ✅ Deployer removed from Safe
  ✅ Threshold changed to 2/2
```

---

## Quick Command Reference Card

```bash
# Full workflow
./test/transfer-proxyadmin-l1-and-l2.sh

# Check L1 ProxyAdmin owner
cast call $PROXY_ADMIN "owner()" --rpc-url $L1_RPC_URL

# Check L2 ProxyAdmin owner
cast call 0x4200000000000000000000000000000000000018 "owner()" \
  --rpc-url $L2_RPC_URL

# Check Safe configuration
cast call $PROXY_ADMIN_OWNER_SAFE "getOwners()" --rpc-url $L1_RPC_URL
cast call $PROXY_ADMIN_OWNER_SAFE "getThreshold()" --rpc-url $L1_RPC_URL

# Finalize (after L2 confirms)
./test/finalize-proxyadmin-ownership.sh

# Compute aliased address (for verification)
python3 -c "print(hex(int('$L1_SAFE_ADDR', 16) + 0x1111000000000000000000000000000000001111))"
```
