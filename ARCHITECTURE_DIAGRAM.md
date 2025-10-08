# Custom Gas Token Architecture Diagram

## System Architecture

```mermaid
graph TB
    subgraph "L1 - Ethereum"
        User[User]
        OKB[OKB Token Contract]
        Adapter[DepositedOKBAdapter]
        Portal[OptimismPortal2]
        SystemConfig[SystemConfig]

        User -->|1. Approve OKB| OKB
        User -->|2. deposit| Adapter
        Adapter -->|3. transferFrom| OKB
        Adapter -->|4. burn to address(0)| OKB
        Adapter -->|5. mint dOKB| Adapter
        Adapter -->|6. depositERC20Transaction| Portal
        Portal -->|7. transferFrom dOKB| Adapter
        Portal -->|8. emit TransactionDeposited| Portal
        SystemConfig -->|Feature Flag| Portal
    end

    subgraph "L2 - Optimism"
        Derivation[Rollup Node Derivation]
        L1Block[L1BlockCGT]
        L2ToL1[L2ToL1MessagePasserCGT]
        UserL2[User L2 Address]

        Portal -.->|9. Event Log| Derivation
        Derivation -->|10. Derive Deposit Tx| UserL2
        UserL2 -->|11. Use CGT| L2ToL1
        L1Block -->|Token Name/Symbol| UserL2
    end

    style Adapter fill:#f9f,stroke:#333,stroke-width:4px
    style Portal fill:#bbf,stroke:#333,stroke-width:4px
    style L1Block fill:#bfb,stroke:#333,stroke-width:4px
```

## Deposit Flow Sequence

```mermaid
sequenceDiagram
    participant U as User
    participant OKB as OKB Token
    participant A as DepositedOKBAdapter
    participant P as OptimismPortal2
    participant R as Rollup Node
    participant L2 as L2 Chain

    Note over U,L2: Standard CGT Deposit Flow
    U->>OKB: approve(Adapter, amount)
    U->>A: deposit(to, amount)
    A->>OKB: transferFrom(user, adapter, amount)
    A->>OKB: transfer(address(0), amount)
    Note over OKB: OKB Burned ♨️
    A->>A: mint(adapter, amount)
    Note over A: dOKB Minted 🪙
    A->>P: depositERC20Transaction(...)
    P->>A: transferFrom(adapter, portal, amount)
    P->>P: emit TransactionDeposited
    R->>R: Derive deposit transaction
    R->>L2: Include deposit in L2 block
    Note over L2: User receives CGT on L2 ✅
```

## Contract Interaction Map

```mermaid
graph LR
    subgraph "ERC20 Layer"
        OKB[OKB Token<br/>ERC20]
        dOKB[DepositedOKBAdapter<br/>ERC20 + Logic]
    end

    subgraph "Portal Layer"
        Portal[OptimismPortal2<br/>Deposit Handler]
        SC[SystemConfig<br/>Feature Flags]
    end

    subgraph "Storage Layer"
        GPT[GasPayingToken<br/>Library]
        Storage[(Storage Slots)]
    end

    subgraph "L2 Predeploys"
        L1B[L1BlockCGT<br/>Token Info]
        L2ToL1[L2ToL1MessagePasserCGT<br/>Withdrawal Blocker]
    end

    OKB -->|burn| dOKB
    dOKB -->|deposit| Portal
    Portal -->|read token| GPT
    GPT -->|access| Storage
    SC -->|feature flag| Portal
    Storage -.->|shared storage| L1B
    L1B -->|token metadata| L2ToL1

    style dOKB fill:#f9f,stroke:#333,stroke-width:2px
    style Portal fill:#bbf,stroke:#333,stroke-width:2px
    style L1B fill:#bfb,stroke:#333,stroke-width:2px
```

## Storage Layout

