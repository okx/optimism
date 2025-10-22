package broadcaster

import (
	"context"
	"fmt"
	"math/big"
	"sync"
	"time"

	"github.com/holiman/uint256"

	"github.com/ethereum-optimism/optimism/op-service/eth"

	"github.com/ethereum-optimism/optimism/op-chain-ops/script"
	opcrypto "github.com/ethereum-optimism/optimism/op-service/crypto"
	"github.com/ethereum-optimism/optimism/op-service/txmgr"
	"github.com/ethereum-optimism/optimism/op-service/txmgr/metrics"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core"
	"github.com/ethereum/go-ethereum/ethclient"
	"github.com/ethereum/go-ethereum/log"
	"github.com/hashicorp/go-multierror"
)

const (
	GasPadFactor = 1.2 // ⚠️[X Layer] Conservative value to stay within RPC gas limits (< 16M per tx)
)

type KeyedBroadcaster struct {
	lgr    log.Logger
	mgr    txmgr.TxManager
	bcasts []script.Broadcast
	client *ethclient.Client
	mtx    sync.Mutex
}

type KeyedBroadcasterOpts struct {
	Logger  log.Logger
	ChainID *big.Int
	Client  *ethclient.Client
	Signer  opcrypto.SignerFn
	From    common.Address
}

func NewKeyedBroadcaster(cfg KeyedBroadcasterOpts) (*KeyedBroadcaster, error) {
	cfg.Logger.Info("⚠️[X Layer] Initializing KeyedBroadcaster with mainnet-optimized config")
	cfg.Logger.Info("⚠️[X Layer] Timeout config",
		"TxSendTimeout", "10m (was 5m)",
		"NetworkTimeout", "30s (was 10s)",
		"TxNotInMempoolTimeout", "2m (was 1m)",
		"ReceiptQueryInterval", "2s (was 1s)")
	cfg.Logger.Info("⚠️[X Layer] Gas config",
		"GasPadFactor", "1.2 (conservative for RPC limits)",
		"FeeLimitMultiplier", "10 (was 5)")

	mgrCfg := &txmgr.Config{
		Backend:                   cfg.Client,
		ChainID:                   cfg.ChainID,
		TxSendTimeout:             10 * time.Minute,
		TxNotInMempoolTimeout:     2 * time.Minute,
		NetworkTimeout:            30 * time.Second,
		ReceiptQueryInterval:      2 * time.Second,
		NumConfirmations:          1,
		SafeAbortNonceTooLowCount: 3,
		Signer:                    cfg.Signer,
		From:                      cfg.From,
		GasPriceEstimatorFn:       DeployerGasPriceEstimator,
	}

	minTipCap, err := eth.GweiToWei(1.0)
	if err != nil {
		panic(err)
	}
	minBaseFee, err := eth.GweiToWei(1.0)
	if err != nil {
		panic(err)
	}

	mgrCfg.RebroadcastInterval.Store(int64(12 * time.Second))
	mgrCfg.ResubmissionTimeout.Store(int64(48 * time.Second))
	mgrCfg.FeeLimitMultiplier.Store(10)
	mgrCfg.FeeLimitThreshold.Store(big.NewInt(100))
	mgrCfg.MinTipCap.Store(minTipCap)
	mgrCfg.MinBaseFee.Store(minBaseFee)

	mgr, err := txmgr.NewSimpleTxManagerFromConfig(
		"transactor",
		cfg.Logger,
		&metrics.NoopTxMetrics{},
		mgrCfg,
	)
	if err != nil {
		return nil, fmt.Errorf("failed to create tx manager: %w", err)
	}

	return &KeyedBroadcaster{
		lgr:    cfg.Logger,
		mgr:    mgr,
		client: cfg.Client,
	}, nil
}

func (t *KeyedBroadcaster) Hook(bcast script.Broadcast) {
	if bcast.Type != script.BroadcastCreate2 && bcast.From != t.mgr.From() {
		panic(fmt.Sprintf("invalid from for broadcast:%v, expected:%v", bcast.From, t.mgr.From()))
	}
	t.mtx.Lock()
	t.bcasts = append(t.bcasts, bcast)
	t.mtx.Unlock()
}

