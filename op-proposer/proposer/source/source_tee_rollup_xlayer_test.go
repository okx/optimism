// For xlayer: Unit tests for TeeRollup HTTP client and proposal source (TEE game type 1960).
package source

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/ethereum-optimism/optimism/op-service/testlog"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/crypto"
	"github.com/ethereum/go-ethereum/log"
	"github.com/stretchr/testify/require"
)

// helper to create a test HTTP server with a given JSON response
func newTeeRollupServer(body string, statusCode int) *httptest.Server {
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(statusCode)
		fmt.Fprint(w, body)
	}))
}

func TestConfirmedBlockInfo_Success(t *testing.T) {
	height := uint64(42)
	blockHash := "0x1111111111111111111111111111111111111111111111111111111111111111"
	appHash := "0x2222222222222222222222222222222222222222222222222222222222222222"
	body, _ := json.Marshal(map[string]interface{}{
		"code":    0,
		"message": "ok",
		"data": map[string]interface{}{
			"height":    height,
			"blockHash": blockHash,
			"appHash":   appHash,
		},
	})
	srv := newTeeRollupServer(string(body), 200)
	defer srv.Close()

	cl, err := NewTeeRollupHTTPClient(srv.URL)
	require.NoError(t, err)

	info, err := cl.ConfirmedBlockInfo(context.Background())
	require.NoError(t, err)
	require.Equal(t, height, info.Height)
	require.Equal(t, common.HexToHash(blockHash), info.BlockHash)
	require.Equal(t, common.HexToHash(appHash), info.AppHash)

	// verify LRU cache was populated
	cached, ok := cl.cache.Get(height)
	require.True(t, ok)
	require.Equal(t, info, cached)
}

func TestConfirmedBlockInfo_ErrorCode(t *testing.T) {
	body, _ := json.Marshal(map[string]interface{}{
		"code":    500,
		"message": "internal error",
		"data":    nil,
	})
	srv := newTeeRollupServer(string(body), 200)
	defer srv.Close()

	cl, err := NewTeeRollupHTTPClient(srv.URL)
	require.NoError(t, err)

	_, err = cl.ConfirmedBlockInfo(context.Background())
	require.ErrorContains(t, err, "RPC error code=500")
}

func TestConfirmedBlockInfo_NullData(t *testing.T) {
	body, _ := json.Marshal(map[string]interface{}{
		"code":    0,
		"message": "ok",
		"data":    nil,
	})
	srv := newTeeRollupServer(string(body), 200)
	defer srv.Close()

	cl, err := NewTeeRollupHTTPClient(srv.URL)
	require.NoError(t, err)

	_, err = cl.ConfirmedBlockInfo(context.Background())
	require.ErrorContains(t, err, "data is null")
}

func TestConfirmedBlockInfo_NullFields(t *testing.T) {
	body, _ := json.Marshal(map[string]interface{}{
		"code":    0,
		"message": "ok",
		"data": map[string]interface{}{
			"height":    nil,
			"blockHash": nil,
			"appHash":   nil,
		},
	})
	srv := newTeeRollupServer(string(body), 200)
	defer srv.Close()

	cl, err := NewTeeRollupHTTPClient(srv.URL)
	require.NoError(t, err)

	_, err = cl.ConfirmedBlockInfo(context.Background())
	require.ErrorContains(t, err, "null fields")
}

func TestConfirmedBlockInfoAtHeight_CacheHit(t *testing.T) {
	// Server that returns 500 if called (cache should prevent HTTP request)
	callCount := 0
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		callCount++
		w.WriteHeader(500)
	}))
	defer srv.Close()

	cl, err := NewTeeRollupHTTPClient(srv.URL)
	require.NoError(t, err)

	// Pre-populate cache
	expectedInfo := TeeRollupBlockInfo{Height: 100, BlockHash: common.HexToHash("0x1234"), AppHash: common.HexToHash("0x5678")}
	cl.cache.Add(uint64(100), expectedInfo)

	info, err := cl.ConfirmedBlockInfoAtHeight(context.Background(), 100)
	require.NoError(t, err)
	require.Equal(t, expectedInfo, info)
	require.Equal(t, 0, callCount, "HTTP should not be called on cache hit")
}

