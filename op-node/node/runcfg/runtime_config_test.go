package runcfg

import (
	"sync"
	"testing"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/log"
	"github.com/stretchr/testify/require"
)

func TestSetP2PSequencerAddress(t *testing.T) {
	runCfg := NewRuntimeConfig(log.New(), nil, nil)

	// Initially zero
	require.Equal(t, common.Address{}, runCfg.P2PSequencerAddress())

	// Set an address
	addr := common.HexToAddress("0x1234567890abcdef1234567890abcdef12345678")
	runCfg.SetP2PSequencerAddress(addr)
	require.Equal(t, addr, runCfg.P2PSequencerAddress())

	// Update to a different address
	addr2 := common.HexToAddress("0xabcdefabcdefabcdefabcdefabcdefabcdefabcd")
	runCfg.SetP2PSequencerAddress(addr2)
	require.Equal(t, addr2, runCfg.P2PSequencerAddress())
}

func TestSetP2PSequencerAddress_ConcurrentSafety(t *testing.T) {
	runCfg := NewRuntimeConfig(log.New(), nil, nil)
	addr := common.HexToAddress("0x1234567890abcdef1234567890abcdef12345678")

	var wg sync.WaitGroup
	// Concurrent writers
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			runCfg.SetP2PSequencerAddress(addr)
		}()
	}
	// Concurrent readers
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_ = runCfg.P2PSequencerAddress()
		}()
	}
	wg.Wait()

	require.Equal(t, addr, runCfg.P2PSequencerAddress())
}
