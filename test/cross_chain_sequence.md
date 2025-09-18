# CGT Cross-Chain Sequence Diagrams

## L2 → L1 Withdrawal Sequence

```mermaid
sequenceDiagram
    participant User
    participant L2_WETH as L2 WETH Contract
    participant L2_Bridge as L2StandardBridge
    participant L1_Bridge as L1StandardBridge
    participant L1_WOKB as L1 OptimismMintableERC20
    participant Challenge as Challenge Period

    Note over User, Challenge: L2 → L1 Withdrawal Process

    %% Step 1: User has OKB, converts to WOKB
    User->>L2_WETH: deposit() with OKB
    L2_WETH-->>User: Mint WOKB
    Note over User: User now has WOKB on L2

    %% Step 2: Approve bridge
    User->>L2_WETH: approve(L2_Bridge, amount)
    L2_WETH-->>User: Approval confirmed

    %% Step 3: Initiate withdrawal
    User->>L2_Bridge: bridgeERC20(L2_WETH, L1_WOKB, amount)
    L2_Bridge->>L2_WETH: transferFrom(User, L2_Bridge, amount)
    L2_WETH-->>L2_Bridge: WOKB locked in L2 bridge
    Note over User: L2 WOKB balance decreases

    %% Step 4: Cross-domain message
    L2_Bridge->>L1_Bridge: Cross-domain message<br/>(withdrawal initiated)
    Note over L1_Bridge, Challenge: Withdrawal enters challenge period

    %% Step 5: Prove withdrawal (manual step)
    User->>L1_Bridge: withdrawal prove (manual)
    Note over L1_Bridge: Withdrawal proof submitted

    %% Step 6: Challenge period
    Challenge-->>Challenge: Wait for challenge period<br/>(MAX_CLOCK_DURATION = 20s)

    %% Step 7: Finalize withdrawal (manual step)
    User->>L1_Bridge: withdrawal finalize (manual)
    L1_Bridge->>L1_WOKB: mint(User, amount)
    L1_WOKB-->>User: WOKB minted on L1
    Note over User: L1 WOKB balance increases<br/>(L2 WOKB remains locked)
```

## L1 → L2 Deposit Sequence

```mermaid
sequenceDiagram
    participant User
    participant L1_WOKB as L1 OptimismMintableERC20
    participant L1_Bridge as L1StandardBridge
    participant L2_Bridge as L2StandardBridge
    participant L2_WETH as L2 WETH Contract

    Note over User, L2_WETH: L1 → L2 Deposit Process

    %% Prerequisite: User has WOKB on L1
    Note over User: User has WOKB on L1<br/>(from previous L2→L1 withdrawal)

    %% Step 1: Approve bridge
    User->>L1_WOKB: approve(L1_Bridge, amount)
    L1_WOKB-->>User: Approval confirmed

    %% Step 2: Initiate deposit
    User->>L1_Bridge: depositERC20(L1_WOKB, L2_WETH, amount)
    L1_Bridge->>L1_WOKB: burn(User, amount)
    L1_WOKB-->>L1_Bridge: WOKB burned on L1
    Note over User: L1 WOKB balance decreases

    %% Step 3: Cross-domain message (automatic relay)
    L1_Bridge->>L2_Bridge: Cross-domain message<br/>(deposit relay)
    Note over L1_Bridge, L2_Bridge: Automatic processing<br/>(2-5 minutes)

    %% Step 4: Finalize deposit (automatic)
    L2_Bridge->>L2_WETH: unlock/transfer(User, amount)
    L2_WETH-->>User: WOKB unlocked on L2
    Note over User: L2 WOKB balance increases

    %% Step 5: User can convert back to OKB if needed
    User->>L2_WETH: withdraw(amount)
    L2_WETH->>L2_WETH: burn(User, amount)
    L2_WETH-->>User: Send OKB
    Note over User: User receives OKB on L2
```

## Key Concepts

### Token Mechanics

#### L2 → L1 Withdrawal (Lock/Mint) - CGT Mode
- **L2**: WOKB tokens are **locked** in L2StandardBridge (WETH is native contract)
- **L1**: WOKB tokens are **minted** on L1 OptimismMintableERC20 (after challenge period)
- **Security**: Challenge period allows dispute of invalid withdrawals

#### L1 → L2 Deposit (Burn/Unlock) - CGT Mode
- **L1**: WOKB tokens are **burned** on L1 OptimismMintableERC20 contract
- **L2**: Previously locked WOKB tokens are **unlocked** from L2StandardBridge
- **Speed**: No challenge period needed, faster processing

### Timing
- **L2 → L1**: Manual prove/finalize steps + challenge period (20s in test)
- **L1 → L2**: Automatic processing (2-5 minutes)
- **Challenge Period**: Configurable via `MAX_CLOCK_DURATION`

### Test Design
- **Part 1**: Complete L2→L1 withdrawal flow (includes prove/finalize)
- **Part 2**: Independent L1→L2 deposit testing
- **Verification**: Real-time balance checking at each step
