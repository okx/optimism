// For xlayer: TeeRollup HTTP client and proposal source for TEE game type 1960.
package source

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"net/http"
	"sync"
	"time"

	lru "github.com/hashicorp/golang-lru/v2"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/crypto"
	"github.com/ethereum/go-ethereum/log"

	"github.com/ethereum-optimism/optimism/op-service/eth"
)

// TeeRollupBlockInfo holds confirmed block info returned by the TeeRollup RPC.
type TeeRollupBlockInfo struct {
	Height    uint64
	AppHash   common.Hash
	BlockHash common.Hash
}

// internal JSON parsing types (pointer fields to distinguish JSON null)
type teeRollupRawResponse struct {
	Code    int              `json:"code"`
	Message string           `json:"message"`
	Data    *teeRollupData   `json:"data"`
}

type teeRollupData struct {
	Height    *uint64 `json:"height"`
	AppHash   *string `json:"appHash"`
	BlockHash *string `json:"blockHash"`
}

// TeeRollupClient is the interface for the TeeRollup RPC client.
type TeeRollupClient interface {
	ConfirmedBlockInfo(ctx context.Context) (TeeRollupBlockInfo, error)
	ConfirmedBlockInfoAtHeight(ctx context.Context, height uint64) (TeeRollupBlockInfo, error)
	Close()
}

// TeeRollupHTTPClient implements TeeRollupClient using HTTP REST.
type TeeRollupHTTPClient struct {
	baseURL    string
	httpClient *http.Client
	cache      *lru.Cache[uint64, TeeRollupBlockInfo]
}

// NewTeeRollupHTTPClient creates a new TeeRollupHTTPClient.
func NewTeeRollupHTTPClient(baseURL string) (*TeeRollupHTTPClient, error) {
	cache, err := lru.New[uint64, TeeRollupBlockInfo](16)
	if err != nil {
		return nil, fmt.Errorf("tee-rollup: failed to create LRU cache: %w", err)
	}
	return &TeeRollupHTTPClient{
		baseURL: baseURL,
		httpClient: &http.Client{
			Timeout: 10 * time.Second,
		},
		cache: cache,
	}, nil
}

// ConfirmedBlockInfo fetches the latest confirmed block info from TeeRollup RPC.
// GET /v1/chain/confirmed_block_info
func (c *TeeRollupHTTPClient) ConfirmedBlockInfo(ctx context.Context) (TeeRollupBlockInfo, error) {
	url := c.baseURL + "/v1/chain/confirmed_block_info"
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return TeeRollupBlockInfo{}, fmt.Errorf("tee-rollup: failed to create request: %w", err)
	}
	resp, err := c.httpClient.Do(req)
	if err != nil {
		return TeeRollupBlockInfo{}, fmt.Errorf("tee-rollup: HTTP request failed: %w", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return TeeRollupBlockInfo{}, fmt.Errorf("tee-rollup: HTTP request failed with status %d", resp.StatusCode)
	}

	body, err := io.ReadAll(io.LimitReader(resp.Body, 10<<20))
	if err != nil {
		return TeeRollupBlockInfo{}, fmt.Errorf("tee-rollup: failed to read response body: %w", err)
	}

	var raw teeRollupRawResponse
	if err := json.Unmarshal(body, &raw); err != nil {
		return TeeRollupBlockInfo{}, fmt.Errorf("tee-rollup: failed to parse response: %w", err)
	}

	if raw.Code != 0 {
		return TeeRollupBlockInfo{}, fmt.Errorf("tee-rollup: RPC error code=%d message=%s", raw.Code, raw.Message)
	}
	if raw.Data == nil {
		return TeeRollupBlockInfo{}, fmt.Errorf("tee-rollup: no confirmed block available (data is null)")
	}
	if raw.Data.Height == nil || raw.Data.AppHash == nil || raw.Data.BlockHash == nil {
		return TeeRollupBlockInfo{}, fmt.Errorf("tee-rollup: confirmed block info incomplete (null fields)")
	}

	info := TeeRollupBlockInfo{
		Height:    *raw.Data.Height,
		AppHash:   common.HexToHash(*raw.Data.AppHash),
		BlockHash: common.HexToHash(*raw.Data.BlockHash),
	}
	c.cache.Add(info.Height, info)
	return info, nil
}

// ConfirmedBlockInfoAtHeight fetches confirmed block info at a specific height.
// Returns from LRU cache if available; otherwise fetches from RPC and validates exact height match.
func (c *TeeRollupHTTPClient) ConfirmedBlockInfoAtHeight(ctx context.Context, height uint64) (TeeRollupBlockInfo, error) {
	if cached, ok := c.cache.Get(height); ok {
		return cached, nil
	}
	info, err := c.ConfirmedBlockInfo(ctx)
	if err != nil {
		return TeeRollupBlockInfo{}, err
	}
	if info.Height != height {
		return TeeRollupBlockInfo{}, fmt.Errorf("tee-rollup: confirmed block height mismatch: expected %d, got %d", height, info.Height)
	}
	return info, nil
}

