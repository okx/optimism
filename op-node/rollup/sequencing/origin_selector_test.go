package sequencing

import (
	"context"
	"errors"
	"testing"

	"github.com/ethereum-optimism/optimism/op-node/rollup"
	"github.com/ethereum-optimism/optimism/op-node/rollup/confdepth"
	"github.com/ethereum-optimism/optimism/op-node/rollup/derive"
	"github.com/ethereum-optimism/optimism/op-node/rollup/engine"
	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-service/testlog"
	"github.com/ethereum-optimism/optimism/op-service/testutils"
	"github.com/ethereum/go-ethereum"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/log"
	"github.com/stretchr/testify/require"
)

// TestOriginSelectorFetchCurrentError ensures that the origin selector
// returns an error when it cannot fetch the current origin and has no
// internal cached state.
func TestOriginSelectorFetchCurrentError(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	log := testlog.Logger(t, log.LevelCrit)
	cfg := &rollup.Config{
		MaxSequencerDrift: 500,
		BlockTime:         2,
	}
	l1 := &testutils.MockL1Source{}
	defer l1.AssertExpectations(t)
	a := eth.L1BlockRef{
		Hash:   common.Hash{'a'},
		Number: 10,
		Time:   20,
	}
	b := eth.L1BlockRef{
		Hash:       common.Hash{'b'},
		Number:     11,
		Time:       25,
		ParentHash: a.Hash,
	}
	l2Head := eth.L2BlockRef{
		L1Origin: a.ID(),
		Time:     24,
	}

	l1.ExpectL1BlockRefByHash(a.Hash, eth.L1BlockRef{}, errors.New("test error"))

	s := NewL1OriginSelector(ctx, log, cfg, l1)

	_, err := s.FindL1Origin(ctx, l2Head)
	require.ErrorContains(t, err, "test error")

	// The same outcome occurs when the cached origin is different from that of the L2 head.
	l1.ExpectL1BlockRefByHash(a.Hash, eth.L1BlockRef{}, errors.New("test error"))

	s = NewL1OriginSelector(ctx, log, cfg, l1)
	s.currentOrigin = b

	_, err = s.FindL1Origin(ctx, l2Head)
	require.ErrorContains(t, err, "test error")
}

// TestOriginSelectorFetchNextError ensures that the origin selector
// gracefully handles an error when fetching the next origin from the
// forkchoice update event.
func TestOriginSelectorFetchNextError(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	log := testlog.Logger(t, log.LevelCrit)
	cfg := &rollup.Config{
		MaxSequencerDrift: 500,
		BlockTime:         2,
	}
	l1 := &testutils.MockL1Source{}
	defer l1.AssertExpectations(t)
	a := eth.L1BlockRef{
		Hash:   common.Hash{'a'},
		Number: 10,
		Time:   20,
	}
	b := eth.L1BlockRef{
		Hash:   common.Hash{'b'},
		Number: 11,
	}
	l2Head := eth.L2BlockRef{
		L1Origin: a.ID(),
		Time:     24,
	}

	s := NewL1OriginSelector(ctx, log, cfg, l1)
	s.currentOrigin = a

	next, err := s.FindL1Origin(ctx, l2Head)
	require.Nil(t, err)
	require.Equal(t, a, next)

	l1.ExpectL1BlockRefByNumber(b.Number, eth.L1BlockRef{}, ethereum.NotFound)

	handled := s.OnEvent(context.Background(), engine.ForkchoiceUpdateEvent{UnsafeL2Head: l2Head})
	require.True(t, handled)

	l1.ExpectL1BlockRefByNumber(b.Number, eth.L1BlockRef{}, errors.New("test error"))

	handled = s.OnEvent(context.Background(), engine.ForkchoiceUpdateEvent{UnsafeL2Head: l2Head})
	require.True(t, handled)

	// The next origin should still be `a` because the fetch failed.
	next, err = s.FindL1Origin(ctx, l2Head)
	require.Nil(t, err)
	require.Equal(t, a, next)
}

