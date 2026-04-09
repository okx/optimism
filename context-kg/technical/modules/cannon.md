---
name: "cannon"
description: "Module design for cannon: fault proof VM for MIPS/RISCV instruction execution"
---
# cannon Module

## Responsibilities
- MIPS and RISCV instruction execution for fault proof traces
- Generates execution traces for dispute game step verification
- Provides prestate snapshots at configurable frequency

## NOT Responsible For
- Online execution (offline trace generation only)
- Dispute game coordination (op-challenger)

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| VM | SnapshotFreq, InfoFreq, PreState | Instruction-level execution engine |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Module-Specific Pitfalls

[Pitfall] SnapshotFreq and InfoFreq must not be zero — required for trace generation; config validation enforced
