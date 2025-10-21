package sequencing

import (
	"context"
	"fmt"
	"math/big"

	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum/go-ethereum/beacon/engine"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/log"
	"github.com/ethereum/go-ethereum/realtime"
	realtimeKafka "github.com/ethereum/go-ethereum/realtime/kafka"
	realtimeTypes "github.com/ethereum/go-ethereum/realtime/types"
	"github.com/ethereum/go-ethereum/trie"
	"github.com/holiman/uint256"
)

func (s *Sequencer) InitRealtimeXLayer() {
	if s.rollupCfg.Realtime.SequencerEnable {
		kafkaProducer, err := realtimeKafka.NewKafkaProducer(s.rollupCfg.Realtime.Kafka, context.Background(), nil)
		if err != nil {
			kafkaProducer = nil
			log.Warn("[Realtime] Failed to initialize kafka producer", "error", err)
		}

		if kafkaProducer != nil {
			s.realtimeProducer = kafkaProducer
			s.realtimeBlock = common.Hash{}
			s.realtimeBlockInfoChan = make(chan *realtimeTypes.BlockInfo, realtimeKafka.DefaultKafkaBufferSize)

			// start realtime producer loop
			go realtime.ListenRealtimeProducer(s.ctx, s.realtimeProducer, s.realtimeBlockInfoChan, nil, false)
			log.Info("[Realtime] Realtime initialized on op-node sequencer")
		}
	}
}

func (s *Sequencer) SendRealtimeErrorTrigger(height uint64) {
	if s.active.Load() && s.rollupCfg != nil && s.rollupCfg.Realtime != nil && s.rollupCfg.Realtime.SequencerEnable && s.realtimeProducer != nil {
		if err := s.realtimeProducer.SendKafkaErrorTrigger(height); err != nil {
			log.Error(fmt.Sprintf("[Realtime] Failed to send kafka error trigger message. error: %v", err))
		}
	}
}

func (s *Sequencer) SendRealtimeConfirmedBlock(envelope *eth.ExecutionPayloadEnvelope) {
	if s.active.Load() && s.rollupCfg != nil && s.rollupCfg.Realtime != nil && s.rollupCfg.Realtime.SequencerEnable && s.realtimeProducer != nil {
		if envelope.ExecutionPayload.BlockHash == s.realtimeBlock {
			return
		}

		if s.realtimeBlockInfoChan != nil {
			header, err := s.ExecutionPayloadToBlockHeader(envelope)
			if err != nil {
				log.Error("[Realtime] Failed to convert execution payload to block header", "error", err)
				return
			}
			// Calculate transaction hash using the same method as geth
			s.realtimeBlockInfoChan <- &realtimeTypes.BlockInfo{
				Header:      header,
				Withdrawals: envelope.ExecutionPayload.Withdrawals,
				TxCount:     int64(len(envelope.ExecutionPayload.Transactions)),
				Hash:        envelope.ExecutionPayload.BlockHash,
				Changeset:   envelope.Changeset,
			}
			s.realtimeBlock = envelope.ExecutionPayload.BlockHash
		}
	}
}

func (s *Sequencer) SetRealtimeEnabledXLayer(attrs *eth.PayloadAttributes) {
	if s.active.Load() && s.rollupCfg != nil && s.rollupCfg.Realtime != nil {
		attrs.RealtimeEnabled = s.rollupCfg.Realtime.SequencerEnable
	}
}

func (s *Sequencer) ExecutionPayloadToBlockHeader(envelope *eth.ExecutionPayloadEnvelope) (*types.Header, error) {
	payload := envelope.ExecutionPayload
	// For txHash
	txBytes := make([][]byte, len(payload.Transactions))
	for i, tx := range payload.Transactions {
		txBytes[i] = []byte(tx)
	}
	txs, err := engine.DecodeTransactionsXLayer([][]byte(txBytes))
	if err != nil {
		return nil, fmt.Errorf("failed to decode transactions for realtime block: %w", err)
	}
	// For withdrawals hash
	var withdrawalsRoot *common.Hash
	if payload.WithdrawalsRoot != nil {
		withdrawalsRoot = payload.WithdrawalsRoot
	} else if payload.Withdrawals != nil {
		wr := types.DeriveSha(*payload.Withdrawals, trie.NewStackTrie(nil))
		withdrawalsRoot = &wr
	}
	// For requests hash
	var requestsHash *common.Hash
	if s.rollupCfg.NewPayloadVersion(uint64(payload.Timestamp)) == eth.NewPayloadV4 {
		requests := [][]byte{}
		h := types.CalcRequestsHash(requests)
		requestsHash = &h
	}

	header := &types.Header{
		ParentHash:       payload.ParentHash,
		UncleHash:        types.EmptyUncleHash,
		Coinbase:         payload.FeeRecipient,
		Root:             common.Hash(payload.StateRoot),
		TxHash:           types.DeriveSha(types.Transactions(txs), trie.NewStackTrie(nil)),
		ReceiptHash:      common.Hash(payload.ReceiptsRoot),
		Bloom:            types.Bloom(payload.LogsBloom),
		Difficulty:       common.Big0, // zeroed, proof-of-work legacy
		Number:           new(big.Int).SetUint64(uint64(payload.BlockNumber)),
		GasLimit:         uint64(payload.GasLimit),
		GasUsed:          uint64(payload.GasUsed),
		Time:             uint64(payload.Timestamp),
		BaseFee:          (*uint256.Int)(&payload.BaseFeePerGas).ToBig(),
		Extra:            payload.ExtraData,
		MixDigest:        common.Hash(payload.PrevRandao),
		WithdrawalsHash:  withdrawalsRoot,
		ExcessBlobGas:    (*uint64)(payload.ExcessBlobGas),
		BlobGasUsed:      (*uint64)(payload.BlobGasUsed),
		ParentBeaconRoot: envelope.ParentBeaconBlockRoot,
		RequestsHash:     requestsHash,
	}
	if header.Hash() != payload.BlockHash {
		return nil, fmt.Errorf("block hash mismatch: %s != %s", header.Hash(), payload.BlockHash)
	}
	return header, nil
}
