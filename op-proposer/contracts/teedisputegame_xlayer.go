// For xlayer: TEE dispute game helpers — status/proposer getters and parent game index resolution.
package contracts

import (
	"context"
	"fmt"
	"strings"
	"sync"

	"github.com/ethereum-optimism/optimism/op-service/sources/batching"
	"github.com/ethereum-optimism/optimism/op-service/sources/batching/rpcblock"
	"github.com/ethereum/go-ethereum/accounts/abi"
	"github.com/ethereum/go-ethereum/common"
)

const teeGameType uint32 = 1960 // For xlayer: TEE game type (TeeRollup)

const teeParentScanLimit = 1000 // For xlayer: max DGF entries to scan for parent game

// For xlayer: GameStatus enum values matching TeeDisputeGame (Types.sol)
const (
	GameStatusInProgress     uint8 = 0
	GameStatusChallengerWins uint8 = 1
	GameStatusDefenderWins   uint8 = 2
)

// For xlayer: ABI for new game contract's no-arg claimData() struct getter
const newGameClaimDataABIJSON = `[{"name":"claimData","type":"function","inputs":[],"outputs":[{"name":"parentIndex","type":"uint32"},{"name":"counteredBy","type":"address"},{"name":"prover","type":"address"},{"name":"claim","type":"bytes32"},{"name":"status","type":"uint8"},{"name":"deadline","type":"uint64"}],"stateMutability":"view"}]`

// For xlayer: ABI for TeeDisputeGame status() and proposer() getters
const teeGameStatusABIJSON = `[{"name":"status","type":"function","inputs":[],"outputs":[{"name":"","type":"uint8"}],"stateMutability":"view"}]`
const teeGameProposerABIJSON = `[{"name":"proposer","type":"function","inputs":[],"outputs":[{"name":"","type":"address"}],"stateMutability":"view"}]`

// For xlayer: parsed ABI for new game contract's claimData() getter
var newGameClaimDataABI abi.ABI

// For xlayer: parsed ABIs for TeeDisputeGame status() and proposer() getters
var teeGameStatusABI abi.ABI
var teeGameProposerABI abi.ABI

func init() {
	var err error
	newGameClaimDataABI, err = abi.JSON(strings.NewReader(newGameClaimDataABIJSON))
	if err != nil {
		panic(fmt.Sprintf("failed to parse new game claim data ABI: %v", err))
	}
	teeGameStatusABI, err = abi.JSON(strings.NewReader(teeGameStatusABIJSON))
	if err != nil {
		panic(fmt.Sprintf("failed to parse tee game status ABI: %v", err))
	}
	teeGameProposerABI, err = abi.JSON(strings.NewReader(teeGameProposerABIJSON))
	if err != nil {
		panic(fmt.Sprintf("failed to parse tee game proposer ABI: %v", err))
	}
}

// For xlayer: cachedGameEntry holds immutable per-game metadata cached by DGF index.
type cachedGameEntry struct {
	GameType        uint32
	Address         common.Address
	Proposer        common.Address
	ProposerFetched bool
}

// For xlayer: gameIndexCache is an in-memory, RWMutex-protected cache of DGF index -> immutable game metadata.
// GameType and Address are cached on first fetch. Proposer (also immutable) is cached lazily on first need.
// Game status is NOT cached — it changes when a game is resolved.
type gameIndexCache struct {
	mu      sync.RWMutex
	entries map[uint64]cachedGameEntry
}

func (c *gameIndexCache) get(idx uint64) (cachedGameEntry, bool) {
	c.mu.RLock()
	defer c.mu.RUnlock()
	e, ok := c.entries[idx]
	return e, ok
}

func (c *gameIndexCache) set(idx uint64, e cachedGameEntry) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.entries == nil {
		c.entries = make(map[uint64]cachedGameEntry)
	}
	c.entries[idx] = e
}

