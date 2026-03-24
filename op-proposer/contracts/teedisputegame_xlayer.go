// For xlayer: TEE dispute game helpers — status/proposer getters and parent game index resolution.
package contracts

import (
	"context"
	"fmt"
	"strings"
	"sync"

	lru "github.com/hashicorp/golang-lru/v2" // bounded LRU cache for game index entries

	"github.com/ethereum-optimism/optimism/op-service/sources/batching"
	"github.com/ethereum-optimism/optimism/op-service/sources/batching/rpcblock"
	"github.com/ethereum/go-ethereum/accounts/abi"
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

// ABI for new game contract's no-arg claimData() struct getter
const newGameClaimDataABIJSON = `[{"name":"claimData","type":"function","inputs":[],"outputs":[{"name":"parentIndex","type":"uint32"},{"name":"counteredBy","type":"address"},{"name":"prover","type":"address"},{"name":"claim","type":"bytes32"},{"name":"status","type":"uint8"},{"name":"deadline","type":"uint64"}],"stateMutability":"view"}]`

// ABI for TeeDisputeGame proxy: status() and proposer()
const teeGameABIJSON = `[{"name":"status","type":"function","inputs":[],"outputs":[{"name":"","type":"uint8"}],"stateMutability":"view"},{"name":"proposer","type":"function","inputs":[],"outputs":[{"name":"","type":"address"}],"stateMutability":"view"}]`

// ABI for DGF.gameImpls(uint32) — returns the implementation contract address for a game type
const dgfGameImplsABIJSON = `[{"name":"gameImpls","type":"function","inputs":[{"name":"_gameType","type":"uint32"}],"outputs":[{"name":"impl_","type":"address"}],"stateMutability":"view"}]`

// ABI for TeeDisputeGame impl anchorStateRegistry() — returns the immutable ASR address
const teeGameAnchorStateRegistryABIJSON = `[{"name":"anchorStateRegistry","type":"function","inputs":[],"outputs":[{"name":"","type":"address"}],"stateMutability":"view"}]`

// ABI for ASR validation: isGameRespected, isGameBlacklisted, isGameRetired
const asrValidationABIJSON = `[{"name":"isGameRespected","type":"function","inputs":[{"name":"_game","type":"address"}],"outputs":[{"name":"","type":"bool"}],"stateMutability":"view"},{"name":"isGameBlacklisted","type":"function","inputs":[{"name":"_game","type":"address"}],"outputs":[{"name":"","type":"bool"}],"stateMutability":"view"},{"name":"isGameRetired","type":"function","inputs":[{"name":"_game","type":"address"}],"outputs":[{"name":"","type":"bool"}],"stateMutability":"view"}]`

// parsed ABI for new game contract's claimData() getter
var newGameClaimDataABI abi.ABI

// parsed ABI for TeeDisputeGame proxy: status() + proposer()
var teeGameABI abi.ABI

// parsed ABIs for DGF gameImpls and impl anchorStateRegistry
var dgfGameImplsABI abi.ABI
var teeGameAnchorStateRegistryABI abi.ABI

// parsed ABI for ASR validation: isGameRespected + isGameBlacklisted + isGameRetired
var asrValidationABI abi.ABI

func init() {
	var err error
	newGameClaimDataABI, err = abi.JSON(strings.NewReader(newGameClaimDataABIJSON))
	if err != nil {
		panic(fmt.Sprintf("failed to parse new game claim data ABI: %v", err))
	}
	teeGameABI, err = abi.JSON(strings.NewReader(teeGameABIJSON))
	if err != nil {
		panic(fmt.Sprintf("failed to parse tee game ABI: %v", err))
	}
	dgfGameImplsABI, err = abi.JSON(strings.NewReader(dgfGameImplsABIJSON))
	if err != nil {
		panic(fmt.Sprintf("failed to parse DGF gameImpls ABI: %v", err))
	}
	teeGameAnchorStateRegistryABI, err = abi.JSON(strings.NewReader(teeGameAnchorStateRegistryABIJSON))
	if err != nil {
		panic(fmt.Sprintf("failed to parse tee game anchorStateRegistry ABI: %v", err))
	}
	asrValidationABI, err = abi.JSON(strings.NewReader(asrValidationABIJSON))
	if err != nil {
		panic(fmt.Sprintf("failed to parse ASR validation ABI: %v", err))
	}
}

// cachedGameEntry holds immutable per-game metadata cached by DGF index.
type cachedGameEntry struct {
	GameType        uint32
	Address         common.Address
	Proposer        common.Address
	ProposerFetched bool
}

// gameIndexCache is an in-memory cache of DGF index -> immutable game metadata.
// entries uses a lazily-initialized LRU cache (thread-safe) to bound memory usage.
// asrAddr is cached separately with a mutex.
type gameIndexCache struct {
	mu      sync.RWMutex                        // protects asrAddr/asrAddrFetched only; entries uses lru's own lock
	once    sync.Once                           // guards one-time LRU initialization
	entries *lru.Cache[uint64, cachedGameEntry] // bounded LRU, thread-safe, lazily initialized
	// ASR address fetched once from the game impl contract and cached
	asrAddr        common.Address
	asrAddrFetched bool
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

// getASRAddr returns the cached ASR address if available.
func (c *gameIndexCache) getASRAddr() (common.Address, bool) {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.asrAddr, c.asrAddrFetched
}

// setASRAddr stores the ASR address in the cache.
func (c *gameIndexCache) setASRAddr(addr common.Address) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.asrAddr = addr
	c.asrAddrFetched = true
}

// gameStatusAndProposerAt fetches status and proposer of a TeeDisputeGame proxy in one batch call.
func (f *DisputeGameFactory) gameStatusAndProposerAt(ctx context.Context, proxyAddr common.Address) (uint8, common.Address, error) {
	cCtx, cancel := context.WithTimeout(ctx, f.networkTimeout)
	defer cancel()
	gameContract := batching.NewBoundContract(&teeGameABI, proxyAddr)
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
func (f *DisputeGameFactory) asrAddrFromImpl(ctx context.Context, gameType uint32) (common.Address, error) {
	if addr, ok := f.teeCache.getASRAddr(); ok {
		return addr, nil
	}
	// Step 1: get impl address from DGF
	cCtx, cancel := context.WithTimeout(ctx, f.networkTimeout)
	defer cancel()
	implsContract := batching.NewBoundContract(&dgfGameImplsABI, f.contract.Addr())
	result, err := f.caller.SingleCall(cCtx, rpcblock.Latest, implsContract.Call("gameImpls", gameType))
	if err != nil {
		return common.Address{}, fmt.Errorf("tee-rollup: failed to get game impl for type %d: %w", gameType, err)
	}
	implAddr := result.GetAddress(0)
	// Step 2: call anchorStateRegistry() on the impl
	cCtx, cancel = context.WithTimeout(ctx, f.networkTimeout)
	defer cancel()
	asrContract := batching.NewBoundContract(&teeGameAnchorStateRegistryABI, implAddr)
	result, err = f.caller.SingleCall(cCtx, rpcblock.Latest, asrContract.Call("anchorStateRegistry"))
	if err != nil {
		return common.Address{}, fmt.Errorf("tee-rollup: failed to get anchorStateRegistry from impl %v: %w", implAddr, err)
	}
	addr := result.GetAddress(0)
	f.teeCache.setASRAddr(addr)
	return addr, nil
}

// isValidParentGame checks AnchorStateRegistry conditions for a candidate parent game.
// Mirrors TeeDisputeGame.initialize() validation: respected && !blacklisted && !retired.
func (f *DisputeGameFactory) isValidParentGame(ctx context.Context, asrAddr common.Address, proxyAddr common.Address) (bool, error) {
	cCtx, cancel := context.WithTimeout(ctx, f.networkTimeout)
	defer cancel()
	asrContract := batching.NewBoundContract(&asrValidationABI, asrAddr)
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

		// fetch status and proposer together (status is always fresh; proposer cached after first fetch)
		status, gameProposer, err := f.gameStatusAndProposerAt(ctx, entry.Address)
		if err != nil {
			return 0, false, fmt.Errorf("tee-rollup: failed to get status/proposer at index %d: %w", idx, err)
		}
		// cache proposer if not yet cached (immutable — set once in initialize())
		if !entry.ProposerFetched {
			entry.Proposer = gameProposer
			entry.ProposerFetched = true
			f.teeCache.set(idx, entry)
		} else {
			gameProposer = entry.Proposer // use cached value
		}

		// contract rejects CHALLENGER_WINS parents (TeeDisputeGame.sol:204)
		if status == GameStatusChallengerWins {
			if idx == 0 {
				break
			}
			continue
		}

		// validate against AnchorStateRegistry using impl's ASR address
		asrAddr, err := f.asrAddrFromImpl(ctx, gameType)
		if err != nil {
			return 0, false, fmt.Errorf("tee-rollup: failed to get ASR addr at index %d: %w", idx, err)
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
