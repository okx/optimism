package sysgo

import (
	"context"
	"errors"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"sync/atomic"
	"time"

	ds "github.com/ipfs/go-datastore"
	dssync "github.com/ipfs/go-datastore/sync"

	bss "github.com/ethereum-optimism/optimism/op-batcher/batcher"
	batcherFlags "github.com/ethereum-optimism/optimism/op-batcher/flags"
	"github.com/ethereum-optimism/optimism/op-chain-ops/devkeys"
	opconductor "github.com/ethereum-optimism/optimism/op-conductor/conductor"
	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/ethereum-optimism/optimism/op-e2e/e2eutils/setuputils"
	"github.com/ethereum-optimism/optimism/op-node/p2p"
	"github.com/ethereum-optimism/optimism/op-node/rollup/derive"
	"github.com/ethereum-optimism/optimism/op-service/dial"
	"github.com/ethereum-optimism/optimism/op-service/endpoint"
	"github.com/ethereum-optimism/optimism/op-service/eth"
	oplog "github.com/ethereum-optimism/optimism/op-service/log"
	opmetrics "github.com/ethereum-optimism/optimism/op-service/metrics"
	"github.com/ethereum-optimism/optimism/op-service/retry"
	oprpc "github.com/ethereum-optimism/optimism/op-service/rpc"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/crypto"
	"github.com/ethereum/go-ethereum/ethclient"
	"github.com/ethereum/go-ethereum/log"
)

// xlayerSequencerNodeNames and xlayerConductorNames name the owners of the three
// EL ports and three conductor RPC ports so a port conflict can be reported as
// component + port rather than as a bare bind error.
var (
	xlayerSequencerNodeNames = [3]string{"sequencer", "sequencer2", "sequencer3"}
	xlayerConductorNames     = [3]string{"conductor1", "conductor2", "conductor3"}
)

// xlayerFixedPortEndpoint pairs a component with the fixed port a plan asks it
// to own.
type xlayerFixedPortEndpoint struct {
	component string
	port      int
}

// xlayerFixedPortEndpoints lists every non-zero fixed port in plan together with
// the component that owns it. A nil plan yields no endpoints.
func xlayerFixedPortEndpoints(plan *XLayerFixedPortPlan) []xlayerFixedPortEndpoint {
	if plan == nil {
		return nil
	}
	out := make([]xlayerFixedPortEndpoint, 0, 7)
	if plan.L1HTTPPort != 0 {
		out = append(out, xlayerFixedPortEndpoint{component: "L1 EL", port: plan.L1HTTPPort})
	}
	for i, port := range plan.ELHTTPPorts {
		if port == 0 {
			continue
		}
		out = append(out, xlayerFixedPortEndpoint{
			component: "L2 EL " + xlayerSequencerNodeNames[i],
			port:      port,
		})
	}
	for i, port := range plan.ConductorRPCPorts {
		if port == 0 {
			continue
		}
		out = append(out, xlayerFixedPortEndpoint{
			component: "conductor " + xlayerConductorNames[i],
			port:      port,
		})
	}
	return out
}

// checkXLayerFixedPortsAvailable reports the first fixed port in plan that cannot
// be bound on loopback at this moment, naming the component and the port.
//
// This is a fail-fast check that runs before any resource is created: without it
// a conflict only surfaces much later as an op-reth startup timeout with no
// indication of which listener was refused. The port is not reserved here — the
// authoritative claim is still made by the owning process (op-reth's own
// --http.port bind, op-conductor's own RPC listen), so a conflict that appears
// between this check and that bind is still reported by the bind itself.
func checkXLayerFixedPortsAvailable(plan *XLayerFixedPortPlan) error {
	for _, ep := range xlayerFixedPortEndpoints(plan) {
		if err := checkXLayerFixedPortAt(ep.component, ep.port); err != nil {
			return err
		}
	}
	return nil
}

// CheckXLayerFixedPortsAvailable is the exported form of the availability check
// so a caller outside this package can validate a plan without constructing a
// topology — and therefore without triggering a fatal assertion.
func CheckXLayerFixedPortsAvailable(plan *XLayerFixedPortPlan) error {
	return checkXLayerFixedPortsAvailable(plan)
}

// checkXLayerFixedPortAt reports whether a single fixed port can be bound on
// loopback right now, naming the component and the port when it cannot.
func checkXLayerFixedPortAt(component string, port int) error {
	ln, err := net.Listen("tcp", net.JoinHostPort("127.0.0.1", strconv.Itoa(port)))
	if err != nil {
		return fmt.Errorf("%s cannot bind fixed port %d: %w", component, port, err)
	}
	return ln.Close()
}

// xlayerSequencerELPort returns the fixed EL HTTP port planned for the sequencer
// at index, and whether the plan fixes one at all. An absent plan, a short list
// and a zero entry all mean "let the OS choose".
func xlayerSequencerELPort(cfg PresetConfig, index int) (int, bool) {
	if cfg.XLayerFixedPorts == nil || index < 0 || index >= len(cfg.XLayerFixedPorts.ELHTTPPorts) {
		return 0, false
	}
	port := cfg.XLayerFixedPorts.ELHTTPPorts[index]
	return port, port != 0
}