// TestOriginSelectorAdvances ensures that the origin selector
// advances the origin with the internal cache
//
// There are 3 L1 blocks at times 20, 22, 24. The L2 Head is at time 24.
// The next L2 time is 26 which is after the next L1 block time. There
// is no conf depth to stop the origin selection so block `b` should
// be the next L1 origin, and then block `c` is the subsequent L1 origin.
func TestOriginSelectorAdvances(t *testing.T) {

	testOriginSelectorAdvances := func(t *testing.T, recoverMode bool) {
		ctx, cancel := context.WithCancel(context.Background())
		defer cancel()

		log := testlog.Logger(t, log.LevelCrit)
		cfg := &rollup.Config{
			MaxSequencerDrift: 500,
			BlockTime:         2,
		}
		l1 := &testutils.MockL1Source{}
		defer l1.AssertExpectations(t)
		a := eth.L1BlockRef{
			Hash:   common.Hash{'a'},
			Number: 10,
			Time:   20,
		}
		b := eth.L1BlockRef{
			Hash:       common.Hash{'b'},
			Number:     11,
			Time:       22,
			ParentHash: a.Hash,
		}
		c := eth.L1BlockRef{
			Hash:       common.Hash{'c'},
			Number:     12,
			Time:       24,
			ParentHash: b.Hash,
		}
		d := eth.L1BlockRef{
			Hash:       common.Hash{'d'},
			Number:     13,
			Time:       36,
			ParentHash: c.Hash,
		}
		l2Head := eth.L2BlockRef{
			L1Origin: a.ID(),
			Time:     24,
		}

		s := NewL1OriginSelector(ctx, log, cfg, l1)

		requireL1OriginAt := func(l2Head eth.L2BlockRef, want eth.L1BlockRef) {
			got, err := s.FindL1Origin(ctx, l2Head)
			require.Nil(t, err)
			require.Equal(t, want, got)
		}

		s.currentOrigin = a
		s.nextOrigin = b

		// Trigger the background fetch via a forkchoice update.
		// This should be a no-op because the next origin is already cached.
		handled := s.OnEvent(context.Background(), engine.ForkchoiceUpdateEvent{UnsafeL2Head: l2Head})
		require.True(t, handled)

		requireL1OriginAt(l2Head, b)

		l2Head = eth.L2BlockRef{
			L1Origin: b.ID(),
			Time:     26,
		}

		// The origin is still `b` because the next origin has not been fetched yet.
		requireL1OriginAt(l2Head, b)

		l1.ExpectL1BlockRefByNumber(c.Number, c, nil)

		// Trigger the background fetch via a forkchoice update.
		// This will actually fetch the next origin because the internal cache is empty.
		handled = s.OnEvent(context.Background(), engine.ForkchoiceUpdateEvent{UnsafeL2Head: l2Head})
		require.True(t, handled)

		// The next origin should be `c` now.
		requireL1OriginAt(l2Head, c)

		// Now force the retrieval of the next L1 origin
		s.recoverMode.Store(recoverMode)

		l2Head = eth.L2BlockRef{
			L1Origin: c.ID(),
			Time:     d.Time + 4,
		}

		if recoverMode {
			// In recovery mode (only) we make two RPC calls.
			// First, cover the case where the nextOrigin
			// is not ready yet by simulating a NotFound error.
			l1.ExpectL1BlockRefByHash(c.Hash, c, nil)
			l1.ExpectL1BlockRefByNumber(d.Number, eth.BlockRef{}, ethereum.NotFound)
			_, err := s.FindL1Origin(ctx, l2Head)
			require.ErrorIs(t, err, derive.ErrTemporary)
			require.ErrorIs(t, err, ethereum.NotFound)

			// Now, simulate the block being ready, and ensure
			// that the origin advances to the next block.
			l1.ExpectL1BlockRefByHash(c.Hash, c, nil)
			l1.ExpectL1BlockRefByNumber(d.Number, d, nil)
			requireL1OriginAt(l2Head, d)
		} else {
			requireL1OriginAt(l2Head, c)
		}
	}

	t.Run("normal", func(t *testing.T) { testOriginSelectorAdvances(t, false) })
	t.Run("recover_mode", func(t *testing.T) { testOriginSelectorAdvances(t, true) })
}

// TestOriginSelectorHandlesReset ensures that the origin selector
// resets its internal cached state on derivation pipeline resets.
func TestOriginSelectorHandlesReset(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	log := testlog.Logger(t, log.LevelCrit)
	cfg := &rollup.Config{
		MaxSequencerDrift: 500,
		BlockTime:         2,
	}
	l1 := &testutils.MockL1Source{}
	defer l1.AssertExpectations(t)
	a := eth.L1BlockRef{
		Hash:   common.Hash{'a'},
		Number: 10,
		Time:   20,
	}
	b := eth.L1BlockRef{
		Hash:       common.Hash{'b'},
		Number:     11,
		Time:       25,
		ParentHash: a.Hash,
	}
	l2Head := eth.L2BlockRef{
		L1Origin: a.ID(),
		Time:     24,
	}

	s := NewL1OriginSelector(ctx, log, cfg, l1)
	s.currentOrigin = a
	s.nextOrigin = b

	next, err := s.FindL1Origin(ctx, l2Head)
	require.Nil(t, err)
	require.Equal(t, b, next)

	// Trigger the pipeline reset
	handled := s.OnEvent(context.Background(), rollup.ResetEvent{})
	require.True(t, handled)

	// The next origin should be `a` now, but we need to fetch it
	// because the internal cache was reset.
	l1.ExpectL1BlockRefByHash(a.Hash, a, nil)

	next, err = s.FindL1Origin(ctx, l2Head)
	require.Nil(t, err)
	require.Equal(t, a, next)
}

