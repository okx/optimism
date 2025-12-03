package rollup

import "math/big"

const (
	// MainnetL2TimeForkTime is the absolute timestamp when the fixed timestamp is used to derive L1 span batches.
	// After this time, the fixed timestamp is used. Corresponds to 2025-12-08 08:00:00 UTC.
	MainnetL2TimeForkTime = 1765152000
	// TestnetL2TimeForkTime is the absolute timestamp when the fixed timestamp is used to derive L1 span batches.
	// After this time, the fixed timestamp is used. Corresponds to 2025-12-04 12:00:00 UTC.
	TestnetL2TimeForkTime = 1764820800
)

const (
	MainnetOldL2Time   = 1761567143
	MainnetFixedL2Time = 1761579057
	TestnetOldL2Time   = 1760699568
	TestnetFixedL2Time = 1760700537
)

func isXLayerMainnet(chainID *big.Int) bool {
	return chainID != nil && chainID.Uint64() == XLayerMainnetChainID
}

func isXLayerTestnet(chainID *big.Int) bool {
	return chainID != nil && chainID.Uint64() == XLayerTestnetChainID
}

func GetBatchStartTime(genesisTimestamp, relTimestamp uint64, chainID *big.Int) uint64 {
	batchStartTime := genesisTimestamp + relTimestamp
	if isXLayerMainnet(chainID) {
		if batchStartTime < MainnetL2TimeForkTime {
			return MainnetOldL2Time + relTimestamp
		} else {
			return MainnetFixedL2Time + relTimestamp
		}
	} else if isXLayerTestnet(chainID) {
		if batchStartTime < TestnetL2TimeForkTime {
			return TestnetOldL2Time + relTimestamp
		} else {
			return TestnetFixedL2Time + relTimestamp
		}
	}

	return batchStartTime
}