// xlayerProducerConfigAvailable reports whether the preset configured a usable
// XLayer execution binary for the candidates.
//
// The producer and flashblocks flags are part of the XLayer build of the
// execution client, supplied through XLAYER_RETH_BIN (OpRethWithBinary). The
// repository's own op-reth rejects them outright — it exits with "unexpected
// argument '--xlayer.sequencer-mode'" — so they are only attached when such a
// binary is configured. Applying them to a generic client would not make it an
// XLayer producer; it would only stop it from starting at all.
//
// The configured path is resolved and then checked as a REAL file: a non-empty
// string is not evidence that a producer was supplied, and a path that does not
// exist or cannot be executed would otherwise carry the XLayer flags all the
// way to a process launch that is guaranteed to fail. Callers that need the
// stronger guarantee — that the named program is this project's XLayer build
// and not merely some executable — get it from the acceptance gate, which runs
// the binary's own version check and asserts the producer flags it received.
func xlayerProducerConfigAvailable(t devtest.T, cfg PresetConfig) bool {
	return xlayerExecutionBinaryUsable(xlayerExecutionBinary(t, cfg))
}

// xlayerExecutionBinary resolves the execution binary path selected by the
// preset's op-reth options, or "" when the preset selects none.
func xlayerExecutionBinary(t devtest.T, cfg PresetConfig) string {
	scratch := DefaultOpRethConfig()
	target := NewComponentTarget(XLayerSequencerNodeName, eth.ChainIDFromUInt64(195))
	for _, opt := range cfg.OpRethOptions {
		if opt == nil {
			continue
		}
		opt.Apply(t, target, scratch)
	}
	return scratch.Binary
}

// xlayerExecutionBinaryUsable reports whether path names a regular file that
// carries an executable bit. An empty path, a directory, a device node and a
// file without any execute permission for anyone all mean "no producer binary
// was configured", because none of them can be launched.
func xlayerExecutionBinaryUsable(path string) bool {
	if path == "" {
		return false
	}
	info, err := os.Stat(path)
	if err != nil {
		return false
	}
	return info.Mode().IsRegular() && info.Mode().Perm()&0o111 != 0
}

// xlayerCandidateExtraOptions returns the options one sequencer candidate adds
// on top of the preset's own per-node options: the XLayer producer
// configuration when an XLayer execution binary is configured, plus the
// candidate's fixed HTTP port when the plan fixes one. Every candidate gets the
// same treatment, because every candidate can be elected and must then be able
// to produce.
func xlayerCandidateExtraOptions(t devtest.T, cfg PresetConfig, name string, index int) ([]OpRethOption, error) {
	extra := make([]OpRethOption, 0, 2)
	if xlayerProducerConfigAvailable(t, cfg) {
		extra = append(extra, xlayerFlashblocksProducerOption())
	}
	if port, ok := xlayerSequencerELPort(cfg, index); ok {
		if err := checkXLayerFixedPortAt("L2 EL "+name, port); err != nil {
			return nil, err
		}
		extra = append(extra, OpRethWithHTTPPort(port))
	}
	return extra, nil
}

// xlayerClosable is a started resource that can be released again. The rollback
// path needs only this much, which keeps it independent of the full node
// interfaces and lets a test drive it with a real bound listener as well as
// with the devstack's own execution clients.
type xlayerClosable interface {
	Stop()
}

// closeXLayerClosers releases the given started resources in reverse start
// order. It is the unwind path of a partially built topology: each resource
// that started holds a live process and a bound listener, so a failure part-way
// through bring-up has to release every earlier one or the ports — fixed or
// not — stay claimed for the rest of the process. Nil entries are skipped so a
// caller can pass the result of a failed step without a guard.
func closeXLayerClosers(closers ...xlayerClosable) {
	for i := len(closers) - 1; i >= 0; i-- {
		if closers[i] != nil {
			closers[i].Stop()
		}
	}
}

// xlayerSequencerStartFuncs are the two startup steps of one XLayer sequencer
// node. The indirection exists so the rollback contract can be exercised with
// fakes as well as with the real devstack starters.
type xlayerSequencerStartFuncs struct {
	startEL func(name string, index int, cfg PresetConfig) (L2ELNode, xlayerClosable, error)
	startCL func(name string, el L2ELNode, cfg PresetConfig) (*OpNode, error)
}

// startXLayerSequencerNodeWith starts one XLayer sequencer node in two steps and
// unwinds whatever it already created when a later step fails: if the CL cannot
// be started, the EL started by the first step is closed before the error is
// returned, so a partially started node never leaves a running process or a
// bound listener behind. Nothing is registered unless both steps succeeded.
func startXLayerSequencerNodeWith(name string, index int, cfg PresetConfig, f xlayerSequencerStartFuncs) (*SingleChainNodeRuntime, *OpNode, error) {
	el, elCloser, err := f.startEL(name, index, cfg)
	if err != nil {
		return nil, nil, fmt.Errorf("start EL for XLayer sequencer %q: %w", name, err)
	}
	cl, err := f.startCL(name, el, cfg)
	if err != nil {
		if elCloser != nil {
			elCloser.Stop()
		}
		return nil, nil, fmt.Errorf("start CL for XLayer sequencer %q, EL already rolled back: %w", name, err)
	}
	return newSingleChainNodeRuntime(name, true, el, cl), cl, nil
}

