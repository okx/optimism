package syncnode

import (
	"context"
	"errors"
	"sync/atomic"

	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/types"
	"github.com/ethereum/go-ethereum"
)

type resetTracker struct {
	a eth.BlockID
	z eth.BlockID

	synchronous bool
	resetting   *atomic.Bool
	cancelling  *atomic.Bool

	managed *ManagedNode
}

func (t *resetTracker) init() {
	t.resetting.Store(true)
	t.cancelling.Store(false)
	t.a = eth.BlockID{}
	t.z = eth.BlockID{}
}

func (t *resetTracker) beginBisectionReset(z eth.BlockID) {
	t.managed.log.Info("beginning reset", "endOfRange", z)
	// only one reset can be in progress at a time
	if t.resetting.Load() {
		return
	}
	// initialize the reset tracker
	t.init()
	t.z = z
	// action tests may prefer to run the managed node totally synchronously
	if t.synchronous {
		t.bisectToTarget()
	} else {
		go t.bisectToTarget()
	}
}

func (t *resetTracker) endReset() {
	t.resetting.Store(false)
	t.cancelling.Store(false)
}

func (t *resetTracker) isResetting() bool {
	return t.resetting.Load()
}

func (t *resetTracker) cancelReset() {
	t.cancelling.Store(true)
}

func (t *resetTracker) bisectToTarget() {
	nodeCtx, nCancel := context.WithTimeout(t.managed.ctx, nodeTimeout)
	defer nCancel()
	internalCtx, iCancel := context.WithTimeout(t.managed.ctx, internalTimeout)
	defer iCancel()

	// initialize the start of the range if it is empty
	if t.a == (eth.BlockID{}) {
		t.managed.log.Debug("Start of range is empty, fetching the anchor block as starting point")
		anchor, err := t.managed.backend.AnchorPoint(internalCtx, t.managed.chainID)
		if err != nil {
			t.managed.log.Error("failed to initialize start of bisection range", "err", err)
			t.endReset()
			return
		}
		t.managed.log.Debug("Start of range is set to anchor point", "anchor", anchor)
		t.a = anchor.Derived.ID()
	}

	// before starting bisection, check if z is already consistent (i.e. the node is ahead but otherwise consistent)
	nodeZ, err := t.managed.Node.BlockRefByNumber(nodeCtx, t.z.Number)
	// if z is already consistent, we can skip the bisection
	// and move straight to a targeted reset
	if err == nil && nodeZ.ID() == t.z {
		t.resetHeadsFromTarget(t.z)
		return
	}

	// before starting bisection, check if a is inconsistent (i.e. the node has no common reference point)
	// if the first block in the range can't be found or is inconsistent, we can't do a reset
	nodeA, err := t.managed.Node.BlockRefByNumber(nodeCtx, t.a.Number)
	if err != nil {
		t.managed.log.Error("failed to get block at start of range. cannot reset node", "err", err)
		t.endReset()
		return
	}
	if nodeA.ID() != t.a {
		t.managed.log.Error("start of range is inconsistent with logs db. cannot reset node",
			"a", t.a,
			"block", nodeA.ID())
		t.endReset()
		return
	}

	// repeatedly bisect the range until the last consistent block is found
	for {
		if t.cancelling.Load() {
			t.managed.log.Debug("reset cancelled")
			t.endReset()
			return
		}
		if t.a.Number >= t.z.Number {
			t.managed.log.Debug("reset target converged. Resetting to start of range", "a", t.a, "z", t.z)
			t.resetHeadsFromTarget(t.a)
			return
		}
		if t.a.Number+1 == t.z.Number {
			break
		}
		err := t.bisect()
		if err != nil {
			t.managed.log.Error("failed to bisect recovery range. cannot reset node", "err", err)
			t.endReset()
			return
		}
	}
	// the bisection is now complete. a is the last consistent block, and z is the first inconsistent block
	t.resetHeadsFromTarget(t.a)
}

func (t *resetTracker) bisect() error {
	internalCtx, iCancel := context.WithTimeout(t.managed.ctx, internalTimeout)
	defer iCancel()
	nodeCtx, nCancel := context.WithTimeout(t.managed.ctx, nodeTimeout)
	defer nCancel()

	// attempt to get the block at the midpoint of the range
	i := (t.a.Number + t.z.Number) / 2
	nodeIRef, err := t.managed.Node.BlockRefByNumber(nodeCtx, i)
	if err != nil {
		// if the block is not known to the node, it is defacto inconsistent
		if errors.Is(err, ethereum.NotFound) {
			t.managed.log.Trace("midpoint of range is not known to node. pulling back end of range", "i", i)
			t.z = eth.BlockID{Number: i}
			return nil
		} else {
			t.managed.log.Error("failed to get block at midpoint of range. cannot reset node", "err", err)
		}
	}

	// Check if the block at i is consistent with the local-safe DB,
	// if we do not know it yet, then fall back to the local-unsafe blocks (logs DB)
	// and update the search range accordingly.
	nodeI := nodeIRef.ID()
	err = t.managed.backend.IsLocalSafe(internalCtx, t.managed.chainID, nodeI)
	if errors.Is(err, types.ErrFuture) {
		t.managed.log.Debug("No local-safe reference for reset bisection, falling back to local-unsafe", "i", i)
		err = t.managed.backend.IsLocalUnsafe(internalCtx, t.managed.chainID, nodeI)
	}
	if err != nil {
		t.managed.log.Debug("midpoint of range is inconsistent. pulling back end of range", "i", i)
		t.z = nodeI
	} else {
		t.managed.log.Debug("midpoint of range is consistent. pushing up start of range", "i", i)
		t.a = nodeI
	}
	return nil
}