// TestOriginSelectorFetchesNextOrigin ensures that the origin selector
// fetches the next origin when a fcu is received and the internal cache is empty
//
// The next L2 time is 26 which is after the next L1 block time. There
// is no conf depth to stop the origin selection so block `b` will
// be the next L1 origin as soon as it is fetched.
func TestOriginSelectorFetchesNextOrigin(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	log := testlog.Logger(t, log.LevelCrit)
	cfg := &rollup.Config{
		MaxSequencerDrift: 500,
		BlockTime:         2,
	}
	l1 := &testutils.MockL1Source{}
	defer l1.AssertExpectations(t)
	a := eth.L1BlockRef{
		Hash:   common.Hash{'a'},
		Number: 10,
		Time:   20,
	}
	b := eth.L1BlockRef{
		Hash:       common.Hash{'b'},
		Number:     11,
		Time:       25,
		ParentHash: a.Hash,
	}
	l2Head := eth.L2BlockRef{
		L1Origin: a.ID(),
		Time:     24,
	}

	// This is called as part of the background prefetch job
	l1.ExpectL1BlockRefByNumber(b.Number, b, nil)

	s := NewL1OriginSelector(ctx, log, cfg, l1)
	s.currentOrigin = a

	next, err := s.FindL1Origin(ctx, l2Head)
	require.Nil(t, err)
	require.Equal(t, a, next)

	// Selection is stable until the next origin is fetched
	next, err = s.FindL1Origin(ctx, l2Head)
	require.Nil(t, err)
	require.Equal(t, a, next)

	// Trigger the background fetch via a forkchoice update
	handled := s.OnEvent(context.Background(), engine.ForkchoiceUpdateEvent{UnsafeL2Head: l2Head})
	require.True(t, handled)

	// The next origin should be `b` now.
	next, err = s.FindL1Origin(ctx, l2Head)
	require.Nil(t, err)
	require.Equal(t, b, next)
}

// TestOriginSelectorRespectsOriginTiming ensures that the origin selector
// does not pick an origin that is ahead of the next L2 block time
//
// There are 2 L1 blocks at time 20 & 25. The L2 Head is at time 22.
// The next L2 time is 24 which is before the next L1 block time. There
// is no conf depth to stop the LOS from potentially selecting block `b`
// but it should select block `a` because the L2 block time must be ahead
// of the the timestamp of it's L1 origin.
func TestOriginSelectorRespectsOriginTiming(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	log := testlog.Logger(t, log.LevelCrit)
	cfg := &rollup.Config{
		MaxSequencerDrift: 500,
		BlockTime:         2,
	}
	l1 := &testutils.MockL1Source{}
	defer l1.AssertExpectations(t)
	a := eth.L1BlockRef{
		Hash:   common.Hash{'a'},
		Number: 10,
		Time:   20,
	}
	b := eth.L1BlockRef{
		Hash:       common.Hash{'b'},
		Number:     11,
		Time:       25,
		ParentHash: a.Hash,
	}
	l2Head := eth.L2BlockRef{
		L1Origin: a.ID(),
		Time:     22,
	}

	s := NewL1OriginSelector(ctx, log, cfg, l1)
	s.currentOrigin = a
	s.nextOrigin = b

	next, err := s.FindL1Origin(ctx, l2Head)
	require.Nil(t, err)
	require.Equal(t, a, next)
}

// TestOriginSelectorRespectsSeqDrift
//
// There are 2 L1 blocks at time 20 & 25. The L2 Head is at time 27.
// The next L2 time is 29. The sequencer drift is 8 so the L2 head is
// valid with origin `a`, but the next L2 block is not valid with origin `b.`
// This is because 29 (next L2 time) > 20 (origin) + 8 (seq drift) => invalid block.
// The origin selector does not yet know about block `b` so it should wait for the
// background fetch to complete synchronously.
func TestOriginSelectorRespectsSeqDrift(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	log := testlog.Logger(t, log.LevelCrit)
	cfg := &rollup.Config{
		MaxSequencerDrift: 8,
		BlockTime:         2,
	}
	l1 := &testutils.MockL1Source{}
	defer l1.AssertExpectations(t)
	a := eth.L1BlockRef{
		Hash:   common.Hash{'a'},
		Number: 10,
		Time:   20,
	}
	b := eth.L1BlockRef{
		Hash:       common.Hash{'b'},
		Number:     11,
		Time:       25,
		ParentHash: a.Hash,
	}
	l2Head := eth.L2BlockRef{
		L1Origin: a.ID(),
		Time:     27,
	}

	l1.ExpectL1BlockRefByHash(a.Hash, a, nil)

	l1.ExpectL1BlockRefByNumber(b.Number, b, nil)

	s := NewL1OriginSelector(ctx, log, cfg, l1)

	next, err := s.FindL1Origin(ctx, l2Head)
	require.NoError(t, err)
	require.Equal(t, b, next)
}