// xlayerSecondaryStarter starts one secondary sequencer candidate and returns
// the handle that releases its execution client again.
type xlayerSecondaryStarter func(name string, index int) (*SingleChainNodeRuntime, *OpNode, xlayerClosable, error)

// startXLayerSecondaryCandidates starts the secondary sequencer candidates in
// order and unwinds the ones that already started when a later candidate fails.
//
// A candidate that started has a live process and a claimed listener and nothing
// else holds a reference to it, so returning the error without closing the
// earlier candidates would strand both — including, on a fixed-port run, a port
// the next attempt needs. The unwind runs in reverse start order, which is the
// order the resources were acquired in.
func startXLayerSecondaryCandidates(
	names []string,
	start xlayerSecondaryStarter,
) (nodes []*SingleChainNodeRuntime, cls []*OpNode, closers []xlayerClosable, err error) {
	nodes = make([]*SingleChainNodeRuntime, 0, len(names))
	cls = make([]*OpNode, 0, len(names))
	closers = make([]xlayerClosable, 0, len(names))
	for i, name := range names {
		node, cl, closer, startErr := start(name, i+1)
		if startErr != nil {
			closeXLayerClosers(closers...)
			return nil, nil, nil, fmt.Errorf("start XLayer sequencer candidate %q: %w", name, startErr)
		}
		nodes = append(nodes, node)
		cls = append(cls, cl)
		closers = append(closers, closer)
	}
	return nodes, cls, closers, nil
}

// addXLayerSequencerNode starts a secondary or tertiary XLayer sequencer EL+CL
// pair and registers it in runtime.Nodes[name]. The CL is started with
// SequencerStopped=true so a conductor can attach before block production begins.
// index selects the ELHTTPPorts entry when fixed ports are configured (0-based).
// A fixed-port conflict is reported before anything is started, and a failure
// after the EL is up closes the EL again.
//
// The returned closer releases this node's execution client. The caller needs
// it because starting one node can fail after an earlier one already succeeded:
// without the handle, the earlier candidate's process and listener would survive
// the failed bring-up with no one holding a reference to release them.
func addXLayerSequencerNode(t devtest.T, runtime *SingleChainRuntime, name string, index int, cfg PresetConfig) (*SingleChainNodeRuntime, *OpNode, xlayerClosable, error) {
	jwtPath := runtime.L2EL.JWTPath()
	jwtSecret := readJWTSecretFromPath(t, jwtPath)

	starters := xlayerSequencerStartFuncs{
		startEL: func(name string, index int, cfg PresetConfig) (L2ELNode, xlayerClosable, error) {
			// A backup is a leadership candidate, so it is built exactly like the
			// primary: the preset's per-node options plus the candidate extras
			// (XLayer producer configuration when the XLayer binary is configured,
			// and this candidate's fixed HTTP port).
			extra, extraErr := xlayerCandidateExtraOptions(t, cfg, name, index)
			if extraErr != nil {
				return nil, nil, extraErr
			}
			elOpts := append(append([]OpRethOption{}, cfg.OpRethOptions...), extra...)
			el := startL2ELForKey(t, runtime.L2Network, jwtPath, jwtSecret, name, NewELNodeIdentity(0), elOpts...)
			if el == nil {
				return nil, nil, fmt.Errorf("L2 EL %s did not start", name)
			}
			// The EL process must be releasable if a later step of this node's
			// startup fails. Every EL backend used here is a stack.Lifecycle.
			closer, _ := el.(xlayerClosable)
			return el, closer, nil
		},
		startCL: func(name string, el L2ELNode, cfg PresetConfig) (*OpNode, error) {
			if el == nil {
				return nil, fmt.Errorf("cannot start CL %s without an EL", name)
			}
			cl := startL2CLNode(t, runtime.Keys, runtime.L1Network, runtime.L2Network, runtime.L1EL, runtime.L1CL, el, jwtSecret, l2CLNodeStartConfig{
				Key:              name,
				IsSequencer:      true,
				NoDiscovery:      true,
				EnableReqResp:    true,
				L2CLOptions:      cfg.GlobalL2CLOptions,
				SequencerStopped: true,
			})
			if cl == nil {
				return nil, fmt.Errorf("L2 CL %s did not start", name)
			}
			return cl, nil
		},
	}

	node, cl, err := startXLayerSequencerNodeWith(name, index, cfg, starters)
	if err != nil {
		return nil, nil, nil, err
	}
	runtime.Nodes[name] = node
	closer, _ := node.EL.(xlayerClosable)
	return node, cl, closer, nil
}