func (t *KeyedBroadcaster) Broadcast(ctx context.Context) ([]BroadcastResult, error) {
	// Empty the internal broadcast buffer as soon as this method is called.
	t.mtx.Lock()
	bcasts := t.bcasts
	t.bcasts = nil
	t.mtx.Unlock()

	if len(bcasts) == 0 {
		return nil, nil
	}

	results := make([]BroadcastResult, len(bcasts))
	futures := make([]<-chan txmgr.SendResponse, len(bcasts))
	ids := make([]common.Hash, len(bcasts))

	latestBlock, err := t.client.BlockByNumber(ctx, nil)
	if err != nil {
		return nil, fmt.Errorf("failed to get latest block: %w", err)
	}

	for i, bcast := range bcasts {
		futures[i], ids[i] = t.broadcast(ctx, bcast, latestBlock.GasLimit())
		t.lgr.Info(
			"transaction broadcasted",
			"id", ids[i],
			"nonce", bcast.Nonce,
		)
		t.lgr.Info("⚠️[X Layer] Gas limit calculation",
			"blockGasLimit", latestBlock.GasLimit(),
			"gasPadFactor", GasPadFactor,
			"note", "Using 1.2x padding (conservative for RPC limits)")
	}

	var txErr *multierror.Error
	var completed int
	for i, fut := range futures {
		bcastRes := <-fut
		completed++
		outRes := BroadcastResult{
			Broadcast: bcasts[i],
		}

		if bcastRes.Err == nil {
			outRes.Receipt = bcastRes.Receipt
			outRes.TxHash = bcastRes.Receipt.TxHash

			if bcastRes.Receipt.Status == 0 {
				failErr := fmt.Errorf("transaction failed: %s", outRes.Receipt.TxHash.String())
				txErr = multierror.Append(txErr, failErr)
				outRes.Err = failErr
				t.lgr.Error(
					"transaction failed on chain",
					"id", ids[i],
					"completed", completed,
					"total", len(bcasts),
					"hash", outRes.Receipt.TxHash.String(),
					"nonce", outRes.Broadcast.Nonce,
				)
			} else {
				t.lgr.Info(
					"transaction confirmed",
					"id", ids[i],
					"completed", completed,
					"total", len(bcasts),
					"hash", outRes.Receipt.TxHash.String(),
					"nonce", outRes.Broadcast.Nonce,
					"creation", outRes.Receipt.ContractAddress,
				)
			}
		} else {
			txErr = multierror.Append(txErr, bcastRes.Err)
			outRes.Err = bcastRes.Err
			t.lgr.Error(
				"transaction failed",
				"id", ids[i],
				"completed", completed,
				"total", len(bcasts),
				"err", bcastRes.Err,
			)
		}

		results[i] = outRes
	}
	return results, txErr.ErrorOrNil()
}

func (t *KeyedBroadcaster) broadcast(ctx context.Context, bcast script.Broadcast, blockGasLimit uint64) (<-chan txmgr.SendResponse, common.Hash) {
	ch := make(chan txmgr.SendResponse, 1)

	id := bcast.ID()
	candidate := asTxCandidate(bcast, blockGasLimit)
	t.mgr.SendAsync(ctx, candidate, ch)
	return ch, id
}

func asTxCandidate(bcast script.Broadcast, blockGasLimit uint64) txmgr.TxCandidate {
	value := ((*uint256.Int)(bcast.Value)).ToBig()
	var candidate txmgr.TxCandidate
	switch bcast.Type {
	case script.BroadcastCall:
		to := &bcast.To
		candidate = txmgr.TxCandidate{
			TxData:   bcast.Input,
			To:       to,
			Value:    value,
			GasLimit: padGasLimit(bcast.Input, bcast.GasUsed, false, blockGasLimit),
		}
	case script.BroadcastCreate:
		candidate = txmgr.TxCandidate{
			TxData:   bcast.Input,
			To:       nil,
			GasLimit: padGasLimit(bcast.Input, bcast.GasUsed, true, blockGasLimit),
		}
	case script.BroadcastCreate2:
		txData := make([]byte, len(bcast.Salt)+len(bcast.Input))
		copy(txData, bcast.Salt[:])
		copy(txData[len(bcast.Salt):], bcast.Input)

		candidate = txmgr.TxCandidate{
			TxData:   txData,
			To:       &script.DeterministicDeployerAddress,
			Value:    value,
			GasLimit: padGasLimit(bcast.Input, bcast.GasUsed, true, blockGasLimit),
		}
	default:
		panic(fmt.Sprintf("unrecognized broadcast type: '%s'", bcast.Type))
	}
	return candidate
}

// padGasLimit calculates the gas limit for a transaction based on the intrinsic gas and the gas used by
// the underlying call. Values are multiplied by a pad factor to account for any discrepancies. The output
// is clamped to the block gas limit since Geth will reject transactions that exceed it before letting them
// into the mempool.
// ⚠️[X Layer] Using GasPadFactor=1.2 (conservative to avoid RPC "gas limit too high" errors)
func padGasLimit(data []byte, gasUsed uint64, creation bool, blockGasLimit uint64) uint64 {
	intrinsicGas, err := core.IntrinsicGas(data, nil, nil, creation, true, true, false)
	// This method never errors - we should look into it if it does.
	if err != nil {
		panic(err)
	}

	floorDataGas, err := core.FloorDataGas(data)
	// We should never cause an overflow here.
	if err != nil {
		panic(err)
	}

	gas := intrinsicGas + gasUsed
	if floorDataGas > gas {
		gas = floorDataGas
	}

	limit := uint64(float64(gas) * GasPadFactor)
	if limit > blockGasLimit {
		// Note: Can't log here as padGasLimit doesn't have logger access
		// The calling function will log the final gas limit
		return blockGasLimit
	}
	return limit
}