// TestOriginSelectorRespectsConfDepth ensures that the origin selector
// will respect the confirmation depth requirement
//
// There are 2 L1 blocks at time 20 & 25. The L2 Head is at time 27.
// The next L2 time is 29 which enough to normally select block `b`
// as the origin, however block `b` is the L1 Head & the sequencer
// needs to wait until that block is confirmed enough before advancing.
func TestOriginSelectorRespectsConfDepth(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	log := testlog.Logger(t, log.LevelCrit)
	cfg := &rollup.Config{
		MaxSequencerDrift: 500,
		BlockTime:         2,
	}
	l1 := &testutils.MockL1Source{}
	defer l1.AssertExpectations(t)
	a := eth.L1BlockRef{
		Hash:   common.Hash{'a'},
		Number: 10,
		Time:   20,
	}
	b := eth.L1BlockRef{
		Hash:       common.Hash{'b'},
		Number:     11,
		Time:       25,
		ParentHash: a.Hash,
	}
	l2Head := eth.L2BlockRef{
		L1Origin: a.ID(),
		Time:     27,
	}

	confDepthL1 := confdepth.NewConfDepth(10, func() eth.L1BlockRef { return b }, l1)
	s := NewL1OriginSelector(ctx, log, cfg, confDepthL1)
	s.currentOrigin = a

	next, err := s.FindL1Origin(ctx, l2Head)
	require.Nil(t, err)
	require.Equal(t, a, next)
}

// TestOriginSelectorStrictConfDepth ensures that the origin selector will maintain the sequencer conf depth,
// even while the time delta between the current L1 origin and the next
// L2 block is greater than the sequencer drift.
// It's more important to maintain safety with an empty block than to maintain liveness with poor conf depth.
//
// There are 2 L1 blocks at time 20 & 25. The L2 Head is at time 27.
// The next L2 time is 29. The sequencer drift is 8 so the L2 head is
// valid with origin `a`, but the next L2 block is not valid with origin `b.`
// This is because 29 (next L2 time) > 20 (origin) + 8 (seq drift) => invalid block.
// We maintain confirmation distance, even though we would shift to the next origin if we could.
func TestOriginSelectorStrictConfDepth(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	log := testlog.Logger(t, log.LevelCrit)
	cfg := &rollup.Config{
		MaxSequencerDrift: 8,
		BlockTime:         2,
	}
	l1 := &testutils.MockL1Source{}
	defer l1.AssertExpectations(t)
	a := eth.L1BlockRef{
		Hash:   common.Hash{'a'},
		Number: 10,
		Time:   20,
	}
	b := eth.L1BlockRef{
		Hash:       common.Hash{'b'},
		Number:     11,
		Time:       25,
		ParentHash: a.Hash,
	}
	l2Head := eth.L2BlockRef{
		L1Origin: a.ID(),
		Time:     27,
	}

	l1.ExpectL1BlockRefByHash(a.Hash, a, nil)
	confDepthL1 := confdepth.NewConfDepth(10, func() eth.L1BlockRef { return b }, l1)
	s := NewL1OriginSelector(ctx, log, cfg, confDepthL1)

	_, err := s.FindL1Origin(ctx, l2Head)
	require.ErrorContains(t, err, "sequencer time drift")
}

func u64ptr(n uint64) *uint64 {
	return &n
}

// TestOriginSelector_FjordSeqDrift has a similar setup to the previous test
// TestOriginSelectorStrictConfDepth but with Fjord activated at the l1 origin.
// This time the same L1 origin is returned if no new L1 head is seen, instead of an error,
// because the Fjord max sequencer drift is higher.
func TestOriginSelector_FjordSeqDrift(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	log := testlog.Logger(t, log.LevelCrit)
	cfg := &rollup.Config{
		MaxSequencerDrift: 8,
		BlockTime:         2,
		FjordTime:         u64ptr(20), // a's timestamp
	}
	l1 := &testutils.MockL1Source{}
	defer l1.AssertExpectations(t)
	a := eth.L1BlockRef{
		Hash:   common.Hash{'a'},
		Number: 10,
		Time:   20,
	}
	l2Head := eth.L2BlockRef{
		L1Origin: a.ID(),
		Time:     27, // next L2 block time would be past pre-Fjord seq drift
	}

	s := NewL1OriginSelector(ctx, log, cfg, l1)
	s.currentOrigin = a

	next, err := s.FindL1Origin(ctx, l2Head)
	require.NoError(t, err, "with Fjord activated, have increased max seq drift")
	require.Equal(t, a, next)
}

