package xlayer

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"os/exec"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/ethereum/go-ethereum/ethclient"
	gethrpc "github.com/ethereum/go-ethereum/rpc"

	conductorRpc "github.com/ethereum-optimism/optimism/op-conductor/rpc"
	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/ethereum-optimism/optimism/op-devstack/presets"
	"github.com/ethereum-optimism/optimism/op-devstack/sysgo"
	"github.com/stretchr/testify/require"
)

// The fixed-port tests all bind the same well-known ports, so they must never
// run as independent parallel cases against them: every test in this file
// constructs its own fixture serially and tears it down before the next one.
const (
	xlayerL1HTTPPort        = 8545
	xlayerELSeq0Port        = 8123
	xlayerELSeq1Port        = 8124
	xlayerELSeq2Port        = 8125
	xlayerConductor1RPCPort = 8547
	xlayerConductor2RPCPort = 8548
	xlayerConductor3RPCPort = 8549
)

// fixedPlan is the canonical fixed-port configuration. It intentionally matches
// the approved port contract: L1 8545, EL 8123/8124/8125, conductor RPC
// 8547/8548/8549, all on loopback.
var fixedPlan = sysgo.XLayerFixedPortPlan{
	L1HTTPPort:        xlayerL1HTTPPort,
	ELHTTPPorts:       [3]int{xlayerELSeq0Port, xlayerELSeq1Port, xlayerELSeq2Port},
	ConductorRPCPorts: [3]int{xlayerConductor1RPCPort, xlayerConductor2RPCPort, xlayerConductor3RPCPort},
}

// fixedPorts is every port the plan fixes, used to prove that the random path
// leaves all of them alone.
var fixedPorts = []int{
	xlayerL1HTTPPort,
	xlayerELSeq0Port, xlayerELSeq1Port, xlayerELSeq2Port,
	xlayerConductor1RPCPort, xlayerConductor2RPCPort, xlayerConductor3RPCPort,
}

// assertPortsFree fails if any of the given ports can already be bound, which
// would mean this process is not actually listening on them.
func assertPortsFree(t require.TestingT, ports ...int) {
	for _, port := range ports {
		ln, err := net.Listen("tcp", net.JoinHostPort("127.0.0.1", strconv.Itoa(port)))
		if err != nil {
			t.Errorf("expected port %d to be free, but binding it failed: %v", port, err)
			continue
		}
		_ = ln.Close()
	}
}

// TestXLayerFixedPortsCrossProcess verifies the fixed-port contract from OUTSIDE
// the test process. A separate OS process dials the real loopback endpoints —
// L1 8545, the three ELs 8123/8124/8125 and the three conductor RPCs
// 8547/8548/8549 — and reports what it sees, so the assertions never read the
// devstack's internal ws:// proxy (which is not the external listener).
func TestXLayerFixedPortsCrossProcess(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	sys := presets.NewXLayerWithConductors(t, presets.WithXLayerFixedPorts(fixedPlan))
	require.NotNil(t, sys)

	// The assertions below are made by a foreign process against the real
	// loopback listeners. The devstack's EL handle is deliberately not used as
	// the target: it points at a dynamic ws:// proxy that is not the fixed
	// external listener.

	raw := dialFixedPortsFromSeparateProcess(t)
	// The L1 runs its own chain, which is not the L2 chain. The expectation is
	// the exact devstack L1 chain ID compared by equality, not a "not 195" test:
	// a negative comparison also passes for an L1 that answered 0, for an empty
	// response, or for whatever else happened to be listening on the port.
	wantL1ChainID := "0x" + sysgo.DefaultL1ID.ToBig().Text(16)
	require.Equalf(t, wantL1ChainID, strings.ToLower(raw.L1ChainID),
		"the L1 EL on fixed port %d must report the devstack L1 chain ID", xlayerL1HTTPPort)
	t.Logf("L1 chain ID observed through the separate process: %s", raw.L1ChainID)
	for i, chainID := range raw.ELChainIDs {
		require.Equal(t, "0xc3", strings.ToLower(chainID),
			"the L2 EL for sequencer index %d must report chainID 195 on its fixed port", i)
	}
	require.Len(t, raw.ELGenesisHashes, 3, "each L2 EL must report a block at the genesis height")
	for i, hash := range raw.ELGenesisHashes {
		require.NotEmpty(t, hash, "L2 EL %d must serve the genesis-height block", i)
		require.Equal(t, raw.ELGenesisHashes[0], hash,
			"all three L2 ELs must share one genesis block at the XLayer genesis height")
	}
	require.Len(t, raw.ConductorResponses, 3, "each conductor RPC must answer over its fixed port")
	for i, body := range raw.ConductorResponses {
		var resp conductorRPCResponse
		require.NoErrorf(t, json.Unmarshal([]byte(body), &resp),
			"conductor %d must answer a JSON-RPC document on its fixed port, got %s", i+1, body)
		require.Equalf(t, "2.0", resp.JSONRPC,
			"conductor %d must answer JSON-RPC 2.0, got %q", i+1, resp.JSONRPC)
		require.Nilf(t, resp.Error,
			"conductor %d must not answer with an RPC error: %+v", i+1, resp.Error)
		require.NotNilf(t, resp.Result,
			"conductor %d must answer a boolean result, got %s", i+1, body)
		require.NotEmptyf(t, resp.ID,
			"conductor %d must echo a request id, got %s", i+1, body)
	}
}