// Close is a no-op (satisfies TeeRollupClient interface).
func (c *TeeRollupHTTPClient) Close() {}

// TeeRollupProposalSource implements ProposalSource for TeeRollup TEE game type 1960.
type TeeRollupProposalSource struct {
	log         log.Logger
	clients     []TeeRollupClient
	parentIdxFn func(ctx context.Context) (uint32, bool, error) // resolves parent DGF game index
}

// NewTeeRollupProposalSource creates a new TeeRollupProposalSource.
func NewTeeRollupProposalSource(log log.Logger, clients ...TeeRollupClient) *TeeRollupProposalSource {
	if len(clients) == 0 {
		panic("no TeeRollup clients provided")
	}
	return &TeeRollupProposalSource{
		log:     log,
		clients: clients,
	}
}

// SetParentIdxFn injects the callback that resolves the parent DGF game index.
// MUST be called before Start() to satisfy Go's happens-before guarantee.
// If nil (default), ProposalAtSequenceNum always uses math.MaxUint32 (anchor state sentinel).
func (s *TeeRollupProposalSource) SetParentIdxFn(fn func(ctx context.Context) (uint32, bool, error)) {
	s.parentIdxFn = fn
}

// SyncStatus queries all clients in parallel and returns the most conservative (lowest) height.
// CurrentL1 is always zero value — TeeRollup has no L1 derivation.
func (s *TeeRollupProposalSource) SyncStatus(ctx context.Context) (SyncStatus, error) {
	type result struct {
		info TeeRollupBlockInfo
		err  error
	}
	results := make([]result, len(s.clients))
	var wg sync.WaitGroup
	for i, cl := range s.clients {
		wg.Add(1)
		go func(idx int, client TeeRollupClient) {
			defer wg.Done()
			info, err := client.ConfirmedBlockInfo(ctx)
			results[idx] = result{info: info, err: err}
		}(i, cl)
	}
	wg.Wait()

	var lowestHeight uint64
	var errs []error
	first := true
	for _, r := range results {
		if r.err != nil {
			errs = append(errs, r.err)
			continue
		}
		if first || r.info.Height < lowestHeight {
			lowestHeight = r.info.Height
			first = false
		}
	}
	if len(errs) == len(s.clients) {
		return SyncStatus{}, errors.Join(errs...)
	}
	return SyncStatus{
		CurrentL1:   eth.BlockID{}, // always zero — no L1 derivation
		SafeL2:      lowestHeight,
		FinalizedL2: lowestHeight,
	}, nil
}

// ProposalAtSequenceNum fetches the proposal at the given L2 sequence number.
// Tries clients in order, fails over on error. Only accepts exact height match.
func (s *TeeRollupProposalSource) ProposalAtSequenceNum(ctx context.Context, seqNum uint64) (Proposal, error) {
	var lastErr error
	for _, cl := range s.clients {
		info, err := cl.ConfirmedBlockInfoAtHeight(ctx, seqNum)
		if err != nil {
			lastErr = err
			continue
		}
		rootClaim := computeRootClaim(info.BlockHash, info.AppHash)

		// resolve parentIdx dynamically; fall back to MaxUint32 (anchor sentinel) on error
		parentIdx := uint32(math.MaxUint32)
		if s.parentIdxFn != nil {
			if idx, found, err := s.parentIdxFn(ctx); err != nil {
				s.log.Warn("tee-rollup: failed to resolve parent game index, using anchor sentinel", "err", err)
			} else if found {
				parentIdx = idx
			}
		}

		proposal := Proposal{
			Root:        rootClaim,
			SequenceNum: seqNum,
			CurrentL1:   eth.BlockID{}, // always zero — no L1 derivation
			TeeRollupData: &TeeRollupProposalData{
				L2SeqNum:  seqNum,
				ParentIdx: parentIdx,
				BlockHash: info.BlockHash,
				StateHash: info.AppHash,
			},
		}
		return proposal, nil
	}
	return Proposal{}, fmt.Errorf("tee-rollup: all clients failed for seqNum=%d: %w", seqNum, lastErr)
}

// computeRootClaim computes the root claim as keccak256(abi.encode(blockHash, stateHash)).
// abi.encode of two bytes32 values = 64 bytes (each padded to 32 bytes).
func computeRootClaim(blockHash, stateHash common.Hash) common.Hash {
	return crypto.Keccak256Hash(append(blockHash.Bytes(), stateHash.Bytes()...))
}

// Close closes all underlying TeeRollup clients.
func (s *TeeRollupProposalSource) Close() {
	for _, cl := range s.clients {
		cl.Close()
	}
}