// TestOriginSelectorSeqDriftRespectsNextOriginTime
//
// There are 2 L1 blocks at time 20 & 100. The L2 Head is at time 27.
// The next L2 time is 29. Even though the next L2 time is past the seq
// drift, the origin should remain on block `a` because the next origin's
// time is greater than the next L2 time.
func TestOriginSelectorSeqDriftRespectsNextOriginTime(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	log := testlog.Logger(t, log.LevelCrit)
	cfg := &rollup.Config{
		MaxSequencerDrift: 8,
		BlockTime:         2,
	}
	l1 := &testutils.MockL1Source{}
	defer l1.AssertExpectations(t)
	a := eth.L1BlockRef{
		Hash:   common.Hash{'a'},
		Number: 10,
		Time:   20,
	}
	b := eth.L1BlockRef{
		Hash:       common.Hash{'b'},
		Number:     11,
		Time:       100,
		ParentHash: a.Hash,
	}
	l2Head := eth.L2BlockRef{
		L1Origin: a.ID(),
		Time:     27,
	}

	s := NewL1OriginSelector(ctx, log, cfg, l1)
	s.currentOrigin = a
	s.nextOrigin = b

	next, err := s.FindL1Origin(ctx, l2Head)
	require.Nil(t, err)
	require.Equal(t, a, next)
}

// TestOriginSelectorSeqDriftRespectsNextOriginTimeNoCache
//
// There are 2 L1 blocks at time 20 & 100. The L2 Head is at time 27.
// The next L2 time is 29. Even though the next L2 time is past the seq
// drift, the origin should remain on block `a` because the next origin's
// time is greater than the next L2 time.
// The L1OriginSelector does not have the next origin cached, and must fetch it
// because the max sequencer drift has been exceeded.
func TestOriginSelectorSeqDriftRespectsNextOriginTimeNoCache(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	log := testlog.Logger(t, log.LevelCrit)
	cfg := &rollup.Config{
		MaxSequencerDrift: 8,
		BlockTime:         2,
	}
	l1 := &testutils.MockL1Source{}
	defer l1.AssertExpectations(t)
	a := eth.L1BlockRef{
		Hash:   common.Hash{'a'},
		Number: 10,
		Time:   20,
	}
	b := eth.L1BlockRef{
		Hash:       common.Hash{'b'},
		Number:     11,
		Time:       100,
		ParentHash: a.Hash,
	}
	l2Head := eth.L2BlockRef{
		L1Origin: a.ID(),
		Time:     27,
	}

	l1.ExpectL1BlockRefByNumber(b.Number, b, nil)

	s := NewL1OriginSelector(ctx, log, cfg, l1)
	s.currentOrigin = a

	next, err := s.FindL1Origin(ctx, l2Head)
	require.Nil(t, err)
	require.Equal(t, a, next)
}

// TestOriginSelectorHandlesLateL1Blocks tests the forced repeat of the previous origin,
// but with a conf depth that first prevents it from learning about the need to repeat.
//
// There are 2 L1 blocks at time 20 & 100. The L2 Head is at time 27.
// The next L2 time is 29. Even though the next L2 time is past the seq
// drift, the origin should remain on block `a` because the next origin's
// time is greater than the next L2 time.
// Due to a conf depth of 2, block `b` is not immediately visible,
// and the origin selection should fail until it is visible, by waiting for block `c`.
func TestOriginSelectorHandlesLateL1Blocks(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	log := testlog.Logger(t, log.LevelCrit)
	cfg := &rollup.Config{
		MaxSequencerDrift: 8,
		BlockTime:         2,
	}
	l1 := &testutils.MockL1Source{}
	defer l1.AssertExpectations(t)
	a := eth.L1BlockRef{
		Hash:   common.Hash{'a'},
		Number: 10,
		Time:   20,
	}
	b := eth.L1BlockRef{
		Hash:       common.Hash{'b'},
		Number:     11,
		Time:       100,
		ParentHash: a.Hash,
	}
	c := eth.L1BlockRef{
		Hash:       common.Hash{'c'},
		Number:     12,
		Time:       150,
		ParentHash: b.Hash,
	}
	d := eth.L1BlockRef{
		Hash:       common.Hash{'d'},
		Number:     13,
		Time:       200,
		ParentHash: c.Hash,
	}
	l2Head := eth.L2BlockRef{
		L1Origin: a.ID(),
		Time:     27,
	}

	// l2 head does not change, so we start at the same origin again and again until we meet the conf depth
	l1.ExpectL1BlockRefByHash(a.Hash, a, nil)

	l1.ExpectL1BlockRefByNumber(b.Number, b, nil)

	l1Head := b
	confDepthL1 := confdepth.NewConfDepth(2, func() eth.L1BlockRef { return l1Head }, l1)
	s := NewL1OriginSelector(ctx, log, cfg, confDepthL1)

	_, err := s.FindL1Origin(ctx, l2Head)
	require.ErrorContains(t, err, "sequencer time drift")

	l1Head = c
	_, err = s.FindL1Origin(ctx, l2Head)
	require.ErrorContains(t, err, "sequencer time drift")

	l1Head = d
	next, err := s.FindL1Origin(ctx, l2Head)
	require.Nil(t, err)
	require.Equal(t, a, next, "must stay on a because the L1 time may not be higher than the L2 time")
}

