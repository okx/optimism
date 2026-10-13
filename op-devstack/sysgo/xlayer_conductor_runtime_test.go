package sysgo

import (
	"context"
	"errors"
	"math/big"
	"net"
	"os"
	"os/exec"
	"path/filepath"
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

// requireXLayerExecutionBinary resolves the XLayer execution client from
// XLAYER_RETH_BIN and records the version it reports.
//
// The producer and flashblocks flags this acceptance path depends on exist only
// in that build. When the binary is absent or cannot report a version, the
// caller is blocked: the test skips with an explicit reason and can never report
// a pass, because a green XLayer-named test that never exercised an XLayer
// client would be worse than no test at all.
func requireXLayerExecutionBinary(t *testing.T) (path, version string) {
	t.Helper()
	path = os.Getenv("XLAYER_RETH_BIN")
	if path == "" {
		t.Skip("BLOCKED: XLAYER_RETH_BIN not available — cannot validate XLayer acceptance")
	}
	out, err := exec.Command(path, "--version").CombinedOutput()
	if err != nil {
		t.Skipf("BLOCKED: XLAYER_RETH_BIN at %s cannot report a version: %v (%s)",
			path, err, strings.TrimSpace(string(out)))
	}
	version = strings.TrimSpace(string(out))
	if version == "" {
		t.Skipf("BLOCKED: XLAYER_RETH_BIN at %s reported an empty version string", path)
	}
	return path, version
}

// stubCloser records that the rollback path released the resource it wraps.
type stubCloser struct {
	stopped bool
}

func (s *stubCloser) Stop() { s.stopped = true }

// listenerCloser is a started resource whose lifetime is a REAL bound socket
// rather than a struct field. The rollback contract is about releasing a
// process and the listener it holds, so the closest stand-in available without
// the XLayer execution binary is an actual listener on an actual port: a
// rollback that forgets to close it leaves the port genuinely unbindable, and
// the test observes that instead of taking a boolean's word for it.
type listenerCloser struct {
	name string
	ln   net.Listener
	port int
}

func newListenerCloser(t *testing.T, name string) *listenerCloser {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	require.NoError(t, err, "the test must be able to claim a real port")
	return &listenerCloser{name: name, ln: ln, port: ln.Addr().(*net.TCPAddr).Port}
}

func (l *listenerCloser) Stop() { _ = l.ln.Close() }

// portIsFree reports whether a port can be bound right now.
func portIsFree(port int) bool {
	ln, err := net.Listen("tcp", net.JoinHostPort("127.0.0.1", strconv.Itoa(port)))
	if err != nil {
		return false
	}
	_ = ln.Close()
	return true
}

// TestStartXLayerSecondaryCandidatesRollsBackRealListeners verifies the partial
// start unwind over real sockets: the first candidate claims an actual port, the
// second fails, and the port the first one claimed must be genuinely free again.
// A rollback that only flipped a flag on a stub passes a stubbed test and fails
// this one, which is the point of driving it with a real listener.
func TestStartXLayerSecondaryCandidatesRollsBackRealListeners(t *testing.T) {
	first := newListenerCloser(t, "sequencer2")

	var calls []string
	_, _, _, err := startXLayerSecondaryCandidates(
		[]string{"sequencer2", "sequencer3"},
		func(name string, _ int) (*SingleChainNodeRuntime, *OpNode, xlayerClosable, error) {
			calls = append(calls, name)
			if name == "sequencer2" {
				return nil, nil, first, nil
			}
			return nil, nil, nil, errors.New("the tertiary candidate refused its HTTP port")
		})

	require.Error(t, err, "a failing later candidate must surface as an error")
	require.Equal(t, []string{"sequencer2", "sequencer3"}, calls,
		"candidates must be started in order, and the failure must not stop the unwind")
	require.Contains(t, err.Error(), "sequencer3", "the error must name the candidate that failed")
	require.Truef(t, portIsFree(first.port),
		"port %d claimed by the first candidate must be free after the later candidate failed", first.port)
}

// TestStartXLayerSecondaryCandidatesKeepsListenersOnSuccess verifies the other
// half of the contract: when every candidate starts, none of their listeners is
// released. Without this, a rollback that closed everything unconditionally
// would pass the failure test above and destroy a healthy topology.
func TestStartXLayerSecondaryCandidatesKeepsListenersOnSuccess(t *testing.T) {
	first := newListenerCloser(t, "sequencer2")
	second := newListenerCloser(t, "sequencer3")

	nodes, cls, closers, err := startXLayerSecondaryCandidates(
		[]string{"sequencer2", "sequencer3"},
		func(name string, _ int) (*SingleChainNodeRuntime, *OpNode, xlayerClosable, error) {
			if name == "sequencer2" {
				return nil, nil, first, nil
			}
			return nil, nil, second, nil
		})

	require.NoError(t, err)
	require.Len(t, nodes, 2)
	require.Len(t, cls, 2)
	require.Len(t, closers, 2)
	require.Falsef(t, portIsFree(first.port),
		"port %d must still be claimed after a successful bring-up", first.port)
	require.Falsef(t, portIsFree(second.port),
		"port %d must still be claimed after a successful bring-up", second.port)
}

// TestCloseXLayerClosersReleasesInReverseOrder verifies that the unwind releases
// the resources in reverse acquisition order and tolerates nil handles, so a
// caller can pass the result of a failed step without a guard.
func TestCloseXLayerClosersReleasesInReverseOrder(t *testing.T) {
	var order []string
	recorder := func(name string) *recordingCloser {
		return &recordingCloser{name: name, order: &order}
	}

	closeXLayerClosers(recorder("first"), nil, recorder("third"), recorder("second"))

	require.Equal(t, []string{"second", "third", "first"}, order,
		"resources must be released in reverse acquisition order")
}

// recordingCloser appends its name to a shared order slice when released.
type recordingCloser struct {
	name  string
	order *[]string
}

func (r *recordingCloser) Stop() { *r.order = append(*r.order, r.name) }

// TestXLayerExecutionBinaryUsable verifies that a non-empty path is not by
// itself accepted as an execution binary. The producer flags may only be
// attached to something that can actually be launched.
func TestXLayerExecutionBinaryUsable(t *testing.T) {
	dir := t.TempDir()

	executable := filepath.Join(dir, "op-reth")
	require.NoError(t, os.WriteFile(executable, []byte("#!/bin/sh\n"), 0o755))
	require.True(t, xlayerExecutionBinaryUsable(executable), "an executable regular file is usable")

	notExecutable := filepath.Join(dir, "not-executable")
	require.NoError(t, os.WriteFile(notExecutable, []byte("#!/bin/sh\n"), 0o644))
	require.False(t, xlayerExecutionBinaryUsable(notExecutable), "a file without an execute bit is not usable")

	require.False(t, xlayerExecutionBinaryUsable(""), "an empty path is not a binary")
	require.False(t, xlayerExecutionBinaryUsable(dir), "a directory is not a binary")
	require.False(t, xlayerExecutionBinaryUsable(filepath.Join(dir, "missing")), "a missing path is not a binary")
}

// TestXLayerRethBinaryProbeWrongBinary verifies that a program which merely
// exists and exits zero is NOT accepted as an execution client.
//
// Two shapes of "not a client" are covered, because they fail at different
// checks and a probe that only had one of them would accept the other:
//
//   - a program that prints nothing at all, which must be rejected for having no
//     version (its exit status alone is zero and therefore worthless as evidence);
//   - a real program from the system that does report a version about itself,
//     which must be rejected because it does not advertise the flags this
//     topology passes its execution client.
//
// The second case is the one an existence check cannot catch: there genuinely is
// an executable there, and it genuinely answers --version.
func TestXLayerRethBinaryProbeWrongBinary(t *testing.T) {
	dir := t.TempDir()

	silent := filepath.Join(dir, "silent")
	require.NoError(t, os.WriteFile(silent, []byte("#!/bin/sh\nexit 0\n"), 0o755))
	probe, err := probeXLayerExecutionBinary(context.Background(), silent, "test silent program")
	require.Error(t, err, "a program that reports no version must not be accepted as an execution client")
	require.Contains(t, err.Error(), "cannot determine binary version",
		"the failure must name the missing version, got: %v", err)
	require.Empty(t, probe.Version, "no version may be recorded for a program that reported none")

	impostor := filepath.Join(dir, "not-a-client")
	require.NoError(t, os.Symlink("/bin/true", impostor), "the test needs a real program that is not a client")
	probe, err = probeXLayerExecutionBinary(context.Background(), impostor, "test impostor")
	require.Error(t, err, "a program that lacks the expected flags must not be accepted")
	require.Contains(t, err.Error(), "does not advertise", "got: %v", err)
}

// TestXLayerRethBinaryProbeRejectsNonExecutable verifies the existence half of
// the contract on its own: the version probe and the file check are independent,
// and neither may stand in for the other.
func TestXLayerRethBinaryProbeRejectsNonExecutable(t *testing.T) {
	dir := t.TempDir()

	plain := filepath.Join(dir, "plain-file")
	require.NoError(t, os.WriteFile(plain, []byte("not a program\n"), 0o644))
	_, err := probeXLayerExecutionBinary(context.Background(), plain, "test non-executable")
	require.Error(t, err)
	require.Contains(t, err.Error(), "not an executable regular file", "got: %v", err)

	_, err = probeXLayerExecutionBinary(context.Background(), dir, "test directory")
	require.Error(t, err)
	require.Contains(t, err.Error(), "not an executable regular file", "got: %v", err)

	_, err = probeXLayerExecutionBinary(context.Background(), filepath.Join(dir, "missing"), "test missing")
	require.Error(t, err)
	require.Contains(t, err.Error(), "not an executable regular file", "got: %v", err)
}

// TestXLayerRethBinaryProbe verifies the probe against the execution client the
// preset actually selected. Without that binary there is nothing to probe, so
// the test reports BLOCKED rather than passing on an unverified program.
func TestXLayerRethBinaryProbe(t *testing.T) {
	binPath, _ := requireXLayerExecutionBinary(t)

	cfg := PresetConfig{OpRethOptions: []OpRethOption{OpRethWithBinary(binPath)}}
	ctx, cancel := context.WithTimeout(t.Context(), 60*time.Second)
	defer cancel()

	probe, err := resolveXLayerExecutionBinary(ctx, devtest.SerialT(t), cfg)
	require.NoError(t, err, "the configured execution client must pass the probe")
	require.Equal(t, binPath, probe.Path, "the probe must report the binary the preset selected")
	require.NotEmpty(t, probe.Version, "the probe must record a version string")
	require.ElementsMatch(t, xlayerExpectedExecutionFlags, probe.Flags,
		"the probe must confirm every flag this topology configures")
	t.Logf("execution binary probe: path=%s source=%s version=%s flags=%v",
		probe.Path, probe.Source, probe.Version, probe.Flags)
}

// TestResolveXLayerExecutionBinaryWithoutSelection verifies that a preset which
// selected no binary at all is reported as such, instead of the probe silently
// inventing a default.
func TestResolveXLayerExecutionBinaryWithoutSelection(t *testing.T) {
	_, err := resolveXLayerExecutionBinary(context.Background(), devtest.SerialT(t), PresetConfig{})
	require.Error(t, err)
	require.Contains(t, err.Error(), "no execution binary was selected", "got: %v", err)
}

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
// The ordering half of the contract is enforced by the constructor this test
// builds, and each assertion below is what makes it observable from outside:
//
//   - the head is frozen as one (number, hash) observation while NO conductor
//     exists, and every candidate is converged onto it first
//     (`freezeXLayerSeed`);
//   - the attach is deliberately delayed and the elapsed time is asserted
//     (`xlayerConductorAttachDelay`, asserted at the attach site);
//   - the Raft state machine is seeded with that frozen payload, passed INTO the
//     seeding step rather than read inside it (`startXLayerConductorClusterSeeded`
//     takes the seed as a parameter, so a post-Resume read is not expressible);
//   - the readback and the pre-Resume per-candidate sample both compare against
//     the same frozen pair, and Resume follows only after both succeed;
//   - an assertion inside the constructor is a real gate here: a regression in the
//     ordering fails THIS test, because the test is the construction.
//
// The frozen head is read from the leader's committed payload rather than captured
// mid-construction: the constructor resumes the conductors before it returns, so a
// head sampled inside the paused window is not observable from outside. Everything
// asserted here is read from the running cluster.
func TestXLayerConductorColdStartSeed(t *testing.T) {
	// The producer and flashblocks flags this acceptance gate depends on belong to
	// the XLayer build of the execution client, which is supplied out of band. A
	// run without it cannot exercise the XLayer producer path, so it must report
	// blocked rather than pass: a green XLayer-named test that never touched an
	// XLayer client would be worse than no test at all.
	binPath, binVersion := requireXLayerExecutionBinary(t)
	t.Logf("XLayer execution binary: path=%s version=%s", binPath, binVersion)

	cfg := PresetConfig{
		OpRethOptions: []OpRethOption{OpRethWithBinary(binPath)},
	}
	dt := devtest.SerialT(t)
	runtime := NewXLayerWithConductorsRuntimeWithConfig(dt, cfg)
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

	// The binary is present (gated above), so every candidate must actually carry
	// the producer configuration. A candidate that quietly fell back to a plain
	// follower would be unable to take over sequencing.
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
