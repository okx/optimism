// For xlayer: TEE dispute game helpers — status/proposer getters and parent game index resolution.
package contracts

import (
	"context"
	"fmt"
	"sync"

	lru "github.com/hashicorp/golang-lru/v2" // bounded LRU cache for game index entries

	"github.com/ethereum-optimism/optimism/op-service/sources/batching"
	"github.com/ethereum-optimism/optimism/op-service/sources/batching/rpcblock"
	"github.com/ethereum-optimism/optimism/packages/contracts-bedrock/snapshots"
	"github.com/ethereum/go-ethereum/common"
)

// TEEGameType is the dispute game type ID for TeeRollup TEE attestations.
// Defined here (contracts package) so both contracts and proposer packages can reference it
// without a circular import (proposer already imports contracts).
const TEEGameType uint32 = 1960

const TeeParentScanLimit = 1000 // max DGF entries to scan for parent game

// GameStatus enum values matching TeeDisputeGame (Types.sol)
const (
	GameStatusInProgress     uint8 = 0
	GameStatusChallengerWins uint8 = 1
	GameStatusDefenderWins   uint8 = 2
)

// For xlayer: ABI instances loaded once at package init from compiled snapshots and reused
// across all calls (parsed once for performance, avoids repeated JSON parsing per call).
var (
	teeDisputeGameSnapshotABI = snapshots.LoadTeeDisputeGameABI()
	asrSnapshotABI            = snapshots.LoadAnchorStateRegistryABI() // For xlayer
)

// cachedGameEntry holds immutable per-game metadata cached by DGF index.
type cachedGameEntry struct {
	GameType        uint32
	Address         common.Address
	Proposer        common.Address
	ProposerFetched bool
}

// gameIndexCache is an in-memory cache of DGF index -> immutable game metadata.
// entries uses a lazily-initialized LRU cache (thread-safe) to bound memory usage.
// asrAddr is fetched exactly once via asrOnce.
type gameIndexCache struct {
	once    sync.Once                           // guards one-time LRU initialization
	entries *lru.Cache[uint64, cachedGameEntry] // bounded LRU, thread-safe, lazily initialized
	// For xlayer: guards one-time ASR address fetch
	asrOnce sync.Once
	asrAddr common.Address
	asrErr  error
}

// lruEntries returns the lazily-initialized LRU cache.
// lru.New never fails for a positive size, so the error is safely ignored.
func (c *gameIndexCache) lruEntries() *lru.Cache[uint64, cachedGameEntry] {
	c.once.Do(func() {
		c.entries, _ = lru.New[uint64, cachedGameEntry](4096) // 4096-entry bounded LRU
	})
	return c.entries
}

func (c *gameIndexCache) get(idx uint64) (cachedGameEntry, bool) {
	return c.lruEntries().Get(idx) // lru.Cache is thread-safe; no mutex needed
}

func (c *gameIndexCache) set(idx uint64, e cachedGameEntry) {
	c.lruEntries().Add(idx, e) // lru.Cache is thread-safe; no mutex needed
}

// gameStatusAndProposerAt fetches status and proposer of a TeeDisputeGame proxy in one batch call.
func (f *DisputeGameFactory) gameStatusAndProposerAt(ctx context.Context, proxyAddr common.Address) (uint8, common.Address, error) {
	cCtx, cancel := context.WithTimeout(ctx, f.networkTimeout)
	defer cancel()
	// For xlayer: use snapshot ABI loaded from compiled artifact instead of inline JSON
	gameContract := batching.NewBoundContract(teeDisputeGameSnapshotABI, proxyAddr)
	results, err := f.caller.Call(cCtx, rpcblock.Latest,
		gameContract.Call("status"),
		gameContract.Call("proposer"),
	)
	if err != nil {
		return 0, common.Address{}, fmt.Errorf("tee-rollup: failed to get status/proposer of game %v: %w", proxyAddr, err)
	}
	return results[0].GetUint8(0), results[1].GetAddress(0), nil
}

// asrAddrFromImpl fetches the AnchorStateRegistry address from the DGF's
// game implementation contract for the given gameType. The result is immutable and cached.
// For xlayer: sync.Once guarantees the RPC is issued exactly once even under concurrent callers,
// eliminating the TOCTOU race of the previous check-then-fetch pattern.
func (f *DisputeGameFactory) asrAddrFromImpl(ctx context.Context, gameType uint32) (common.Address, error) {
	f.teeCache.asrOnce.Do(func() {
		// Step 1: get impl address from DGF
		cCtx, cancel := context.WithTimeout(ctx, f.networkTimeout)
		defer cancel()
		result, err := f.caller.SingleCall(cCtx, rpcblock.Latest, f.contract.Call("gameImpls", gameType))
		if err != nil {
			f.teeCache.asrErr = fmt.Errorf("tee-rollup: failed to get game impl for type %d: %w", gameType, err)
			return
		}
		implAddr := result.GetAddress(0)
		// Step 2: call anchorStateRegistry() on the impl
		// For xlayer: use snapshot ABI loaded from compiled artifact instead of inline JSON
		cCtx2, cancel2 := context.WithTimeout(ctx, f.networkTimeout)
		defer cancel2()
		asrContract := batching.NewBoundContract(teeDisputeGameSnapshotABI, implAddr)
		result, err = f.caller.SingleCall(cCtx2, rpcblock.Latest, asrContract.Call("anchorStateRegistry"))
		if err != nil {
			f.teeCache.asrErr = fmt.Errorf("tee-rollup: failed to get anchorStateRegistry from impl %v: %w", implAddr, err)
			return
		}
		f.teeCache.asrAddr = result.GetAddress(0)
	})
	if f.teeCache.asrErr != nil {
		return common.Address{}, f.teeCache.asrErr
	}
	return f.teeCache.asrAddr, nil
}