// TestOriginSelectorMiscEvent ensures that the origin selector ignores miscellaneous events,
// but instead returns false to indicate that the event was not handled.
func TestOriginSelectorMiscEvent(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	log := testlog.Logger(t, log.LevelCrit)
	cfg := &rollup.Config{
		MaxSequencerDrift: 8,
		BlockTime:         2,
	}
	l1 := &testutils.MockL1Source{}
	defer l1.AssertExpectations(t)

	s := NewL1OriginSelector(ctx, log, cfg, l1)

	// This event is not handled
	handled := s.OnEvent(context.Background(), rollup.L1TemporaryErrorEvent{})
	require.False(t, handled)
}

// TestOriginSelectorConvergenceToConfDepth tests the evolution of L1 origin selection
// over time as L1 and L2 blocks are produced.
//
// Setup:
// - L1 blocks produced every 12 seconds
// - L2 blocks produced every 2 seconds
// - ConfDepth = 4
// - MaxSequencerDrift = 1800 (large enough to not interfere)
// - Initial state: currentOrigin = L1Head - 1 (block 9, L1Head = block 10)
// - Simulates realistic block production over time
func TestOriginSelectorConvergenceToConfDepth(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	log := testlog.Logger(t, log.LevelCrit)
	cfg := &rollup.Config{
		MaxSequencerDrift: 1800, // Large drift to focus on confDepth behavior
		BlockTime:         2,    // L2 block every 2 seconds
	}

	confDepth := uint64(4) // ConfDepth = 4

	// Create initial 11 L1 blocks: every 12 seconds, starting from block 0
	var l1Blocks []eth.L1BlockRef
	for i := 0; i <= 10; i++ {
		block := eth.L1BlockRef{
			Hash:   common.Hash{byte(i)},
			Number: uint64(i),
			Time:   uint64(i * 12), // Every 12 seconds
		}
		if i > 0 {
			block.ParentHash = l1Blocks[i-1].Hash
		}
		l1Blocks = append(l1Blocks, block)
	}

	l1 := &testutils.MockL1Source{}
	defer l1.AssertExpectations(t)

	// Create confDepth wrapper
	var currentL1Head eth.L1BlockRef = l1Blocks[10] // Start with L1 Head = block 10 (latest)
	confDepthL1 := confdepth.NewConfDepth(confDepth, func() eth.L1BlockRef { return currentL1Head }, l1)

	s := NewL1OriginSelector(ctx, log, cfg, confDepthL1)
	s.currentOrigin = l1Blocks[9] // Initial currentOrigin = block 9

	// Set up initial mock expectations for existing L1 blocks
	for i := 0; i < len(l1Blocks); i++ {
		l1.On("L1BlockRefByNumber", l1Blocks[i].Number).Return(l1Blocks[i], nil).Maybe()
		l1.On("L1BlockRefByHash", l1Blocks[i].Hash).Return(l1Blocks[i], nil).Maybe()
	}

	// Create initial L2 head: L2 time = 120, origin points to block 9
	l2Head := eth.L2BlockRef{
		L1Origin: l1Blocks[9].ID(), // L2 head origin points to block 9
		Time:     uint64(120),      // L2 time = 120
	}

	// Simulate time progression: every 2 seconds for L2, every 12 seconds for L1
	currentTime := uint64(120) // Start at L2 time
	l1BlockNum := 10           // Start with block 10 as L1 head

	t.Logf("=== Starting simulation ===")
	t.Logf("Initial: Time=%d, L1Head=%d, L2Head.Number=%d, L2Head.L1Origin=%d, s.currentOrigin=%d, s.nextOrigin=%d",
		currentTime, currentL1Head.Number, l2Head.Number, l2Head.L1Origin.Number, s.currentOrigin.Number, s.nextOrigin.Number)

	for step := 1; step <= 100; step++ {
		currentTime += 2
		// Every 12 seconds, advance L1 head by 1 block
		if currentTime%12 == 0 {
			l1BlockNum++
			// Create new L1 block
			newL1Block := eth.L1BlockRef{
				Hash:       common.Hash{byte(l1BlockNum)},
				Number:     uint64(l1BlockNum),
				ParentHash: l1Blocks[l1BlockNum-1].Hash,
				Time:       currentTime,
			}
			l1Blocks = append(l1Blocks, newL1Block)
			currentL1Head = newL1Block

			// Set up mock expectations for the new L1 block
			// This block can be fetched by number and by hash
			l1.On("L1BlockRefByNumber", newL1Block.Number).Return(newL1Block, nil).Maybe()
			l1.On("L1BlockRefByHash", newL1Block.Hash).Return(newL1Block, nil).Maybe()
		}

		// Update L2 head time
		l2Head.Time = currentTime
		l2Head.Number++ // Increment L2 block number

		// Find next L1 origin for the next L2 block
		nextOrigin, err := s.FindL1Origin(ctx, l2Head)
		t.Logf("nextOrigin=%d", nextOrigin.Number)
		require.NoError(t, err)

		l2Head.L1Origin = nextOrigin.ID()

		s.onForkchoiceUpdate(l2Head)

		maxUsableBlock := currentL1Head.Number - confDepth
		t.Logf("Step %d: Time=%d, L1Head=%d, L2Head.Number=%d, L2Head.L1Origin=%d, s.currentOrigin=%d, s.nextOrigin=%d, MaxUsable=%d",
			step, currentTime, currentL1Head.Number, l2Head.Number, l2Head.L1Origin.Number, s.currentOrigin.Number, s.nextOrigin.Number, maxUsableBlock)
	}
}

