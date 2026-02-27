package rollup

import "math/big"

const (
	// MainnetL2TimeForkTime is the absolute timestamp when the fixed timestamp is used to derive L1 span batches.
	// After this time, the fixed timestamp is used. Corresponds to 2025-12-08 14:00:00 UTC+8.
	MainnetL2TimeForkTime = 1765173600
	// TestnetL2TimeForkTime is the absolute timestamp when the fixed timestamp is used to derive L1 span batches.
	// After this time, the fixed timestamp is used. Corresponds to 2025-12-04 18:00:00 UTC+8.
	TestnetL2TimeForkTime = 1764842400
)

const (
	// MainnetOldL2Time is the wrong time from rollup.json
	MainnetOldL2Time = 1761567143
	// MainnetFixedL2Time is the correct time from genesis.json and will be written to new rollup.json
	MainnetFixedL2Time = 1761579057
	// TestnetOldL2Time is the wrong time from rollup.json
	TestnetOldL2Time = 1760699568
	// TestnetFixedL2Time is the correct time from genesis.json and will be written to new rollup.json
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