func TestConfirmedBlockInfoAtHeight_HeightMismatch(t *testing.T) {
	height := uint64(99) // server returns 99, but we request 100
	blockHash := "0x1111111111111111111111111111111111111111111111111111111111111111"
	appHash := "0x2222222222222222222222222222222222222222222222222222222222222222"
	body, _ := json.Marshal(map[string]interface{}{
		"code":    0,
		"message": "ok",
		"data": map[string]interface{}{
			"height":    height,
			"blockHash": blockHash,
			"appHash":   appHash,
		},
	})
	srv := newTeeRollupServer(string(body), 200)
	defer srv.Close()

	cl, err := NewTeeRollupHTTPClient(srv.URL)
	require.NoError(t, err)

	_, err = cl.ConfirmedBlockInfoAtHeight(context.Background(), 100)
	require.ErrorContains(t, err, "height mismatch")
}

func TestComputeRootClaim(t *testing.T) {
	blockHash := common.HexToHash("0x1111111111111111111111111111111111111111111111111111111111111111")
	stateHash := common.HexToHash("0x2222222222222222222222222222222222222222222222222222222222222222")

	got := computeRootClaim(blockHash, stateHash)

	expected := crypto.Keccak256Hash(append(blockHash.Bytes(), stateHash.Bytes()...))
	require.Equal(t, expected, got)
}