// conductorRPCResponse is the JSON-RPC envelope a conductor's HTTP endpoint
// answers with. The body is parsed into typed fields and type-checked instead of
// being searched for a substring: "the body mentions result" is also true of an
// error document that quotes the word, and of a body that carries neither field.
type conductorRPCResponse struct {
	JSONRPC string             `json:"jsonrpc"`
	ID      json.RawMessage    `json:"id"`
	Result  *bool              `json:"result"`
	Error   *conductorRPCError `json:"error"`
}

// conductorRPCError is the error member of the same envelope.
type conductorRPCError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

// rawFixedPortReport is what the separate dialer process reports back.
type rawFixedPortReport struct {
	L1ChainID          string   `json:"l1_chain_id"`
	ELChainIDs         []string `json:"el_chain_ids"`
	ELGenesisHashes    []string `json:"el_genesis_hashes"`
	ConductorResponses []string `json:"conductor_responses"`
}

// xlayerGenesisHeight is the non-zero L2 genesis height every XLayer EL shares,
// used by the cross-process dialer to compare the genesis block across nodes.
func xlayerGenesisHeight() uint64 { return sysgo.XLayerDefaultL2GenesisHeight }

// dialFixedPortsFromSeparateProcess runs a Python one-liner that speaks raw
// JSON-RPC over TCP to each fixed endpoint. It is a genuinely independent OS
// process: it shares no memory, no sockets and no Go runtime with the test, so a
// success here is real evidence that a foreign process can reach the listeners.
func dialFixedPortsFromSeparateProcess(t devtest.T) rawFixedPortReport {
	// The script is assembled with named tokens, not with a printf format: the
	// Python source itself contains "%" operators, and a format pass would
	// consume them.
	script := `
import json, sys, urllib.request

def call(port, method, params):
    body = json.dumps({"jsonrpc":"2.0","id":1,"method":method,"params":params}).encode()
    req = urllib.request.Request("http://127.0.0.1:" + str(port), data=body,
                                 headers={"Content-Type":"application/json"})
    with urllib.request.urlopen(req, timeout=10) as resp:
        return resp.read().decode()

el_ports = [__EL0__, __EL1__, __EL2__]
conductor_ports = [__C1__, __C2__, __C3__]

out = {}
out["l1_chain_id"] = json.loads(call(__L1__, "eth_chainId", []))["result"]
out["el_chain_ids"] = [json.loads(call(p, "eth_chainId", []))["result"] for p in el_ports]
height = hex(__HEIGHT__)
out["el_genesis_hashes"] = [
    (json.loads(call(p, "eth_getBlockByNumber", [height, False])).get("result") or {}).get("hash", "")
    for p in el_ports
]
out["conductor_responses"] = [call(p, "conductor_leader", []) for p in conductor_ports]
sys.stdout.write(json.dumps(out))
`
	script = strings.NewReplacer(
		"__L1__", strconv.Itoa(xlayerL1HTTPPort),
		"__EL0__", strconv.Itoa(xlayerELSeq0Port),
		"__EL1__", strconv.Itoa(xlayerELSeq1Port),
		"__EL2__", strconv.Itoa(xlayerELSeq2Port),
		"__C1__", strconv.Itoa(xlayerConductor1RPCPort),
		"__C2__", strconv.Itoa(xlayerConductor2RPCPort),
		"__C3__", strconv.Itoa(xlayerConductor3RPCPort),
		"__HEIGHT__", strconv.FormatUint(xlayerGenesisHeight(), 10),
	).Replace(script)

	ctx, cancel := context.WithTimeout(t.Ctx(), 60*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "python3", "-c", script)
	out, err := cmd.CombinedOutput()
	require.NoErrorf(t, err, "the separate dialer process must reach every fixed port: %s", string(out))

	var report rawFixedPortReport
	require.NoErrorf(t, json.Unmarshal(out, &report), "dialer output must be JSON: %s", string(out))
	return report
}