// startXLayerConductorNode attaches an in-process op-conductor to opNode.
// rpcPort, when non-zero, fixes the conductor RPC listener port; 0 means OS-assigned.
func startXLayerConductorNode(
	t devtest.T,
	conductorName string,
	l2Net *L2Network,
	opNode *OpNode,
	l2EL L2ELNode,
	bootstrap bool,
	paused bool,
	rpcPort int,
) *Conductor {
	require := t.Require()
	serverID := conductorName
	require.NotEmpty(serverID, "conductor ID key cannot be empty")

	var conductorRPCEndpoint atomic.Value
	conductorRPCEndpoint.Store("")
	opNode.cfg.ConductorEnabled = true
	opNode.cfg.ConductorRpcTimeout = 5 * time.Second
	opNode.cfg.ConductorRpc = func(ctx context.Context) (string, error) {
		for {
			if ep, _ := conductorRPCEndpoint.Load().(string); ep != "" {
				return ep, nil
			}
			select {
			case <-ctx.Done():
				return "", ctx.Err()
			case <-time.After(100 * time.Millisecond):
			}
		}
	}
	opNode.cfg.Driver.SequencerStopped = true
	opNode.Stop()
	// The in-memory P2P peerstore datastore (cfg.P2P.Store) is created once in
	// startL2CLNode and reused across Stop/Start cycles. After stop it still holds
	// the pre-restart listen address, so the peerstore returned by Self() on the
	// next Start would report the old (dead) port. Replacing the store here gives
	// the restarted op-node a clean peerstore with no stale address entries.
	if p2pCfg, ok := opNode.cfg.P2P.(*p2p.Config); ok && p2pCfg != nil {
		p2pCfg.Store = dssync.MutexWrap(ds.NewMapDatastore())
	}
	opNode.Start()

	metricsPortStr, metricsPortErr := getAvailableLocalPort()
	require.NoError(metricsPortErr, "allocate metrics port for conductor")
	metricsPort, metricsPortParseErr := strconv.Atoi(metricsPortStr)
	require.NoError(metricsPortParseErr, "parse metrics port")

	conductorCfg := opconductor.Config{
		ConsensusAddr:           "127.0.0.1",
		ConsensusPort:           0,
		ConsensusAdvertisedAddr: "",
		RaftServerID:            serverID,
		RaftStorageDir:          filepath.Join(t.TempDirWithPrefix("op-conductor-"+NewComponentTarget(conductorName, l2Net.ChainID()).String()), "raft"),
		RaftBootstrap:           bootstrap,
		RaftSnapshotInterval:    120 * time.Second,
		RaftSnapshotThreshold:   8192,
		RaftTrailingLogs:        10240,
		RaftHeartbeatTimeout:    1000 * time.Millisecond,
		RaftLeaderLeaseTimeout:  500 * time.Millisecond,
		NodeRPC:                 opNode.UserRPC(),
		ExecutionRPC:            l2EL.UserRPC(),
		Paused:                  paused,
		HealthCheck: opconductor.HealthCheckConfig{
			// Health detection is driven purely by the CL peer count and the
			// staleness of the unsafe head. A conductor whose sequencer stops
			// producing therefore reads unhealthy once its unsafe head is older
			// than UnsafeInterval, which is what lets the remaining conductors
			// win the re-election.
			Interval:       2,
			UnsafeInterval: 12,
			SafeInterval:   120,
			SafeEnabled:    false,
			MinPeerCount:   1,
		},
		RollupCfg:      *l2Net.rollupCfg,
		RPCEnableProxy: false,
		LogConfig: oplog.CLIConfig{
			Level:  log.LevelInfo,
			Format: oplog.FormatText,
			Color:  false,
		},
		MetricsConfig: opmetrics.CLIConfig{
			Enabled:    true,
			ListenAddr: "127.0.0.1",
			ListenPort: metricsPort,
		},
		RPC: oprpc.CLIConfig{
			ListenAddr: "127.0.0.1",
			ListenPort: rpcPort,
		},
		HTTPBodyLimitMB: 5,
	}

	logger := t.Logger().New("component", "conductor", "name", conductorName, "chain", l2Net.ChainID())
	svc, err := opconductor.New(t.Ctx(), &conductorCfg, logger, "0.0.1")
	require.NoErrorf(err, "construct conductor %s", conductorName)
	stopConductor := func() {
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		logger.Info("Closing conductor")
		if err := svc.Stop(ctx); err != nil {
			logger.Warn("Failed to close conductor cleanly", "err", err)
		}
	}
	if err := svc.Start(t.Ctx()); err != nil {
		// A bind failure here means the conductor's own RPC or metrics listener
		// could not be claimed. Close whatever listener did come up before
		// surfacing the error, so a failed attach leaves nothing behind.
		stopConductor()
		require.NoErrorf(err, "start conductor %s (RPC port %d, metrics port %d)", conductorName, rpcPort, metricsPort)
	}
	t.Cleanup(stopConductor)

	out := &Conductor{
		name:              conductorName,
		chainID:           l2Net.ChainID(),
		serverID:          serverID,
		consensusEndpoint: svc.ConsensusEndpoint(),
		rpcEndpoint:       svc.HTTPEndpoint(),
		metricsURL:        fmt.Sprintf("http://127.0.0.1:%d/metrics", metricsPort),
		service:           svc,
	}
	conductorRPCEndpoint.Store(svc.HTTPEndpoint())
	return out
}