// isValidParentGame checks AnchorStateRegistry conditions for a candidate parent game.
// Mirrors TeeDisputeGame.initialize() validation: respected && !blacklisted && !retired.
func (f *DisputeGameFactory) isValidParentGame(ctx context.Context, asrAddr common.Address, proxyAddr common.Address) (bool, error) {
	cCtx, cancel := context.WithTimeout(ctx, f.networkTimeout)
	defer cancel()
	asrContract := batching.NewBoundContract(asrSnapshotABI, asrAddr)
	results, err := f.caller.Call(cCtx, rpcblock.Latest,
		asrContract.Call("isGameRespected", proxyAddr),
		asrContract.Call("isGameBlacklisted", proxyAddr),
		asrContract.Call("isGameRetired", proxyAddr),
	)
	if err != nil {
		return false, fmt.Errorf("tee-rollup: failed ASR validation for %v: %w", proxyAddr, err)
	}
	respected := results[0].GetBool(0)
	blacklisted := results[1].GetBool(0)
	retired := results[2].GetBool(0)
	return respected && !blacklisted && !retired, nil
}

// FindLastGameIndex scans the DGF in reverse to find the most recent game with the
// given gameType that is a valid parent candidate:
//   - DEFENDER_WINS: accepted immediately (contract allows it)
//   - IN_PROGRESS + self-proposed: accepted (proposer == self)
//   - CHALLENGER_WINS: skipped (contract rejects with InvalidParentGame)
//   - IN_PROGRESS + other proposer: skipped (cannot control resolution)
//
// Uses an in-memory cache for immutable fields (GameType, Address, Proposer) to avoid redundant RPCs.
// Returns (idx, true, nil) if found, (0, false, nil) if not found within maxScan, or (0, false, err) on error.
func (f *DisputeGameFactory) FindLastGameIndex(ctx context.Context, gameType uint32, proposer common.Address, maxScan uint64) (uint64, bool, error) {
	gameCount, err := f.gameCount(ctx)
	if err != nil {
		return 0, false, fmt.Errorf("tee-rollup: failed to get game count: %w", err)
	}
	if gameCount == 0 {
		return 0, false, nil
	}
	// For xlayer: hoist ASR address lookup before the loop — it is immutable per game type
	// and caching avoids a redundant RPC on every iteration.
	asrAddr, err := f.asrAddrFromImpl(ctx, gameType)
	if err != nil {
		return 0, false, fmt.Errorf("tee-rollup: failed to get ASR addr for game type %d: %w", gameType, err)
	}
	scanned := uint64(0)
	for idx := gameCount - 1; ; idx-- {
		if scanned >= maxScan {
			return 0, false, nil
		}
		scanned++

		// check cache first — GameType and Address are immutable per DGF index
		entry, cached := f.teeCache.get(idx)
		if !cached {
			game, err := f.gameAtIndex(ctx, idx)
			if err != nil {
				return 0, false, fmt.Errorf("tee-rollup: failed to get game at index %d: %w", idx, err)
			}
			entry = cachedGameEntry{GameType: game.GameType, Address: game.Address}
			f.teeCache.set(idx, entry)
		}

		if entry.GameType != gameType {
			if idx == 0 {
				break
			}
			continue
		}

		// For xlayer: when proposer is already cached, only fetch status (SingleCall);
		// otherwise batch-fetch both status and proposer together.
		var status uint8
		var gameProposer common.Address
		if entry.ProposerFetched {
			// proposer is immutable — reuse cached value, only fetch fresh status
			cCtx, cancel := context.WithTimeout(ctx, f.networkTimeout)
			gameContract := batching.NewBoundContract(teeDisputeGameSnapshotABI, entry.Address)
			result, callErr := f.caller.SingleCall(cCtx, rpcblock.Latest, gameContract.Call("status"))
			cancel()
			if callErr != nil {
				return 0, false, fmt.Errorf("tee-rollup: failed to get status at index %d: %w", idx, callErr)
			}
			status = result.GetUint8(0)
			gameProposer = entry.Proposer
		} else {
			// first time seeing this game — batch-fetch status and proposer together
			var fetchErr error
			status, gameProposer, fetchErr = f.gameStatusAndProposerAt(ctx, entry.Address)
			if fetchErr != nil {
				return 0, false, fmt.Errorf("tee-rollup: failed to get status/proposer at index %d: %w", idx, fetchErr)
			}
			// cache proposer (immutable — set once in initialize())
			entry.Proposer = gameProposer
			entry.ProposerFetched = true
			f.teeCache.set(idx, entry)
		}

		// contract rejects CHALLENGER_WINS parents (TeeDisputeGame.sol:204)
		if status == GameStatusChallengerWins {
			if idx == 0 {
				break
			}
			continue
		}
		valid, err := f.isValidParentGame(ctx, asrAddr, entry.Address)
		if err != nil {
			return 0, false, fmt.Errorf("tee-rollup: failed to validate parent game at index %d: %w", idx, err)
		}
		if !valid {
			// game is retired/blacklisted/not-respected — skip
			if idx == 0 {
				break
			}
			continue
		}

		// DEFENDER_WINS — accept immediately
		if status == GameStatusDefenderWins {
			return idx, true, nil
		}

		// IN_PROGRESS — only accept if self-proposed
		if gameProposer == proposer {
			return idx, true, nil
		}
		// IN_PROGRESS by another proposer — skip

		if idx == 0 {
			break
		}
	}
	return 0, false, nil
}