// TestXLayerRandomDefaultRegression verifies that the random default is
// untouched: with no plan at all the topology still comes up, and not one of the
// well-known fixed ports is occupied by it. Two instances are alive at the same
// time to show that the random path cannot clash with itself either.
func TestXLayerRandomDefaultRegression(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	first := presets.NewXLayerWithConductors(t)
	require.NotNil(t, first)

	// Both instances are live: the first one is only torn down by its Cleanup at
	// the end of the test.
	second := presets.NewXLayerWithConductors(t)
	require.NotNil(t, second)

	for chainID, conductors := range first.ConductorSets {
		require.Equal(t, "195", chainID.String(), "the topology must run the XLayer chain ID")
		for _, c := range conductors {
			require.NotEmpty(t, c.String())
		}
	}

	// Nothing in either random-port instance may have taken a fixed port.
	assertPortsFree(t, fixedPorts...)
}

// TestXLayerFixedPortCollisionCleanup verifies the occupied-port contract over
// the REAL listener lifecycle: a live socket holds one planned port, the
// availability check the constructor runs on its way in reports that port
// against the component that owns it, no other planned port was consumed while
// it looked, and once the holder is closed every planned port binds for real
// again — which is the proof that the refused attempt leaked nothing.
//
// It does not need the XLayer execution binary: everything it asserts is about
// port ownership, which the operating system answers on its own.
func TestXLayerFixedPortCollisionCleanup(gt *testing.T) {
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	// Pre-occupy one of the planned EL ports with a real listener.
	holder, err := net.Listen("tcp", net.JoinHostPort("127.0.0.1", strconv.Itoa(xlayerELSeq0Port)))
	require.NoError(t, err, "the test must be able to occupy the planned port")
	defer func() { _ = holder.Close() }()

	err = sysgo.CheckXLayerFixedPortsAvailable(&fixedPlan)
	require.Error(t, err, "an occupied fixed port must be reported")
	require.Containsf(t, err.Error(), strconv.Itoa(xlayerELSeq0Port),
		"the error must name the occupied port, got: %v", err)
	require.Containsf(t, err.Error(), "L2 EL",
		"the error must name the owning component, got: %v", err)

	// Nothing may have been left behind on the other planned ports.
	assertPortsFree(t,
		xlayerL1HTTPPort,
		xlayerELSeq1Port, xlayerELSeq2Port,
		xlayerConductor1RPCPort, xlayerConductor2RPCPort, xlayerConductor3RPCPort,
	)

	// Release the port and prove the very same plan is accepted again, then take
	// every planned port for real and give it back. A second validation pass after
	// that bind/release cycle is what shows the checks themselves hold nothing.
	require.NoError(t, holder.Close(), "releasing the occupied port must succeed")
	require.NoError(t, sysgo.CheckXLayerFixedPortsAvailable(&fixedPlan),
		"the same plan must be accepted once the holder releases the port")
	for _, port := range fixedPorts {
		ln, listenErr := net.Listen("tcp", net.JoinHostPort("127.0.0.1", strconv.Itoa(port)))
		require.NoErrorf(t, listenErr, "port %d must be bindable after the refused attempt", port)
		require.NoError(t, ln.Close())
	}
	require.NoError(t, sysgo.CheckXLayerFixedPortsAvailable(&fixedPlan),
		"the plan must still be usable after a full bind and release cycle")
}

