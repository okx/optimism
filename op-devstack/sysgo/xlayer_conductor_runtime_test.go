package sysgo

import (
	"context"
	"errors"
	"math/big"
	"net"
	"sort"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/ethclient"
	"github.com/stretchr/testify/require"

	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
)

// stubCloser records that the rollback path released the resource it wraps.
type stubCloser struct {
	stopped bool
}

func (s *stubCloser) Stop() { s.stopped = true }

// TestCheckXLayerFixedPortsAvailable_OccupiedPort verifies that a fixed port which
// is already bound is reported before any resource is created, naming both the
// owning component and the port so the conflict is actionable.
func TestCheckXLayerFixedPortsAvailable_OccupiedPort(t *testing.T) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	require.NoError(t, err)
	defer func() { _ = ln.Close() }()
	port := ln.Addr().(*net.TCPAddr).Port

	plan := &XLayerFixedPortPlan{ELHTTPPorts: [3]int{0, port, 0}}
	err = checkXLayerFixedPortsAvailable(plan)
	require.Error(t, err, "an occupied fixed port must be reported")
	require.Contains(t, err.Error(), "L2 EL sequencer2", "the error must name the owning component")
	require.Contains(t, err.Error(), strconv.Itoa(port), "the error must name the port")
}

// TestCheckXLayerFixedPortsAvailable_CleanupThenReuse verifies that once the holder
// of a fixed port releases it, the same plan becomes valid again — the check leaves
// nothing bound behind, so ports are reusable across test runs.
func TestCheckXLayerFixedPortsAvailable_CleanupThenReuse(t *testing.T) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	require.NoError(t, err)
	port := ln.Addr().(*net.TCPAddr).Port

	plan := &XLayerFixedPortPlan{
		L1HTTPPort:        port,
		ELHTTPPorts:       [3]int{0, 0, 0},
		ConductorRPCPorts: [3]int{0, 0, 0},
	}
	require.Error(t, checkXLayerFixedPortsAvailable(plan), "port is held, so the plan must be rejected")

	require.NoError(t, ln.Close())
	require.NoError(t, checkXLayerFixedPortsAvailable(plan), "the same port must be usable after the holder releases it")
	// A second run in a row proves the check itself does not leak the listener.
	require.NoError(t, checkXLayerFixedPortsAvailable(plan), "the check must not leave the probed port bound")
}

// TestCheckXLayerFixedPortsAvailable_ConductorPort verifies that a conflict on a
// conductor RPC port is attributed to the conductor, not to an EL.
func TestCheckXLayerFixedPortsAvailable_ConductorPort(t *testing.T) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	require.NoError(t, err)
	defer func() { _ = ln.Close() }()
	port := ln.Addr().(*net.TCPAddr).Port

	plan := &XLayerFixedPortPlan{ConductorRPCPorts: [3]int{0, 0, port}}
	err = checkXLayerFixedPortsAvailable(plan)
	require.Error(t, err)
	require.Contains(t, err.Error(), "conductor conductor3")
	require.Contains(t, err.Error(), strconv.Itoa(port))
}

// TestCheckXLayerFixedPortsAvailable_NilPlan verifies that the random-port path is
// untouched: a plan that fixes nothing has nothing to check.
func TestCheckXLayerFixedPortsAvailable_NilPlan(t *testing.T) {
	require.NoError(t, checkXLayerFixedPortsAvailable(nil))
	require.NoError(t, checkXLayerFixedPortsAvailable(&XLayerFixedPortPlan{}))
}

// TestStartXLayerSequencerNodeWithRollsBackELOnCLFailure verifies the partial-start
// contract: when the CL step fails, the EL that the first step already started is
// closed, so a half-started node leaves no running process or bound listener behind.
func TestStartXLayerSequencerNodeWithRollsBackELOnCLFailure(t *testing.T) {
	el := &stubCloser{}
	f := xlayerSequencerStartFuncs{
		startEL: func(string, int, PresetConfig) (L2ELNode, xlayerClosable, error) {
			return nil, el, nil
		},
		startCL: func(string, L2ELNode, PresetConfig) (*OpNode, error) {
			return nil, errors.New("CL startup failed")
		},
	}

	node, cl, err := startXLayerSequencerNodeWith("sequencer2", 1, PresetConfig{}, f)
	require.Error(t, err, "a failing CL step must surface as an error")
	require.Nil(t, node, "a failed node must not be registered")
	require.Nil(t, cl)
	require.True(t, el.stopped, "the already-started EL must be closed when the CL step fails")
	require.Contains(t, err.Error(), "sequencer2")
}

