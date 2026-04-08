---
name: "op-conductor"
description: "Module design for op-conductor: sequencer HA via Raft consensus"
---
# op-conductor Module

## Responsibilities
- Provides high-availability sequencer consensus via Raft
- Coordinates leader election and sequencer state across multiple nodes
- Delegates to op-node for actual sequencing
- Manages health monitoring and leader update processing

## NOT Responsible For
- Block derivation (op-node)
- Batch submission (op-batcher)
- Output proposal (op-proposer)

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| ConductorService | SequencerControl, Consensus, HealthMonitor | Main service with consensus and health |
| Config | NodeRPC, RaftStorageDir, ConsensusAddr | Connection and storage configuration |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Module-Specific Pitfalls

[Pitfall] SequencerControl must initialize before Consensus — ordering dependency in service init
[Pitfall] Health monitor depends on initialized sequencer — cannot start health checks before sequencer ready
[Pitfall] Loop must process leader/health updates atomically — race between handleLeaderUpdate and handleHealthUpdate
[Warning] Missing metrics tracer — conductor metrics not yet added
