package sequencing

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"math/big"

	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/log"
	realtimeKafka "github.com/ethereum/go-ethereum/realtime/kafka"
	realtimeTypes "github.com/ethereum/go-ethereum/realtime/types"
	"github.com/ethereum/go-ethereum/trie"
	"github.com/holiman/uint256"
)

type rawTransactions []eth.Data

func (s rawTransactions) Len() int { return len(s) }
func (s rawTransactions) EncodeIndex(i int, w *bytes.Buffer) {
	w.Write(s[i])
}

func (s *Sequencer) InitRealtimeXLayer() {
	if s.rollupCfg.Realtime.Enable {
		kafkaProducer, err := realtimeKafka.NewKafkaProducer(s.rollupCfg.Realtime.Kafka, context.Background(), nil)
		if err != nil {
			kafkaProducer = nil
			log.Warn("[Realtime] Failed to initialize kafka producer", "error", err)
		}
		s.realtimeProducer = kafkaProducer
		s.realtimeBlockInfoChan = make(chan *realtimeTypes.BlockInfo, realtimeKafka.DefaultKafkaBufferSize)
	}
}

func (s *Sequencer) StartRealtimeXLayer() error {
	isLeader, err := s.conductor.Leader(s.ctx)
	if err != nil {
		return fmt.Errorf("sequencer leader check failed: %w", err)
	} else if !isLeader {
		return errors.New("sequencer is not the leader, aborting")
	}
	s.isLeader = isLeader
	s.SendRealtimeErrorTrigger(0)
	return nil
}

func (s *Sequencer) SendRealtimeErrorTrigger(height uint64) {
	if s.isLeader && s.rollupCfg != nil && s.rollupCfg.Realtime != nil && s.rollupCfg.Realtime.Enable {
		if err := s.realtimeProducer.SendKafkaErrorTrigger(height); err != nil {
			log.Error(fmt.Sprintf("[Realtime] Failed to send kafka error trigger message. error: %v", err))
		}
	}
}

func (s *Sequencer) SendRealtimeConfirmedBlock(envelope *eth.ExecutionPayloadEnvelope) {
	if s.isLeader && s.rollupCfg != nil && s.rollupCfg.Realtime != nil && s.rollupCfg.Realtime.Enable {
		if s.realtimeBlockInfoChan != nil {
			payload := envelope.ExecutionPayload
			hasher := trie.NewStackTrie(nil)
			txHash := types.DeriveSha(rawTransactions(payload.Transactions), hasher)
			s.realtimeBlockInfoChan <- &realtimeTypes.BlockInfo{
				Header: &types.Header{
					ParentHash:       payload.ParentHash,
					UncleHash:        types.EmptyUncleHash,
					Coinbase:         payload.FeeRecipient,
					Root:             common.Hash(payload.StateRoot),
					TxHash:           txHash,
					ReceiptHash:      common.Hash(payload.ReceiptsRoot),
					Bloom:            types.Bloom(payload.LogsBloom),
					Difficulty:       common.Big0, // zeroed, proof-of-work legacy
					Number:           big.NewInt(int64(payload.BlockNumber)),
					GasLimit:         uint64(payload.GasLimit),
					GasUsed:          uint64(payload.GasUsed),
					Time:             uint64(payload.Timestamp),
					Extra:            payload.ExtraData,
					MixDigest:        common.Hash(payload.PrevRandao),
					Nonce:            types.BlockNonce{}, // zeroed, proof-of-work legacy
					BaseFee:          (*uint256.Int)(&payload.BaseFeePerGas).ToBig(),
					WithdrawalsHash:  nil, // set below
					BlobGasUsed:      (*uint64)(payload.BlobGasUsed),
					ExcessBlobGas:    (*uint64)(payload.ExcessBlobGas),
					ParentBeaconRoot: envelope.ParentBeaconBlockRoot,
				},
				TxCount:   int64(len(envelope.ExecutionPayload.Transactions)),
				Hash:      envelope.ExecutionPayload.BlockHash,
				Changeset: envelope.ExecutionPayload.Changeset,
			}
		}
	}
}

func (s *Sequencer) SetRealtimeEnabledXLayer(attrs *eth.PayloadAttributes) {
	if s.isLeader && s.rollupCfg != nil && s.rollupCfg.Realtime != nil {
		attrs.RealtimeEnabled = s.rollupCfg.Realtime.Enable
	}
}