// TestStartXLayerSequencerNodeWithELFailureStartsNothing verifies that a failure in
// the first step allocates nothing and therefore has nothing to roll back.
func TestStartXLayerSequencerNodeWithELFailureStartsNothing(t *testing.T) {
	el := &stubCloser{}
	clStarted := false
	f := xlayerSequencerStartFuncs{
		startEL: func(string, int, PresetConfig) (L2ELNode, xlayerClosable, error) {
			return nil, el, errors.New("EL port in use")
		},
		startCL: func(string, L2ELNode, PresetConfig) (*OpNode, error) {
			clStarted = true
			return nil, nil
		},
	}

	node, cl, err := startXLayerSequencerNodeWith("sequencer3", 2, PresetConfig{}, f)
	require.Error(t, err)
	require.Nil(t, node)
	require.Nil(t, cl)
	require.False(t, clStarted, "the CL step must not run once the EL step has failed")
	require.False(t, el.stopped, "nothing was started, so there is nothing to close")
	require.Contains(t, err.Error(), "EL port in use")
}

// TestStartXLayerSequencerNodeWithSuccess verifies that a node is registered only
// after both startup steps have succeeded.
func TestStartXLayerSequencerNodeWithSuccess(t *testing.T) {
	el := &stubCloser{}
	f := xlayerSequencerStartFuncs{
		startEL: func(string, int, PresetConfig) (L2ELNode, xlayerClosable, error) {
			return nil, el, nil
		},
		startCL: func(string, L2ELNode, PresetConfig) (*OpNode, error) {
			return nil, nil
		},
	}

	node, cl, err := startXLayerSequencerNodeWith("sequencer2", 1, PresetConfig{}, f)
	require.NoError(t, err)
	require.NotNil(t, node, "a fully started node must be returned")
	require.Nil(t, cl)
	require.False(t, el.stopped, "a successful start must not close the EL")
	require.True(t, node.IsSequencer)
	require.Equal(t, "sequencer2", node.Name)
}

// xlayerConductorRuntimes returns the sequencer EL backends of a fully
// constructed XLayer conductor runtime, keyed by node name. The topology
// registers the primary and both additional sequencers in runtime.Nodes.
func xlayerConductorRuntimes(runtime *SingleChainRuntime) map[string]L2ELNode {
	out := make(map[string]L2ELNode, len(runtime.Nodes))
	for name, node := range runtime.Nodes {
		out[name] = node.EL
	}
	return out
}