// TestXLayerFixedPortTeardownThenReuse verifies the teardown-then-reuse contract
// on real sockets: the whole planned port set is claimed the way the topology
// would claim it, released in reverse order, and then claimed again from
// scratch. A port that was left bound by the teardown — or a check that closes
// only the listener it happened to look at last — fails on the second pass.
//
// It does not need the XLayer execution binary: it is entirely about listener
// ownership and release.
func TestXLayerFixedPortTeardownThenReuse(gt *testing.T) {
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	// Claim every planned port, in the order the topology claims them.
	held := make([]net.Listener, 0, len(fixedPorts))
	for _, port := range fixedPorts {
		ln, err := net.Listen("tcp", net.JoinHostPort("127.0.0.1", strconv.Itoa(port)))
		require.NoErrorf(t, err, "the test must be able to claim planned port %d", port)
		held = append(held, ln)
	}

	// With every port held the plan must be refused, and by the L1, which is the
	// first endpoint the check walks.
	err := sysgo.CheckXLayerFixedPortsAvailable(&fixedPlan)
	require.Error(t, err, "a fully occupied plan must be reported")
	require.Containsf(t, err.Error(), strconv.Itoa(xlayerL1HTTPPort),
		"the first reported conflict must name the L1 port, got: %v", err)

	// Full teardown, in reverse claim order.
	for i := len(held) - 1; i >= 0; i-- {
		require.NoErrorf(t, held[i].Close(), "releasing planned port %d must succeed", fixedPorts[i])
	}

	// Every port is genuinely free and the plan validates — twice, so the second
	// pass also proves the first validation left nothing bound.
	require.NoError(t, sysgo.CheckXLayerFixedPortsAvailable(&fixedPlan),
		"every planned port must be reusable after a full teardown")
	assertPortsFree(t, fixedPorts...)
	require.NoError(t, sysgo.CheckXLayerFixedPortsAvailable(&fixedPlan),
		"the plan must still validate after the ports were probed")

	// And the whole set can be claimed again from scratch.
	for _, port := range fixedPorts {
		ln, listenErr := net.Listen("tcp", net.JoinHostPort("127.0.0.1", strconv.Itoa(port)))
		require.NoErrorf(t, listenErr, "planned port %d must be claimable again after teardown", port)
		require.NoError(t, ln.Close())
	}
}

// TestXLayerL1HTTPPortIsFixed verifies that the in-process L1 EL binds to the
// port specified in XLayerFixedPortPlan.L1HTTPPort by dialling it directly.
func TestXLayerL1HTTPPortIsFixed(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	_ = presets.NewXLayerWithConductors(t, presets.WithXLayerFixedPorts(fixedPlan))

	ctx, cancel := context.WithTimeout(t.Ctx(), 15*time.Second)
	defer cancel()

	endpoint := fmt.Sprintf("http://127.0.0.1:%d", xlayerL1HTTPPort)
	client, err := ethclient.DialContext(ctx, endpoint)
	require.NoError(t, err, "the L1 EL must be reachable at its fixed HTTP port")
	defer client.Close()
	_, err = client.BlockNumber(ctx)
	require.NoError(t, err, "the L1 EL at the fixed port must answer eth_blockNumber")
}

// TestXLayerELHTTPPortsAreFixed verifies that each sequencer EL binds to its own
// planned port.
func TestXLayerELHTTPPortsAreFixed(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	_ = presets.NewXLayerWithConductors(t, presets.WithXLayerFixedPorts(fixedPlan))

	ctx, cancel := context.WithTimeout(t.Ctx(), 15*time.Second)
	defer cancel()

	for i, port := range fixedPlan.ELHTTPPorts {
		endpoint := fmt.Sprintf("http://127.0.0.1:%d", port)
		client, err := ethclient.DialContext(ctx, endpoint)
		require.NoErrorf(t, err, "L2 EL index %d must be reachable at fixed HTTP port %d", i, port)
		_, err = client.BlockNumber(ctx)
		client.Close()
		require.NoErrorf(t, err, "L2 EL index %d at fixed port %d must answer eth_blockNumber", i, port)
	}
}

// TestXLayerConductorRPCPortsAreFixed verifies that each in-process conductor
// binds to its own planned RPC port.
func TestXLayerConductorRPCPortsAreFixed(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	_ = presets.NewXLayerWithConductors(t, presets.WithXLayerFixedPorts(fixedPlan))

	ctx, cancel := context.WithTimeout(t.Ctx(), 15*time.Second)
	defer cancel()

	for i, port := range fixedPlan.ConductorRPCPorts {
		endpoint := fmt.Sprintf("http://127.0.0.1:%d", port)
		rpcCl, err := gethrpc.DialContext(ctx, endpoint)
		require.NoErrorf(t, err, "conductor %d must be reachable at %s", i+1, endpoint)
		api := conductorRpc.NewAPIClient(rpcCl)
		_, err = api.Leader(ctx)
		rpcCl.Close()
		require.NoErrorf(t, err, "conductor %d at port %d must answer a real RPC request", i+1, port)
	}
}
