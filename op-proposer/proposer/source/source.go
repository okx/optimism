package source

import (
	"context"
	"encoding/binary"

	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum/go-ethereum/common"
)

type Proposal struct {
	// Root is the proposal hash
	Root common.Hash

	// SequenceNum represents the L2 Block number or Super Root L2 timestamp
	SequenceNum uint64

	// Super is present if, and only if, this Proposal is a Super Root proposal
	Super eth.Super

	CurrentL1 eth.BlockID

	// Legacy provides data that is only available when retrieving data from a single rollup node.
	// It should only be used for optional logs and metrics.
	Legacy LegacyProposalData

	// For xlayer: TeeRollupData is present if this is a TEE game type 1960 proposal
	TeeRollupData *TeeRollupProposalData
}

// For xlayer: TEE proposal data for TeeRollup game type (1960)
type TeeRollupProposalData struct {
	L2SeqNum  uint64
	ParentIdx uint32      // 4 bytes; l2SeqNum encoded as uint256 (32 bytes) → total extraData 100 bytes
	BlockHash common.Hash
	StateHash common.Hash
}

// IsSuperRootProposal returns true if the proposal is a Super Root proposal.
func (p *Proposal) IsSuperRootProposal() bool {
	return p.Super != nil
}

// For xlayer: IsTEEProposal returns true if the proposal is a TeeRollup TEE proposal.
func (p *Proposal) IsTEEProposal() bool { return p.TeeRollupData != nil }

// ExtraData returns the Dispute Game extra data as appropriate for the proposal type.
func (p *Proposal) ExtraData() []byte {
	if p.Super != nil {
		return p.Super.Marshal()
	} else if p.TeeRollupData != nil { // For xlayer
		return encodeTeeRollupExtraData(p.TeeRollupData)
	} else {
		var extraData [32]byte
		binary.BigEndian.PutUint64(extraData[24:], p.SequenceNum)
		return extraData[:]
	}
}

// For xlayer: encodeTeeRollupExtraData encodes TeeRollup proposal data into 100 bytes.
// Byte layout (abi.encodePacked), matching TeeDisputeGame.sol:
//
//	[0:32]   l2SeqNum  — uint256 big-endian (uint64 value in last 8 bytes, first 24 bytes zero-padded)
//	[32:36]  parentIdx — uint32 big-endian
//	[36:68]  blockHash — bytes32
//	[68:100] stateHash — bytes32
func encodeTeeRollupExtraData(d *TeeRollupProposalData) []byte {
	// For xlayer: l2SeqNum encoded as uint256 (32 bytes, big-endian), parentIdx uint32 (4 bytes),
	// blockHash bytes32 (32 bytes), stateHash bytes32 (32 bytes) = 100 bytes total.
	var buf [100]byte
	binary.BigEndian.PutUint64(buf[24:32], d.L2SeqNum) // uint256, value in last 8 bytes
	binary.BigEndian.PutUint32(buf[32:36], d.ParentIdx)
	copy(buf[36:68], d.BlockHash.Bytes())
	copy(buf[68:100], d.StateHash.Bytes())
	return buf[:]
}

type LegacyProposalData struct {
	HeadL1      eth.L1BlockRef
	SafeL2      eth.L2BlockRef
	FinalizedL2 eth.L2BlockRef

	// Support legacy metrics when possible
	BlockRef eth.L2BlockRef
}

type ProposalSource interface {
	ProposalAtSequenceNum(ctx context.Context, seqNum uint64) (Proposal, error)
	SyncStatus(ctx context.Context) (SyncStatus, error)

	// Close closes the underlying client or clients
	Close()
}

type SyncStatus struct {
	CurrentL1   eth.BlockID
	SafeL2      uint64
	FinalizedL2 uint64
}