// TestXLayerConductorColdStartSeed verifies the phased cold start of the XLayer
// conductor cluster end to end:
//
//  1. the cluster comes up with exactly one Raft leader and three voting members;
//  2. the leader reads back a non-nil committed unsafe payload — the FSM was
//     seeded, not left empty (a fresh Raft with no committed payload would return
//     nil here, and the sequencer could not start);
//  3. the seeded payload is a real block above the non-zero genesis height, and
//     all three sequencer ELs carry that exact block at that exact height, so the
//     cluster converged on the seeded head rather than merely reporting a number;
//  4. after the conductors are resumed the unsafe head keeps advancing past the
//     seed, which is only possible once the seed made the sequencer startable.
//
// The frozen head is read from the leader's committed payload rather than captured
// mid-construction: the constructor resumes the conductors before it returns, so a
// head sampled inside the paused window is not observable from outside. Everything
// asserted here is read from the running cluster.
func TestXLayerConductorColdStartSeed(t *testing.T) {
	dt := devtest.SerialT(t)
	runtime := NewXLayerWithConductorsRuntimeWithConfig(dt, PresetConfig{})
	require.Len(t, runtime.Conductors, 3, "expected three conductors")
	els := xlayerConductorRuntimes(runtime)
	require.Len(t, els, 3, "expected three sequencer EL backends")

	// All three candidates must be built from ONE recipe. A backup is a leadership
	// candidate, so it may not be a generic follower: whatever execution binary and
	// producer configuration the primary runs, the backups run too. This assertion
	// is what catches a topology whose backups could not take over.
	producerCandidates := make(map[string]bool, len(els))
	binaries := make(map[string]string, len(els))
	for name, el := range els {
		opReth, ok := el.(*OpReth)
		require.Truef(t, ok, "candidate %s must run the execution client", name)
		joined := strings.Join(opReth.args, " ")
		hasProducer := strings.Contains(joined, "--xlayer.sequencer-mode") &&
			strings.Contains(joined, "--flashblocks.enabled")
		producerCandidates[name] = hasProducer
		binaries[name] = opReth.execPath
		t.Logf("candidate %s binary=%s producerConfig=%t", name, opReth.execPath, hasProducer)
	}

	names := make([]string, 0, len(els))
	for name := range els {
		names = append(names, name)
	}
	sort.Strings(names)

	for _, name := range names[1:] {
		require.Equalf(t, binaries[names[0]], binaries[name],
			"candidate %s runs a different execution binary than %s", name, names[0])
		require.Equalf(t, producerCandidates[names[0]], producerCandidates[name],
			"candidate %s carries a different producer configuration than %s", name, names[0])
	}

	if !producerCandidates[names[0]] {
		// The preset did not configure an XLayer execution binary, so the
		// producer/flashblocks flags are not applicable: the generic client
		// rejects them outright and would not start. Recorded rather than
		// asserted, so the evidence states which binary was actually exercised.
		t.Logf("XLayer producer configuration not applied: no XLayer execution binary configured")
		return
	}
	for _, name := range names {
		require.Truef(t, producerCandidates[name],
			"candidate %s must carry the XLayer producer configuration", name)
	}

	ctx, cancel := context.WithTimeout(dt.Ctx(), 120*time.Second)
	defer cancel()

	// 1. Exactly one leader, three voting members.
	var leader *Conductor
	for name, c := range runtime.Conductors {
		if c.service.Leader(ctx) {
			require.Nilf(t, leader, "more than one conductor claimed to be Raft leader (%s)", name)
			leader = c
		}
	}
	require.NotNil(t, leader, "exactly one conductor must be the Raft leader")

	for name, c := range runtime.Conductors {
		m, err := c.service.ClusterMembership(ctx)
		require.NoErrorf(t, err, "read cluster membership from %s", name)
		require.Len(t, m.Servers, 3, "membership must contain all three conductors")
	}

	// 2. The leader reads back the committed unsafe payload it seeded the cluster
	//    with. The read is strongly consistent, so only the leader can serve it;
	//    a follower returning ErrNotLeader is the expected Raft behaviour, not a
	//    missing seed. A nil payload here would mean the FSM was never seeded.
	payload, err := leader.service.LatestUnsafePayload(ctx)
	require.NoError(t, err, "read back the committed unsafe payload from the leader")
	require.NotNil(t, payload, "the leader must have a committed unsafe payload, not an empty FSM")

	seedHash := common.Hash(payload.ExecutionPayload.BlockHash)
	seedNumber := uint64(payload.ExecutionPayload.BlockNumber)

	// 3. The seeded payload is a real block above the non-zero genesis, and all
	//    three sequencer ELs carry that exact block at that height — the cluster
	//    converged on the seeded head instead of merely reporting a height.
	t.Logf("seeded unsafe head: number=%d hash=%s", seedNumber, seedHash)
	require.Greater(t, seedNumber, XLayerDefaultL2GenesisHeight,
		"the seed must be a block produced above the non-zero XLayer genesis height")

	for name, el := range xlayerConductorRuntimes(runtime) {
		elCtx, elCancel := context.WithTimeout(ctx, 60*time.Second)
		client, err := ethclient.DialContext(elCtx, el.UserRPC())
		require.NoErrorf(t, err, "dial %s EL", name)

		elName, elClient, seed, seedHeight := name, client, seedHash, seedNumber
		require.Eventuallyf(t, func() bool {
			latest, err := elClient.BlockNumber(elCtx)
			if err != nil {
				t.Logf("EL %s latest block number unavailable: %v", elName, err)
				return false
			}
			if latest < seedHeight {
				t.Logf("EL %s head %d is behind the seeded height %d", elName, latest, seedHeight)
				return false
			}
			block, err := elClient.BlockByNumber(elCtx, new(big.Int).SetUint64(seedHeight))
			if err != nil {
				t.Logf("EL %s failed to read block %d: %v", elName, seedHeight, err)
				return false
			}
			if block.Hash() != seed {
				t.Logf("EL %s block %d hash %s != seeded %s", elName, seedHeight, block.Hash(), seed)
				return false
			}
			return true
		}, 60*time.Second, 500*time.Millisecond,
			"EL %s must carry the seeded block %s at height %d", elName, seed, seedHeight)
		elCancel()
		elClient.Close()
	}

	// 4. Block production continues past the seed after the resume.
	primaryCtx, primaryCancel := context.WithTimeout(ctx, 60*time.Second)
	defer primaryCancel()
	primary, err := ethclient.DialContext(primaryCtx, runtime.L2EL.UserRPC())
	require.NoError(t, err, "dial the primary EL")
	defer primary.Close()

	require.Eventually(t, func() bool {
		latest, err := primary.BlockNumber(primaryCtx)
		if err != nil {
			return false
		}
		return latest > seedNumber
	}, 60*time.Second, 500*time.Millisecond,
		"the unsafe head must advance past the seed once the conductors are resumed")
}
