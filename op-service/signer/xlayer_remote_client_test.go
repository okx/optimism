package signer

import (
	"math"
	"testing"

	"github.com/ethereum/go-ethereum/common/hexutil"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/log"
	"github.com/stretchr/testify/require"

	"github.com/ethereum-optimism/optimism/op-service/testlog"
)

// word returns a 32-byte big-endian (right-aligned) ABI word for v.
func word(v uint64) []byte {
	b := make([]byte, 32)
	for i := 0; i < 8; i++ {
		b[31-i] = byte(v >> (8 * i))
	}
	return b
}

// maxWord returns a 32-byte word larger than uint64 (all 0xff).
func maxWord() []byte {
	b := make([]byte, 32)
	for i := range b {
		b[i] = 0xff
	}
	return b
}

// dgfCreateTx builds a DisputeGameFactory.create transaction whose calldata is
// the method selector followed by the given concatenated methodData bytes.
func dgfCreateTx(methodData []byte) *types.Transaction {
	sig := hexutil.MustDecode(MethodSigDGFCreate)
	data := append(append([]byte{}, sig...), methodData...)
	return types.NewTx(&types.LegacyTx{Data: data})
}

func TestUnpackProposerTransaction_ExtraData(t *testing.T) {
	c := &XLayerRemoteClient{logger: testlog.Logger(t, log.LevelCrit)}

	gameType := word(7)         // parsed from methodData[28:32] => 7
	rootClaim := word(0x1234)   // methodData[32:64]
	canonicalOffset := word(96) // points at the length word right after the 3 head words

	t.Run("valid", func(t *testing.T) {
		extra := []byte{0xde, 0xad, 0xbe, 0xef}
		md := concat(gameType, rootClaim, canonicalOffset, word(uint64(len(extra))), extra)
		args, err := c.unpackProposerTransaction(dgfCreateTx(md))
		require.NoError(t, err)
		require.Equal(t, uint32(7), args.GameType)
		require.Equal(t, extra, args.ExtraData)
	})

	t.Run("offset exceeds uint64", func(t *testing.T) {
		md := concat(gameType, rootClaim, maxWord()) // 96 bytes, offset word > uint64
		_, err := c.unpackProposerTransaction(dgfCreateTx(md))
		require.ErrorContains(t, err, "offset: exceeds uint64")
	})

	t.Run("offset out of range (would overflow offset+32)", func(t *testing.T) {
		// MaxUint64-10 fits uint64 but is far past the data; the old additive
		// check offset+32 would wrap around and could pass.
		md := concat(gameType, rootClaim, word(math.MaxUint64-10))
		_, err := c.unpackProposerTransaction(dgfCreateTx(md))
		require.ErrorContains(t, err, "invalid extraData offset")
	})

	t.Run("length exceeds uint64", func(t *testing.T) {
		md := concat(gameType, rootClaim, canonicalOffset, maxWord()) // length word > uint64
		_, err := c.unpackProposerTransaction(dgfCreateTx(md))
		require.ErrorContains(t, err, "length: exceeds uint64")
	})

	t.Run("length out of range (would overflow offset+32+length)", func(t *testing.T) {
		// offset=96, methodData=128 (no trailing data). length=MaxUint64-50 fits
		// uint64; the old additive check offset+32+length would wrap and pass,
		// then make([]byte, huge) would OOM. New check rejects it.
		md := concat(gameType, rootClaim, canonicalOffset, word(math.MaxUint64-50))
		_, err := c.unpackProposerTransaction(dgfCreateTx(md))
		require.ErrorContains(t, err, "invalid extraData length")
	})

	t.Run("method data too short", func(t *testing.T) {
		md := concat(gameType, rootClaim) // 64 bytes < 96
		_, err := c.unpackProposerTransaction(dgfCreateTx(md))
		require.ErrorContains(t, err, "method data too short")
	})
}

func concat(parts ...[]byte) []byte {
	var out []byte
	for _, p := range parts {
		out = append(out, p...)
	}
	return out
}