// startXLayerConductorClusterSeeded bootstraps a 3-node Raft cluster for the
// non-zero XLayer genesis. It waits for all secondary CLs to reach the primary's
// unsafe head, commits a seeding payload via the leader, then resumes all conductors
// so they begin sequencing.
func startXLayerConductorClusterSeeded(
	t devtest.T,
	bootstrap *Conductor,
	members []*Conductor,
	primaryEL L2ELNode,
	secondaryCLs []L2CLNode,
	secondaryELs []L2ELNode,
	l2Net *L2Network,
) {
	require := t.Require()
	ctx, cancel := context.WithTimeout(t.Ctx(), 90*time.Second)
	defer cancel()

	// Wait for the bootstrap conductor to become Raft leader.
	err := retry.Do0(ctx, 90, retry.Fixed(500*time.Millisecond), func() error {
		if !bootstrap.service.Leader(ctx) {
			return errors.New("bootstrap conductor is not leader yet")
		}
		return nil
	})
	require.NoError(err, "bootstrap conductor never became leader")

	// Add each member as non-voter then promote to voter.
	for _, member := range members {
		err := retry.Do0(ctx, 40, retry.Fixed(250*time.Millisecond), func() error {
			return bootstrap.service.AddServerAsNonvoter(ctx, member.ServerID(), member.ConsensusEndpoint(), 0)
		})
		require.NoErrorf(err, "failed to add conductor %s as non-voter", member.ServerID())

		err = retry.Do0(ctx, 40, retry.Fixed(250*time.Millisecond), func() error {
			return bootstrap.service.AddServerAsVoter(ctx, member.ServerID(), member.ConsensusEndpoint(), 0)
		})
		require.NoErrorf(err, "failed to add conductor %s as voter", member.ServerID())
	}

	// Wait for cluster membership to stabilize.
	expectedServers := 1 + len(members)
	err = retry.Do0(ctx, 90, retry.Fixed(500*time.Millisecond), func() error {
		membership, err := bootstrap.service.ClusterMembership(ctx)
		if err != nil {
			return err
		}
		if len(membership.Servers) != expectedServers {
			return fmt.Errorf("expected %d conductors in cluster membership, got %d", expectedServers, len(membership.Servers))
		}
		return nil
	})
	require.NoError(err, "conductor cluster did not converge to expected membership")

	// Seed from the primary EL's current unsafe head. When conductors are attached
	// after the primary sequencer has already produced blocks, seeding from genesis
	// would cause compareUnsafeHead in op-conductor to fail (Raft consensus behind
	// the EL's latest unsafe block). The primary sequencer is stopped before this
	// call, so the primary EL head is stable for the duration of seeding.
	//
	// The head is frozen HERE, once, into the immutable pair seedNumber/seedHash,
	// before anything is resumed. Every later assertion in this function compares
	// against that pair rather than re-reading a head that the resume is about to
	// move: a re-read would test whatever the cluster happened to be at, not the
	// block the cluster was actually seeded with.
	seedBlock, err := xlayerFrozenBlock(ctx, primaryEL)
	require.NoError(err, "freeze the primary's unsafe head for the seeded bootstrap")
	seedNumber := seedBlock.NumberU64()
	seedHash := seedBlock.Hash()
	require.Greaterf(seedNumber, XLayerDefaultL2GenesisHeight,
		"the frozen head must be a block produced above the XLayer genesis height %d, not genesis itself; got %d",
		XLayerDefaultL2GenesisHeight, seedNumber)

	// Wait for secondary CLs to reach the primary's frozen head exactly. Secondary
	// ELs sync historical blocks from the primary via EL P2P; once they catch up,
	// secondary CLs derive their unsafe L2 head from the EL to match. The
	// comparison is exact on BOTH fields: a height-only test would be satisfied by
	// a different block at the same height, which is the divergence the seed is
	// there to prevent.
	for _, cl := range secondaryCLs {
		rollupCl, dialErr := dial.DialRollupClientWithTimeout(ctx, t.Logger(), cl.UserRPC())
		require.NoError(dialErr, "dial secondary CL for sync check")
		convErr := retry.Do0(ctx, 60, retry.Fixed(500*time.Millisecond), func() error {
			status, statusErr := rollupCl.SyncStatus(ctx)
			if statusErr != nil {
				return statusErr
			}
			if status.UnsafeL2.Number != seedNumber || status.UnsafeL2.Hash != seedHash {
				return fmt.Errorf("secondary CL unsafe head %d/%s != frozen %d/%s",
					status.UnsafeL2.Number, status.UnsafeL2.Hash, seedNumber, seedHash)
			}
			return nil
		})
		rollupCl.Close()
		require.NoError(convErr, "secondary CL did not sync to the primary's frozen head before seeding")
	}

	// Seed the Raft FSM with the frozen head's payload so all conductors start
	// from a consistent state matching the EL.
	payload, err := eth.BlockAsPayloadEnv(seedBlock, l2Net.ChainConfig())
	require.NoError(err, "convert block to execution payload envelope")
	require.NoError(bootstrap.service.CommitUnsafePayload(ctx, payload), "seed Raft FSM with the frozen head payload")

	// Verify the seeded payload is readable back from the leader so every conductor
	// starts from the same unsafe head the cluster was seeded with. Both fields are
	// compared against the frozen pair, so a readback that returned a different
	// block at the seeded height fails here rather than silently passing.
	latest, readbackErr := bootstrap.service.LatestUnsafePayload(ctx)
	require.NoError(readbackErr, "read back LatestUnsafePayload after commit")
	require.NotNil(latest, "LatestUnsafePayload must not be nil after commit")
	require.Equal(seedHash, latest.ExecutionPayload.BlockHash, "readback hash must equal the frozen head hash")
	require.Equal(seedNumber, uint64(latest.ExecutionPayload.BlockNumber), "readback number must equal the frozen head number")

	// The frozen head must still be the head everywhere at the moment the control
	// loops are resumed. Every sequencer is stopped while the conductors are
	// paused, so a head that moved since the freeze would mean something produced
	// uncontrolled blocks during the mount — exactly the race this phasing exists
	// to prevent. Sampling here, immediately before the first Resume, is what
	// makes "no block was produced between freeze and Resume" an observation
	// rather than an assumption.
	allELs := append([]L2ELNode{primaryEL}, secondaryELs...)
	for i, el := range allELs {
		sampleCtx, sampleCancel := context.WithTimeout(ctx, 10*time.Second)
		candidate, sampleErr := xlayerFrozenBlock(sampleCtx, el)
		sampleCancel()
		require.NoErrorf(sampleErr, "sample the head of sequencer candidate %d before resume", i)
		require.Equalf(seedNumber, candidate.NumberU64(),
			"sequencer candidate %d does not sit on the frozen head: number %d != %d",
			i, candidate.NumberU64(), seedNumber)
		require.Equalf(seedHash, candidate.Hash(),
			"sequencer candidate %d does not sit on the frozen head: hash %s != %s",
			i, candidate.Hash(), seedHash)
	}
	t.Logger().New("component", "xlayer-conductor-seed").Info(
		"Seeded cold start reached the resume point with an unmoved head",
		"seededHeight", seedNumber, "seededHash", seedHash)

	// Resume all conductors so they start sequencing.
	cluster := append([]*Conductor{bootstrap}, members...)
	for _, conductor := range cluster {
		err := retry.Do0(ctx, 40, retry.Fixed(250*time.Millisecond), func() error {
			return conductor.service.Resume(ctx)
		})
		require.NoErrorf(err, "failed to resume conductor %s", conductor.ServerID())
	}
}