// For xlayer: gameStatusAt fetches the current GameStatus of a TeeDisputeGame proxy via status() getter.
// Status is mutable (changes on resolve) and must NOT be cached.
func (f *DisputeGameFactory) gameStatusAt(ctx context.Context, proxyAddr common.Address) (uint8, error) {
	cCtx, cancel := context.WithTimeout(ctx, f.networkTimeout)
	defer cancel()
	statusContract := batching.NewBoundContract(&teeGameStatusABI, proxyAddr)
	result, err := f.caller.SingleCall(cCtx, rpcblock.Latest, statusContract.Call("status"))
	if err != nil {
		return 0, fmt.Errorf("tee-rollup: failed to get status of game %v: %w", proxyAddr, err)
	}
	return result.GetUint8(0), nil
}

// For xlayer: gameProposerAt fetches the proposer() of a TeeDisputeGame proxy.
// Proposer is immutable (set to tx.origin in initialize()) but cached lazily.
func (f *DisputeGameFactory) gameProposerAt(ctx context.Context, proxyAddr common.Address) (common.Address, error) {
	cCtx, cancel := context.WithTimeout(ctx, f.networkTimeout)
	defer cancel()
	proposerContract := batching.NewBoundContract(&teeGameProposerABI, proxyAddr)
	result, err := f.caller.SingleCall(cCtx, rpcblock.Latest, proposerContract.Call("proposer"))
	if err != nil {
		return common.Address{}, fmt.Errorf("tee-rollup: failed to get proposer of game %v: %w", proxyAddr, err)
	}
	return result.GetAddress(0), nil
}

// For xlayer: FindLastGameIndex scans the DGF in reverse to find the most recent game with the
// given gameType that is a valid parent candidate:
//   - DEFENDER_WINS: accepted immediately (contract allows it)
//   - IN_PROGRESS + self-proposed: accepted (proposer == self)
//   - CHALLENGER_WINS: skipped (contract rejects with InvalidParentGame)
//   - IN_PROGRESS + other proposer: skipped (cannot control resolution)
//
// Uses an in-memory cache for immutable fields (GameType, Address, Proposer) to avoid redundant RPCs.
// Returns (idx, true, nil) if found, (0, false, nil) if not found within maxScan, or (0, false, err) on error.
func (f *DisputeGameFactory) FindLastGameIndex(ctx context.Context, gameType uint32, proposer common.Address, maxScan uint64) (uint64, bool, error) { // For xlayer
	gameCount, err := f.gameCount(ctx)
	if err != nil {
		return 0, false, fmt.Errorf("tee-rollup: failed to get game count: %w", err)
	}
	if gameCount == 0 {
		return 0, false, nil
	}
	scanned := uint64(0)
	for idx := gameCount - 1; ; idx-- {
		if scanned >= maxScan {
			return 0, false, nil
		}
		scanned++

		// For xlayer: check cache first — GameType and Address are immutable per DGF index
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

		// For xlayer: fetch status fresh — mutable, must not be cached
		status, err := f.gameStatusAt(ctx, entry.Address)
		if err != nil {
			return 0, false, fmt.Errorf("tee-rollup: failed to get status at index %d: %w", idx, err)
		}

		// For xlayer: contract rejects CHALLENGER_WINS parents (TeeDisputeGame.sol:204)
		if status == GameStatusChallengerWins {
			if idx == 0 {
				break
			}
			continue
		}

		// For xlayer: DEFENDER_WINS — accept immediately
		if status == GameStatusDefenderWins {
			return idx, true, nil
		}

		// For xlayer: IN_PROGRESS — only accept if self-proposed
		// Check cached proposer first; only call gameProposerAt on cache miss
		gameProposer := entry.Proposer
		if !entry.ProposerFetched {
			gameProposer, err = f.gameProposerAt(ctx, entry.Address)
			if err != nil {
				// For xlayer: cannot verify proposer — skip this game
				if idx == 0 {
					break
				}
				continue
			}
			// For xlayer: cache proposer (immutable — set once in initialize())
			entry.Proposer = gameProposer
			entry.ProposerFetched = true
			f.teeCache.set(idx, entry)
		}

		if gameProposer == proposer {
			return idx, true, nil
		}
		// For xlayer: IN_PROGRESS by another proposer — skip

		if idx == 0 {
			break
		}
	}
	return 0, false, nil
}