```mermaid
graph TD
    subgraph "L1: OptimismPortal2 Storage"
        S1[GAS_PAYING_TOKEN_SLOT<br/>address + decimals]
        S2[GAS_PAYING_TOKEN_NAME_SLOT<br/>token name]
        S3[GAS_PAYING_TOKEN_SYMBOL_SLOT<br/>token symbol]
    end

    subgraph "L2: L1BlockCGT Storage"
        L1[IS_CUSTOM_GAS_TOKEN_SLOT<br/>bool flag]
        L2[Inherited L1Block storage]
    end

    subgraph "GasPayingToken Library"
        Lib[Read/Write Functions]
    end

    Lib -->|read/write| S1
    Lib -->|read/write| S2
    Lib -->|read/write| S3
    Lib -->|read| L1

    style S1 fill:#ffd,stroke:#333
    style S2 fill:#ffd,stroke:#333
    style S3 fill:#ffd,stroke:#333
    style L1 fill:#dff,stroke:#333
```

## Feature Flag Flow

```mermaid
graph TD
    A[Deploy Contracts] --> B[Set Gas Paying Token<br/>in Portal Storage]
    B --> C[Enable CUSTOM_GAS_TOKEN<br/>in SystemConfig]
    C --> D{Check Feature Flag}
    D -->|Enabled| E[Use depositERC20Transaction]
    D -->|Disabled| F[Use depositTransaction with ETH]
    E --> G[CGT Mode Active]
    F --> H[ETH Mode Active]

    style C fill:#f96,stroke:#333,stroke-width:2px
    style E fill:#9f9,stroke:#333,stroke-width:2px
    style F fill:#99f,stroke:#333,stroke-width:2px
```

## Comparison: ETH vs CGT Chains

```mermaid
graph TB
    subgraph "ETH Chain"
        E1[User] -->|ETH + data| E2[depositTransaction]
        E2 -->|lock ETH| E3[ETHLockbox]
        E2 -->|emit event| E4[L2 Derivation]
        E4 -->|mint ETH| E5[L2 User Balance]
    end

    subgraph "CGT Chain"
        C1[User] -->|approve ERC20| C2[CGT Token]
        C1 -->|deposit| C3[depositERC20Transaction]
        C3 -->|transferFrom| C2
        C3 -->|lock tokens| C3
        C3 -->|emit event| C4[L2 Derivation]
        C4 -->|mint CGT| C5[L2 User Balance]
    end

    style E3 fill:#99f,stroke:#333
    style C3 fill:#9f9,stroke:#333
```

## Token Flow: Burn and Mint

```mermaid
graph LR
    subgraph "L1 Supply"
        L1_Before[21M OKB Total]
        L1_Burn[OKB Burned<br/>to address 0]
        L1_After[<21M OKB Remaining]
        L1_dOKB[dOKB Minted<br/>locked in Adapter]
    end

    subgraph "Bridge"
        Bridge[OptimismPortal2<br/>Lock & Emit]
    end

    subgraph "L2 Supply"
        L2_Mint[CGT Minted on L2]
        L2_User[User Balance]
    end

    L1_Before --> L1_Burn
    L1_Burn --> L1_After
    L1_Burn -.-> L1_dOKB
    L1_dOKB --> Bridge
    Bridge -.-> L2_Mint
    L2_Mint --> L2_User

    style L1_Burn fill:#f99,stroke:#333,stroke-width:2px
    style L1_dOKB fill:#f9f,stroke:#333,stroke-width:2px
    style L2_Mint fill:#9f9,stroke:#333,stroke-width:2px
```

## Key Design Principles

### 1. **Separation of Concerns**
- **Adapter Layer**: Handles token-specific logic (OKB burning)
- **Portal Layer**: Generic deposit mechanism for any CGT
- **L2 Layer**: Token metadata and withdrawal restrictions

### 2. **Lock and Mint Pattern**
- L1: Lock tokens in Portal (or burn via Adapter)
- L2: Mint equivalent tokens to user
- Maintains 1:1 supply between L1 and L2

### 3. **Backward Compatibility**
- ETH chains continue using `depositTransaction`
- CGT chains use `depositERC20Transaction`
- Feature flag controls behavior

### 4. **Security Through Restriction**
- DepositedOKBAdapter: Limited transfers
- Portal: Feature flag checks
- L2ToL1MessagePasserCGT: Value restrictions

### 5. **Generic Infrastructure**
- Same Portal code for all CGT chains
- Token-specific adapters (like DepositedOKBAdapter)
- Shared storage patterns via GasPayingToken library
