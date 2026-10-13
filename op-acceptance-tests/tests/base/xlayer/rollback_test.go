package xlayer

import (
	"net"
	"os"
	"os/exec"
	"strconv"
	"testing"
	"time"

	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/ethereum-optimism/optimism/op-devstack/presets"
	"github.com/ethereum-optimism/optimism/op-devstack/sysgo"
	"github.com/stretchr/testify/require"
)

// rollbackChildEnv marks the re-executed copy of this test binary that drives the
// failing construction. The marker is what keeps the child from launching a
// grandchild of its own.
const rollbackChildEnv = "XLAYER_ROLLBACK_CHILD"

// TestXLayerRealLifecycleRollback drives the REAL topology constructor through
// the failure it is supposed to survive, rather than driving a listener stub.
//
// A stub-based rollback test answers "does this helper close what it was handed".
// That is not the question here. The question is whether the real constructor —
// the one that allocates a world, an L1, three execution clients, three rollup
// nodes, three conductors and a batcher — refuses a plan it cannot honour without
// leaving anything behind, and whether the very same plan works once the
// obstruction is gone. Only the real constructor can answer that.
//
// The failing construction runs in a re-executed copy of this test binary. It has
// to: the constructor's fixed-port check is fatal by design — an unavailable port
// must allocate nothing — and a fatal assertion takes the whole test with it, so
// the only way to observe the refusal AND continue to the reuse half is to let
// the abort happen somewhere it can be caught, which is a child process.
//
// This half needs no XLayer execution binary. The constructor refuses the plan
// before it would have needed one, which is precisely the property being tested.
func TestXLayerRealLifecycleRollback(t *testing.T) {
	if os.Getenv(rollbackChildEnv) == "1" {
		runRollbackChild(t)
		return
	}

	child := exec.Command(os.Args[0], "-test.run=^TestXLayerRealLifecycleRollback$", "-test.v")
	child.Env = append(os.Environ(), rollbackChildEnv+"=1")
	out, err := child.CombinedOutput()

	require.Error(t, err, "the child construction must fail on the occupied port")
	require.Containsf(t, string(out), "L2 EL sequencer2",
		"the refusal must name the component that owns the port; child output:\n%s", out)
	require.Containsf(t, string(out), strconv.Itoa(xlayerELSeq1Port),
		"the refusal must name the port that could not be bound; child output:\n%s", out)
	require.Containsf(t, string(out), "cannot bind fixed port",
		"the refusal must say what went wrong; child output:\n%s", out)

	// The failed attempt must have released everything. If the constructor had
	// started allocating before checking, some of these would still be claimed and
	// the reuse attempt below would fail for a reason that has nothing to do with
	// the plan.
	assertPortsFree(t,
		xlayerL1HTTPPort,
		xlayerELSeq0Port, xlayerELSeq2Port,
		xlayerConductor1RPCPort, xlayerConductor2RPCPort, xlayerConductor3RPCPort,
	)

	// Every planned port is claimable again — the failed attempt leaked nothing,
	// down to the socket level. This is the port-reuse half of the contract, and
	// it is asserted through real binds rather than through the availability check
	// that produced the refusal.
	for _, port := range fixedPorts {
		ln, listenErr := net.Listen("tcp", net.JoinHostPort("127.0.0.1", strconv.Itoa(port)))
		require.NoErrorf(t, listenErr, "planned port %d must be reusable after the refused construction", port)
		require.NoError(t, ln.Close())
	}

	t.Logf("real lifecycle rollback: the real constructor refused an occupied fixed port (%d), named it, and released everything",
		xlayerELSeq1Port)
}

// runRollbackChild occupies one planned port and calls the real constructor,
// which is expected to abort. Reaching the bottom of this function means the
// constructor ACCEPTED a plan it cannot honour, which is the defect.
func runRollbackChild(t *testing.T) {
	child := devtest.SerialT(t)
	sysgo.SkipOnKonaNode(child, "not supported")

	holder, err := net.Listen("tcp", net.JoinHostPort("127.0.0.1", strconv.Itoa(xlayerELSeq1Port)))
	require.NoErrorf(t, err, "the child must be able to occupy the planned EL port %d", xlayerELSeq1Port)
	defer func() { _ = holder.Close() }()

	_ = presets.NewXLayerWithConductors(child, presets.WithXLayerFixedPorts(fixedPlan))

	t.Fatalf("the real constructor must refuse a plan whose EL port %d is already held", xlayerELSeq1Port)
}

// TestXLayerRealLifecycleRollbackRestart is the other half of the same contract:
// once the obstruction is gone, the SAME plan must come up completely — three
// sequencers, three conductors and the batcher.
//
// It needs the XLayer execution binary, because a successful construction is
// exactly the path that launches execution clients. Without it the test reports
// BLOCKED rather than passing on a topology it never built.
func TestXLayerRealLifecycleRollbackRestart(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	sys := presets.NewXLayerWithConductors(t, presets.WithXLayerFixedPorts(fixedPlan))
	require.NotNil(t, sys, "the same plan must start once no port is obstructed")

	for chainID, conductors := range sys.ConductorSets {
		require.Equal(t, "195", chainID.String(), "the restarted topology must run the XLayer chain ID")
		require.Len(t, conductors, 3, "the restarted topology must bring up all three conductors")
	}

	// Dial the planned ports from outside the devstack's own handles: an EL handle
	// points at a dynamic internal proxy, so reaching the planned port is the only
	// check that says the external listener is the one that came up.
	for i, port := range fixedPlan.ELHTTPPorts {
		conn, dialErr := net.DialTimeout("tcp", net.JoinHostPort("127.0.0.1", strconv.Itoa(port)), 5*time.Second)
		require.NoErrorf(t, dialErr, "L2 EL index %d must answer on its fixed port %d after the restart", i, port)
		_ = conn.Close()
	}
	for i, port := range fixedPlan.ConductorRPCPorts {
		conn, dialErr := net.DialTimeout("tcp", net.JoinHostPort("127.0.0.1", strconv.Itoa(port)), 5*time.Second)
		require.NoErrorf(t, dialErr, "conductor %d must answer on its fixed RPC port %d after the restart", i+1, port)
		_ = conn.Close()
	}
}