func (t *resetTracker) resetHeadsFromTarget(target eth.BlockID) {
	internalCtx, iCancel := context.WithTimeout(t.managed.ctx, internalTimeout)
	defer iCancel()

	// if the target is empty, no reset can be done
	if target == (eth.BlockID{}) {
		t.managed.log.Error("no reset target found. cannot reset node")
		t.endReset()
		return
	}

	t.managed.log.Info("reset target identified", "target", target)

	// Try to find corresponding L1 block number for direct reset
	// This is an optimization to use the simpler RequestReset RPC if available
	var l1BlockNum uint64
	found := false

	// Check if this target matches our current safe head
	safePair, err := t.managed.backend.LocalSafe(internalCtx, t.managed.chainID)
	if err == nil && safePair.Derived.Number == target.Number {
		l1BlockNum = safePair.Source.Number
		found = true
	} else if source, err := t.managed.backend.SafeDerivedAt(internalCtx, t.managed.chainID, target); err == nil {
		l1BlockNum = source.Number
		found = true
	}

	// If we found an L1 block and the node supports RequestReset, try that first
	if found {
		if resetter, ok := t.managed.Node.(ResetRequester); ok {
			nodeCtx, nCancel := context.WithTimeout(t.managed.ctx, nodeTimeout)
			defer nCancel()

			t.managed.log.Info("attempting reset via RequestReset", "l1_block_number", l1BlockNum)
			if err := resetter.RequestReset(nodeCtx, l1BlockNum); err == nil {
				t.managed.log.Info("successfully reset node using RequestReset", "l1_block_number", l1BlockNum)
				t.endReset()
				return
			} else {
				t.managed.log.Warn("RequestReset failed, falling back to traditional reset", "err", err)
			}
		}
	}

	// Setting up for a traditional full reset
	defer t.endReset()
	t.fullReset(internalCtx, target)
}

func (t *resetTracker) fullReset(ctx context.Context, target eth.BlockID) {
	var lUnsafe, xUnsafe, lSafe, xSafe, finalized eth.BlockID

	// the unsafe block is always the last block we found to be consistent
	lUnsafe = target

	// all other blocks are either the last consistent block, or the last block in the db, whichever is earlier
	// cross unsafe
	lastXUnsafe, err := t.managed.backend.CrossUnsafe(ctx, t.managed.chainID)
	if err != nil {
		t.managed.log.Error("failed to get last cross unsafe block. cancelling reset", "err", err)
		return
	}
	if lastXUnsafe.Number < target.Number {
		xUnsafe = lastXUnsafe
	} else {
		xUnsafe = target
	}
	// local safe
	lastLSafe, err := t.managed.backend.LocalSafe(ctx, t.managed.chainID)
	if err != nil {
		t.managed.log.Error("failed to get last safe block. cancelling reset", "err", err)
		return
	}
	if lastLSafe.Derived.Number < target.Number {
		lSafe = lastLSafe.Derived
	} else {
		lSafe = target
	}
	// cross safe
	lastXSafe, err := t.managed.backend.CrossSafe(ctx, t.managed.chainID)
	if err != nil {
		t.managed.log.Error("failed to get last cross safe block. cancelling reset", "err", err)
		return
	}
	if lastXSafe.Derived.Number < target.Number {
		xSafe = lastXSafe.Derived
	} else {
		xSafe = target
	}
	// finalized
	lastFinalized, err := t.managed.backend.Finalized(ctx, t.managed.chainID)
	if errors.Is(err, types.ErrFuture) {
		t.managed.log.Warn("finalized block is not yet known", "err", err)
		lastFinalized = eth.BlockID{}
	} else if err != nil {
		t.managed.log.Error("failed to get last finalized block. cancelling reset", "err", err)
		return
	}
	if lastFinalized.Number < target.Number {
		finalized = lastFinalized
	} else {
		finalized = target
	}

	// trigger the reset
	t.managed.log.Info("triggering traditional reset on node",
		"localUnsafe", lUnsafe,
		"crossUnsafe", xUnsafe,
		"localSafe", lSafe,
		"crossSafe", xSafe,
		"finalized", finalized)
	t.managed.OnResetReady(lUnsafe, xUnsafe, lSafe, xSafe, finalized)
}