// TestOriginSelectorL1ReorgWithinConfDepth tests that L1 reorgs within confDepth
// do not cause L2 reorgs, demonstrating the protection provided by confDepth.
//
// Setup:
// - L1 blocks produced every 12 seconds
// - L2 blocks produced every 2 seconds
// - ConfDepth = 4
// - MaxSequencerDrift = 1800 (large enough to not interfere)
// - Simulates L1 reorg within confDepth range during runtime
func TestOriginSelectorL1ReorgWithinConfDepth(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	log := testlog.Logger(t, log.LevelCrit)
	cfg := &rollup.Config{
		MaxSequencerDrift: 1800, // Large drift to focus on confDepth behavior
		BlockTime:         2,    // L2 block every 2 seconds
	}

	confDepth := uint64(4) // ConfDepth = 4

	// Create initial 10 L1 blocks: every 12 seconds, starting from block 0
	var l1Blocks []eth.L1BlockRef
	for i := 0; i <= 10; i++ {
		block := eth.L1BlockRef{
			Hash:   common.Hash{byte(i)},
			Number: uint64(i),
			Time:   uint64(i * 12), // Every 12 seconds
		}
		if i > 0 {
			block.ParentHash = l1Blocks[i-1].Hash
		}
		l1Blocks = append(l1Blocks, block)
	}

	l1 := &testutils.MockL1Source{}
	defer l1.AssertExpectations(t)

	// Create confDepth wrapper
	var currentL1Head eth.L1BlockRef = l1Blocks[10] // Start with L1 Head = block 10
	confDepthL1 := confdepth.NewConfDepth(confDepth, func() eth.L1BlockRef { return currentL1Head }, l1)

	s := NewL1OriginSelector(ctx, log, cfg, confDepthL1)
	s.currentOrigin = l1Blocks[6] // Initial currentOrigin = block 6 (L1Head - 4)

	// Set up initial mock expectations for existing L1 blocks
	for i := 0; i < len(l1Blocks); i++ {
		l1.On("L1BlockRefByNumber", l1Blocks[i].Number).Return(l1Blocks[i], nil).Maybe()
		l1.On("L1BlockRefByHash", l1Blocks[i].Hash).Return(l1Blocks[i], nil).Maybe()
	}

	// Create initial L2 head: L2 time = 120, origin points to block 6
	l2Head := eth.L2BlockRef{
		L1Origin: l1Blocks[6].ID(), // L2 head origin points to block 6
		Time:     uint64(120),      // L2 time = 120
		Number:   uint64(60),       // L2 block number = 60 (starting point)
	}

	// Simulate time progression: every 2 seconds for L2, every 12 seconds for L1
	currentTime := uint64(120) // Start at L2 time
	l1BlockNum := 10           // Start with block 10 as L1 head
	reorgTriggered := false    // Flag to track when reorg happens
	var l2BlockNumAtReorg uint64
	var l1OriginAtReorg eth.BlockID

	t.Logf("=== Starting simulation with L1 reorg within confDepth ===")
	t.Logf("Initial: Time=%d, L1Head=%d, L2Head.Number=%d, L2Head.L1Origin=%d, s.currentOrigin=%d, ConfDepth=%d",
		currentTime, currentL1Head.Number, l2Head.Number, l2Head.L1Origin.Number, s.currentOrigin.Number, confDepth)

	for step := 1; step <= 30; step++ {
		currentTime += 2

		// Every 12 seconds, advance L1 head by 1 block
		if currentTime%12 == 0 {
			l1BlockNum++

			// Trigger L1 reorg when we have enough blocks and L2 is stable
			// Reorg affects blocks from (currentL1Head - confDepth) to currentL1Head
			// This ensures the reorg is within confDepth protection range
			if l1BlockNum >= 12 && !reorgTriggered {
				t.Logf("=== L1 Reorg triggered at step %d (L1Head=%d, L2Head.Number=%d, L2Head.L1Origin=%d) ===", step, currentL1Head.Number, l2Head.Number, l2Head.L1Origin.Number)

				// Record state before reorg
				l1OriginAtReorg = l2Head.L1Origin
				l2BlockNumAtReorg = l2Head.Number

				// Calculate reorg range: from (currentL1Head - confDepth + 1) to (currentL1Head + 1)
				reorgStart := currentL1Head.Number - confDepth + 1
				reorgEnd := currentL1Head.Number + 1

				t.Logf("Reorg range: blocks %d to %d (confDepth=%d)", reorgStart, reorgEnd, confDepth)

				// Create L1 reorg: replace blocks in the reorg range with new blocks
				var reorgL1Blocks []eth.L1BlockRef

				// Keep blocks before reorg range unchanged
				for i := 0; i < int(reorgStart); i++ {
					reorgL1Blocks = append(reorgL1Blocks, l1Blocks[i])
				}

				// Create new blocks in reorg range with different hashes
				for i := reorgStart; i <= reorgEnd; i++ {
					newBlock := eth.L1BlockRef{
						Hash:       common.Hash{byte(i + 100)}, // Different hash to simulate reorg
						Number:     uint64(i),
						ParentHash: reorgL1Blocks[len(reorgL1Blocks)-1].Hash,
						Time:       uint64(i * 12),
					}
					reorgL1Blocks = append(reorgL1Blocks, newBlock)
				}

				currentL1Head = reorgL1Blocks[len(reorgL1Blocks)-1] // Use the last block as current head

				// Update mock expectations for all blocks in the reorged chain
				for i := 0; i < len(reorgL1Blocks); i++ {
					l1.On("L1BlockRefByNumber", reorgL1Blocks[i].Number).Return(reorgL1Blocks[i], nil).Maybe()
					l1.On("L1BlockRefByHash", reorgL1Blocks[i].Hash).Return(reorgL1Blocks[i], nil).Maybe()
				}

				// Update confDepth wrapper with new L1 head
				confDepthL1 = confdepth.NewConfDepth(confDepth, func() eth.L1BlockRef { return currentL1Head }, l1)
				s = NewL1OriginSelector(ctx, log, cfg, confDepthL1)
				s.currentOrigin = l1Blocks[l2Head.L1Origin.Number]

				reorgTriggered = true
				t.Logf("L1 reorg completed: L1Head=%d, L2Head.Number=%d, L2Head.L1Origin=%d, currentOrigin=%d", currentL1Head.Number, l2Head.Number, l2Head.L1Origin.Number, s.currentOrigin.Number)
			} else {
				// Normal L1 block creation
				newL1Block := eth.L1BlockRef{
					Hash:       common.Hash{byte(l1BlockNum)},
					Number:     uint64(l1BlockNum),
					ParentHash: l1Blocks[len(l1Blocks)-1].Hash, // Use the last block as parent
					Time:       currentTime,
				}
				l1Blocks = append(l1Blocks, newL1Block)
				currentL1Head = newL1Block

				// Set up mock expectations for the new L1 block
				l1.On("L1BlockRefByNumber", newL1Block.Number).Return(newL1Block, nil).Maybe()
				l1.On("L1BlockRefByHash", newL1Block.Hash).Return(newL1Block, nil).Maybe()
			}
		}

		// Update L2 head time and number
		l2Head.Time = currentTime
		l2Head.Number++ // Increment L2 block number

		// Find next L1 origin for the next L2 block
		nextOrigin, err := s.FindL1Origin(ctx, l2Head)
		require.NoError(t, err)

		l2Head.L1Origin = nextOrigin.ID()
		s.onForkchoiceUpdate(l2Head)

		maxUsableBlock := currentL1Head.Number - confDepth
		t.Logf("Step %d: Time=%d, L1Head=%d, L2Head.Number=%d, L2Head.L1Origin=%d, s.currentOrigin=%d, s.nextOrigin=%d, MaxUsable=%d",
			step, currentTime, currentL1Head.Number, l2Head.Number, l2Head.L1Origin.Number, s.currentOrigin.Number, s.nextOrigin.Number, maxUsableBlock)
	}

	t.Logf("=== Verification ===")
	t.Logf("L2 block at reorg time: %d", l2BlockNumAtReorg)
	t.Logf("L1 origin at reorg time: %d", l1OriginAtReorg.Number)

	postReorgL1OriginBlock, err := l1.L1BlockRefByNumber(ctx, l1OriginAtReorg.Number)
	require.NoError(t, err)

	// The hash should be the same as before reorg (original hash, not the reorged hash)
	require.Equal(t, l1OriginAtReorg.Hash, postReorgL1OriginBlock.Hash,
		"L2 block %d's L1 origin hash should remain unchanged after L1 reorg within confDepth", l2BlockNumAtReorg)

	t.Logf("✅ L1 reorg within confDepth did not cause L2 reorg - test passed!")
}