func TestEncodeTeeRollupExtraData(t *testing.T) {
	d := &TeeRollupProposalData{
		L2SeqNum:  0x0102030405060708,
		ParentIdx: 0x090A0B0C,
		BlockHash: common.HexToHash("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
		StateHash: common.HexToHash("0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
	}
	buf := encodeTeeRollupExtraData(d)
	// For xlayer: new layout is 100 bytes (l2SeqNum as uint256)
	require.Equal(t, 100, len(buf))

	// [0:24] high bytes of uint256 l2SeqNum — must be zero
	require.Equal(t, make([]byte, 24), buf[0:24])
	// [24:32] low 8 bytes of uint256 l2SeqNum — uint64 big-endian
	require.Equal(t, []byte{0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08}, buf[24:32])
	// [32:36] parentIdx big-endian
	require.Equal(t, []byte{0x09, 0x0A, 0x0B, 0x0C}, buf[32:36])
	// [36:68] blockHash
	require.Equal(t, d.BlockHash.Bytes(), buf[36:68])
	// [68:100] stateHash
	require.Equal(t, d.StateHash.Bytes(), buf[68:100])
}

// mockTeeRollupClient is a test double for TeeRollupClient
type mockTeeRollupClient struct {
	info TeeRollupBlockInfo
	err  error
}

func (m *mockTeeRollupClient) ConfirmedBlockInfo(ctx context.Context) (TeeRollupBlockInfo, error) {
	return m.info, m.err
}
func (m *mockTeeRollupClient) ConfirmedBlockInfoAtHeight(ctx context.Context, height uint64) (TeeRollupBlockInfo, error) {
	if m.err != nil {
		return TeeRollupBlockInfo{}, m.err
	}
	if m.info.Height != height {
		return TeeRollupBlockInfo{}, fmt.Errorf("tee-rollup: confirmed block height mismatch: expected %d, got %d", height, m.info.Height)
	}
	return m.info, nil
}
func (m *mockTeeRollupClient) Close() {}

func TestSyncStatus_UsesLowestHeight(t *testing.T) {
	logger := testlog.Logger(t, log.LevelInfo)
	cl1 := &mockTeeRollupClient{info: TeeRollupBlockInfo{Height: 100}}
	cl2 := &mockTeeRollupClient{info: TeeRollupBlockInfo{Height: 50}}
	cl3 := &mockTeeRollupClient{info: TeeRollupBlockInfo{Height: 80}}

	src := NewTeeRollupProposalSource(logger, cl1, cl2, cl3)
	status, err := src.SyncStatus(context.Background())
	require.NoError(t, err)
	require.Equal(t, uint64(50), status.SafeL2)
	require.Equal(t, uint64(50), status.FinalizedL2)
}

func TestSyncStatus_AllFail(t *testing.T) {
	logger := testlog.Logger(t, log.LevelInfo)
	cl1 := &mockTeeRollupClient{err: errors.New("err1")}
	cl2 := &mockTeeRollupClient{err: errors.New("err2")}

	src := NewTeeRollupProposalSource(logger, cl1, cl2)
	_, err := src.SyncStatus(context.Background())
	require.Error(t, err)
	require.ErrorContains(t, err, "err1")
	require.ErrorContains(t, err, "err2")
}

func TestProposalAtSequenceNum_Success(t *testing.T) {
	logger := testlog.Logger(t, log.LevelInfo)
	blockHash := common.HexToHash("0xaaaa")
	appHash := common.HexToHash("0xbbbb")
	cl := &mockTeeRollupClient{info: TeeRollupBlockInfo{Height: 42, BlockHash: blockHash, AppHash: appHash}}

	src := NewTeeRollupProposalSource(logger, cl)
	proposal, err := src.ProposalAtSequenceNum(context.Background(), 42)
	require.NoError(t, err)

	require.Equal(t, uint64(42), proposal.SequenceNum)
	require.NotNil(t, proposal.TeeRollupData)
	require.Equal(t, uint64(42), proposal.TeeRollupData.L2SeqNum)
	require.Equal(t, blockHash, proposal.TeeRollupData.BlockHash)
	require.Equal(t, appHash, proposal.TeeRollupData.StateHash)

	expectedRoot := computeRootClaim(blockHash, appHash)
	require.Equal(t, expectedRoot, proposal.Root)
}

func TestProposalAtSequenceNum_HeightMismatch(t *testing.T) {
	logger := testlog.Logger(t, log.LevelInfo)
	cl := &mockTeeRollupClient{info: TeeRollupBlockInfo{Height: 99}}

	src := NewTeeRollupProposalSource(logger, cl)
	_, err := src.ProposalAtSequenceNum(context.Background(), 100)
	require.ErrorContains(t, err, "all clients failed")
}

func TestProposalAtSequenceNum_ParentIdxFn_GuardBlocks(t *testing.T) {
	// seqNum == parentL2SeqNum → error, guard triggers
	client := &mockTeeRollupClient{info: TeeRollupBlockInfo{Height: 100, BlockHash: common.Hash{1}, AppHash: common.Hash{2}}}
	src := NewTeeRollupProposalSource(testlog.Logger(t, log.LevelError), client)
	src.SetParentIdxFn(func(ctx context.Context) (uint32, uint64, bool, error) {
		return 5, 100, true, nil // parentL2SeqNum == seqNum == 100
	})
	_, err := src.ProposalAtSequenceNum(context.Background(), 100)
	require.Error(t, err)
	require.Contains(t, err.Error(), "skipping duplicate proposal")
}

func TestProposalAtSequenceNum_ParentIdxFn_GuardPasses(t *testing.T) {
	// seqNum > parentL2SeqNum → success, parentIdx used
	client := &mockTeeRollupClient{info: TeeRollupBlockInfo{Height: 100, BlockHash: common.Hash{1}, AppHash: common.Hash{2}}}
	src := NewTeeRollupProposalSource(testlog.Logger(t, log.LevelError), client)
	src.SetParentIdxFn(func(ctx context.Context) (uint32, uint64, bool, error) {
		return 7, 99, true, nil // parentL2SeqNum=99 < seqNum=100
	})
	proposal, err := src.ProposalAtSequenceNum(context.Background(), 100)
	require.NoError(t, err)
	require.Equal(t, uint32(7), proposal.TeeRollupData.ParentIdx)
}

func TestProposalAtSequenceNum_ParentIdxFn_Error(t *testing.T) {
	// parentIdxFn returns error → falls back to anchor sentinel (MaxUint32)
	client := &mockTeeRollupClient{info: TeeRollupBlockInfo{Height: 100, BlockHash: common.Hash{1}, AppHash: common.Hash{2}}}
	src := NewTeeRollupProposalSource(testlog.Logger(t, log.LevelError), client)
	src.SetParentIdxFn(func(ctx context.Context) (uint32, uint64, bool, error) {
		return 0, 0, false, fmt.Errorf("rpc error")
	})
	proposal, err := src.ProposalAtSequenceNum(context.Background(), 100)
	require.NoError(t, err)
	require.Equal(t, uint32(math.MaxUint32), proposal.TeeRollupData.ParentIdx)
}