// xlayerFrozenBlock reads an EL's latest block. It is what the seeded cold
// start freezes before it resumes anything: the returned block carries both the
// number and the hash of the head as one consistent observation, so the caller
// can compare BOTH fields exactly. Reading a number here and a hash there would
// let a block that moved between the two reads pass as the frozen head.
//
// The client is opened and closed per call rather than held, so the sample does
// not keep a connection open across the resume boundary.
func xlayerFrozenBlock(ctx context.Context, el L2ELNode) (*types.Block, error) {
	client, err := ethclient.DialContext(ctx, el.UserRPC())
	if err != nil {
		return nil, err
	}
	defer client.Close()
	return client.BlockByNumber(ctx, nil) // nil = latest
}

// awaitAnySequencerActive polls all given CL nodes until at least one reports
// SequencerActive = true. The HA batcher's endpoint provider calls SequencerActive
// at construction time, so this must succeed before startXLayerHABatcher is called.
func awaitAnySequencerActive(t devtest.T, clNodes []L2CLNode) {
	require := t.Require()
	ctx, cancel := context.WithTimeout(t.Ctx(), 60*time.Second)
	defer cancel()

	err := retry.Do0(ctx, 120, retry.Fixed(500*time.Millisecond), func() error {
		for _, cl := range clNodes {
			rollupCl, dialErr := dial.DialRollupClientWithTimeout(ctx, t.Logger(), cl.UserRPC())
			if dialErr != nil {
				continue
			}
			active, checkErr := rollupCl.SequencerActive(ctx)
			rollupCl.Close()
			if checkErr == nil && active {
				return nil
			}
		}
		return fmt.Errorf("no active sequencer found among %d CL nodes", len(clNodes))
	})
	require.NoError(err, "no CL node became an active sequencer after conductor resume")
}

