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
    L2_Bridge->>L2_WETH: burn(User, amount)
    L2_WETH-->>L2_Bridge: WOKB burned
    Note over User: L2 WOKB balance decreases

    %% Step 4: Cross-domain message
    L2_Bridge->>L1_Bridge: Cross-domain message<br/>(withdrawal proof)
    Note over L1_Bridge, Challenge: Message enters challenge period

    %% Step 5: Challenge period
    Challenge-->>Challenge: Wait for challenge period<br/>(MAX_CLOCK_DURATION)

    %% Step 6: Finalize withdrawal (after challenge period)
    L1_Bridge->>L1_WOKB: mint(User, amount)
    L1_WOKB-->>User: WOKB minted on L1
    Note over User: L1 WOKB balance increases
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
    L1_WOKB-->>L1_Bridge: WOKB burned
    Note over User: L1 WOKB balance decreases

    %% Step 3: Cross-domain message (automatic relay)
    L1_Bridge->>L2_Bridge: Cross-domain message<br/>(deposit relay)
    Note over L1_Bridge, L2_Bridge: Automatic relay<br/>(few minutes)

    %% Step 4: Finalize deposit
    L2_Bridge->>L2_WETH: mint(User, amount)
    L2_WETH-->>User: WOKB minted on L2
    Note over User: L2 WOKB balance increases

    %% Step 5: User can convert back to OKB if needed
    User->>L2_WETH: withdraw(amount)
    L2_WETH->>L2_WETH: burn(User, amount)
    L2_WETH-->>User: Send OKB
    Note over User: User receives OKB on L2
```

## Complete Test Flow

```mermaid
flowchart TD
    A[Start: User has OKB on L2] --> B[Step 1: Convert OKB → WOKB on L2]
    B --> C[Step 2: L2→L1 Withdraw WOKB]
    C --> D[Wait Challenge Period<br/>MAX_CLOCK_DURATION]
    D --> E[Step 3: L1→L2 Deposit WOKB]
    E --> F[Step 4: Convert WOKB → OKB on L2]

    B -.-> B1[L2 WOKB increases<br/>L2 OKB decreases]
    C -.-> C1[L2 WOKB decreases<br/>L1 WOKB pending]
    D -.-> D1[L1 WOKB increases<br/>Challenge period completes]
    E -.-> E1[L1 WOKB decreases<br/>L2 WOKB increases]
    F -.-> F1[L2 WOKB decreases<br/>L2 OKB increases]

    style A fill:#e1f5fe
    style D fill:#fff3e0
    style F fill:#e8f5e8
```

## Key Concepts

- **L2 → L1**: Withdrawal with challenge period for security
- **L1 → L2**: Deposit with automatic relay for speed
- **Challenge Period**: Configurable via `MAX_CLOCK_DURATION`
- **Test Split**: Avoid waiting during automated testing