// startXLayerHABatcher starts an HA batcher wired to all supplied EL+CL endpoints.
// When len(elNodes) > 1, the batcher uses multi-endpoint failover via
// NewActiveL2EndpointProvider. The primary EL and CL are the first entries of each slice.
func startXLayerHABatcher(
	t devtest.T,
	keys devkeys.Keys,
	l1EL L1ELNode,
	l2Net *L2Network,
	elNodes []L2ELNode,
	clNodes []L2CLNode,
	batcherOpts ...BatcherOption,
) *L2Batcher {
	require := t.Require()
	batcherSecret, err := keys.Secret(devkeys.BatcherRole.Key(l2Net.ChainID().ToBig()))
	require.NoError(err)
	batcherTarget := NewComponentTarget("main", l2Net.ChainID())

	logger := t.Logger().New("component", "l2-batcher-ha")
	logger.SetContext(t.Ctx())
	logger.Info("HA batcher key acquired", "addr", crypto.PubkeyToAddress(batcherSecret.PublicKey))

	l2EthRpcs := make([]string, len(elNodes))
	for i, el := range elNodes {
		l2EthRpcs[i] = el.UserRPC()
	}
	rollupRpcs := make([]string, len(clNodes))
	for i, cl := range clNodes {
		rollupRpcs[i] = cl.UserRPC()
	}

	batcherCLIConfig := &bss.CLIConfig{
		L1EthRpc:                 l1EL.UserRPC(),
		L2EthRpc:                 l2EthRpcs,
		RollupRpc:                rollupRpcs,
		MaxPendingTransactions:   7,
		MaxChannelDuration:       1,
		MaxL1TxSize:              120_000,
		TestUseMaxTxSizeForBlobs: false,
		TargetNumFrames:          1,
		ApproxComprRatio:         0.4,
		SubSafetyMargin:          4,
		PollInterval:             500 * time.Millisecond,
		// The batcher follows the sequencer that the conductor cluster has
		// elected, so it must re-check which endpoint is active on a cadence
		// shorter than the failover window.
		ActiveSequencerCheckDuration: 2 * time.Second,
		TxMgrConfig:                  setuputils.NewTxMgrConfig(endpoint.URL(l1EL.UserRPC()), batcherSecret),
		LogConfig: oplog.CLIConfig{
			Level:  log.LevelInfo,
			Format: oplog.FormatText,
		},
		Stopped:               false,
		BatchType:             derive.SpanBatchType,
		MaxBlocksPerSpanBatch: 10,
		DataAvailabilityType:  batcherFlags.CalldataType,
		CompressionAlgo:       derive.Brotli,
		RPC: oprpc.CLIConfig{
			EnableAdmin: true,
		},
	}
	for _, opt := range batcherOpts {
		if opt == nil {
			continue
		}
		opt(batcherTarget, batcherCLIConfig)
	}

	batcherCtx, cancelBatcherCtx := context.WithCancel(t.Ctx())
	closeAppFn := func(cause error) {
		t.Errorf("closeAppFn called, HA batcher hit a critical error: %v", cause)
		cancelBatcherCtx()
	}
	batcher, err := bss.BatcherServiceFromCLIConfig(
		batcherCtx,
		closeAppFn,
		"0.0.1",
		batcherCLIConfig,
		logger,
	)
	require.NoError(err)
	require.NoError(batcher.Start(t.Ctx()))
	t.Cleanup(func() {
		ctx, cancel := context.WithCancel(t.Ctx())
		cancel()
		logger.Info("Closing HA batcher")
		_ = batcher.Stop(ctx)
		logger.Info("Closed HA batcher")
	})

	var primaryEL, primaryCL string
	if len(elNodes) > 0 {
		primaryEL = elNodes[0].UserRPC()
	}
	if len(clNodes) > 0 {
		primaryCL = clNodes[0].UserRPC()
	}
	return &L2Batcher{
		name:    batcherTarget.Name,
		chainID: batcherTarget.ChainID,
		service: batcher,
		rpc:     batcher.HTTPEndpoint(),
		l1RPC:   l1EL.UserRPC(),
		l2CLRPC: primaryCL,
		l2ELRPC: primaryEL,
	}
}

// xlayerConductorPrimary returns a StartPrimary that starts the primary
// sequencer ACTIVE.
//
// It deliberately does not start with SequencerStopped:true. A topology that
// constructs its primary stopped has nothing to freeze: the seeded cold start
// requires the primary to produce at least one real block before the conductors
// attach, so that the frozen head the cluster is seeded with is a produced block
// rather than the genesis block. The "start stopped, then resume" dance also
// put the conductor in a state it then had to be talked out of.
//
// seqExtraOpts carries the XLayer producer configuration so the primary is built
// exactly like the other two candidates, differing only in its index.
func xlayerConductorPrimary(seqExtraOpts ...OpRethOption) func(devtest.T, devkeys.Keys, singleChainRuntimeWorld, *L1Geth, *L1CLNode, string, [32]byte, PresetConfig) singleChainPrimaryRuntime {
	return func(t devtest.T, keys devkeys.Keys, world singleChainRuntimeWorld, l1EL *L1Geth, l1CL *L1CLNode, jwtPath string, jwtSecret [32]byte, cfg PresetConfig) singleChainPrimaryRuntime {
		elOpts := append(append([]OpRethOption{}, cfg.OpRethOptions...), seqExtraOpts...)
		l2EL := startL2ELForKey(t, world.L2Network, jwtPath, jwtSecret, XLayerSequencerNodeName, NewELNodeIdentity(0), elOpts...)
		l2CL := startL2CLNode(t, keys, world.L1Network, world.L2Network, l1EL, l1CL, l2EL, jwtSecret, l2CLNodeStartConfig{
			Key:           XLayerSequencerNodeName,
			IsSequencer:   true,
			NoDiscovery:   true,
			EnableReqResp: true,
			L2CLOptions:   cfg.GlobalL2CLOptions,
		})
		return singleChainPrimaryRuntime{EL: l2EL, CL: l2CL}
	}
}

// NewXLayerWithConductorsRuntimeWithConfig assembles the XLayer 3-sequencer +
// 3-conductor + HA-batcher topology. All three ELs and CLs are wired as a full
// P2P mesh. All conductors start paused; after Raft consensus is seeded from the
// primary EL's current unsafe head, they are resumed and begin sequencing.
func NewXLayerWithConductorsRuntimeWithConfig(t devtest.T, cfg PresetConfig) *SingleChainRuntime {
	// Fail fast on a fixed-port conflict before any world, L1, EL or conductor
	// resource exists, so an unavailable port allocates nothing.
	t.Require().NoError(checkXLayerFixedPortsAvailable(cfg.XLayerFixedPorts),
		"a fixed port in XLayerFixedPortPlan is not available")

	cfg.DeployerOptions = append([]DeployerOption{
		WithXLayerL2GenesisHeight(XLayerDefaultL2GenesisHeight),
		WithUniformL2BlockTimes(XLayerDefaultL2BlockTime),
		WithXLayerFeeMarketConfig(),
		WithSequencingWindow(XLayerSequencerWindowSize),
	}, cfg.DeployerOptions...)

	// Every candidate — the primary and both backups — is built from the same
	// recipe: the preset's per-node options plus the candidate extras. A backup is
	// a leadership candidate, so the XLayer producer configuration travels with
	// all three nodes rather than only the initial primary.
	producerOpts, producerErr := xlayerCandidateExtraOptions(t, cfg, XLayerSequencerNodeName, 0)
	t.Require().NoError(producerErr, "build the primary candidate's execution options")

	runtime := newSingleChainRuntimeWithConfig(t, cfg, singleChainRuntimeSpec{
		BuildWorld:      buildXLayerWorld,
		ConfigureL1:     configureXLayerSystemConfig,
		StartPrimary:    xlayerConductorPrimary(producerOpts...),
		StartBatcher:    false,
		StartProposer:   true,
		StartChallenger: false,
	})

	secondaryNodes, secondaryCLs, secondaryClosers, err := startXLayerSecondaryCandidates(
		[]string{"sequencer2", "sequencer3"},
		func(name string, index int) (*SingleChainNodeRuntime, *OpNode, xlayerClosable, error) {
			return addXLayerSequencerNode(t, runtime, name, index, cfg)
		},
	)
	t.Require().NoError(err, "add the secondary XLayer sequencer nodes")
	seq2Node, seq3Node := secondaryNodes[0], secondaryNodes[1]
	seq2CL, seq3CL := secondaryCLs[0], secondaryCLs[1]

	// Any later failure — a conductor that cannot claim its RPC or metrics
	// listener, a cluster that never converges, a batcher that will not start —
	// aborts through a fatal assertion, so the unwind for the candidates that are
	// already running has to happen here. On success the topology owns its own
	// teardown and this is inert.
	built := false
	defer func() {
		if !built {
			closeXLayerClosers(secondaryClosers...)
		}
	}()

	// Wire the full EL P2P mesh (trusted=true for sequencer peers).
	connectL2ELPeers(t, t.Logger(), runtime.L2EL.UserRPC(), seq2Node.EL.UserRPC(), true)
	connectL2ELPeers(t, t.Logger(), runtime.L2EL.UserRPC(), seq3Node.EL.UserRPC(), true)
	connectL2ELPeers(t, t.Logger(), seq2Node.EL.UserRPC(), seq3Node.EL.UserRPC(), true)

	// Resolve fixed conductor RPC ports, defaulting to 0 (OS-assigned).
	var conductorRPCPorts [3]int
	if cfg.XLayerFixedPorts != nil {
		conductorRPCPorts = cfg.XLayerFixedPorts.ConductorRPCPorts
	}

	seq1OpNode := runtime.L2CL.(*OpNode)
	cond1 := startXLayerConductorNode(t, "conductor1", runtime.L2Network, seq1OpNode, runtime.L2EL, true, true, conductorRPCPorts[0])
	cond2 := startXLayerConductorNode(t, "conductor2", runtime.L2Network, seq2CL, seq2Node.EL, false, true, conductorRPCPorts[1])
	cond3 := startXLayerConductorNode(t, "conductor3", runtime.L2Network, seq3CL, seq3Node.EL, false, true, conductorRPCPorts[2])

	// Wire the full CL P2P mesh after all conductor node restarts.
	// Each startXLayerConductorNode restarts its op-node; wiring after all
	// three restarts avoids connections being dropped by a subsequent restart.
	connectL2CLPeers(t, t.Logger(), runtime.L2CL, seq2CL)
	connectL2CLPeers(t, t.Logger(), runtime.L2CL, seq3CL)
	connectL2CLPeers(t, t.Logger(), seq2CL, seq3CL)

	startXLayerConductorClusterSeeded(
		t,
		cond1,
		[]*Conductor{cond2, cond3},
		runtime.L2EL,
		[]L2CLNode{seq2CL, seq3CL},
		[]L2ELNode{seq2Node.EL, seq3Node.EL},
		runtime.L2Network,
	)

	// Wait for the Raft cluster to elect a leader and activate a sequencer before
	// constructing the HA batcher: NewActiveL2EndpointProvider calls SequencerActive
	// at construction time and fails if no sequencer is active yet.
	awaitAnySequencerActive(t, []L2CLNode{seq1OpNode, seq2CL, seq3CL})

	runtime.L2Batcher = startXLayerHABatcher(
		t,
		runtime.Keys,
		runtime.L1EL,
		runtime.L2Network,
		[]L2ELNode{runtime.L2EL, seq2Node.EL, seq3Node.EL},
		[]L2CLNode{seq1OpNode, seq2CL, seq3CL},
		cfg.BatcherOptions...,
	)
	runtime.Conductors = map[string]*Conductor{
		"conductor1": cond1,
		"conductor2": cond2,
		"conductor3": cond3,
	}
	built = true
	return runtime
}
