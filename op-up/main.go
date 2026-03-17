package main

import (
	"bytes"
	"context"
	_ "embed"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"runtime/debug"
	"slices"
	"strconv"
	"sync"
	"syscall"
	"time"

	"github.com/ethereum-optimism/optimism/op-chain-ops/devkeys"
	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/ethereum-optimism/optimism/op-devstack/shim"
	"github.com/ethereum-optimism/optimism/op-devstack/stack"
	"github.com/ethereum-optimism/optimism/op-devstack/stack/match"
	"github.com/ethereum-optimism/optimism/op-deployer/pkg/deployer/standard"
	"github.com/ethereum-optimism/optimism/op-devstack/sysgo"
	opservice "github.com/ethereum-optimism/optimism/op-service"
	"github.com/ethereum-optimism/optimism/op-service/cliapp"
	"github.com/ethereum-optimism/optimism/op-service/client"
	"github.com/ethereum-optimism/optimism/op-service/eth"
	oplog "github.com/ethereum-optimism/optimism/op-service/log"
	"github.com/ethereum-optimism/optimism/op-service/log/logfilter"
	"github.com/ethereum-optimism/optimism/op-service/testreq"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/crypto"
	"github.com/ethereum/go-ethereum/log"
	"github.com/ethereum/go-ethereum/rpc"
	"github.com/urfave/cli/v2"
	"go.opentelemetry.io/otel/trace"
)

const asciiArt = ` ____  ____        _     ____
/  _ \/  __\      / \ /\/  __\
| / \||  \/|_____ | | |||  \/|
| \_/||  __/\____\| \_/||  __/
\____/\_/         \____/\_/`

var (
	Version     = "v0.0.0"
	VersionMeta = "dev"
	GitCommit   string
	GitDate     string

	envPrefix = "OP_UP"
	dirFlag   = &cli.PathFlag{
		Name:    "dir",
		Usage:   "the path to the op-up directory, which is used for caching among other things.",
		EnvVars: opservice.PrefixEnvVar(envPrefix, "DIR"),
		Value: func() string {
			parentDir, err := os.UserHomeDir()
			if err != nil {
				parentDir, err = os.Getwd()
				if err != nil {
					return "error: could not find home or working directories"
				}
			}
			return filepath.Join(parentDir, ".op-up")
		}(),
	}
)

func main() {
	ctx, cancel := signal.NotifyContext(context.Background(), syscall.SIGTERM, os.Interrupt)
	defer cancel()
	if err := run(ctx, os.Args, os.Stdout, os.Stderr); err != nil {
		fmt.Fprintf(os.Stderr, "error: %v\n", err)
		os.Exit(1)
	}
}

func run(ctx context.Context, args []string, stdout, stderr io.Writer) error {
	app := cli.NewApp()
	app.Writer = stdout
	app.ErrWriter = stderr
	app.Version = opservice.FormatVersion(Version, GitCommit, GitDate, VersionMeta)
	app.Name = "op-up"
	app.Usage = "deploys an in-memory OP Stack devnet."
	app.Flags = cliapp.ProtectFlags([]cli.Flag{dirFlag})
	// The default OnUsageError behavior will print the error twice: once in the cli package and
	// once in our main function.
	// The function below prints help and returns the error for further handling/error messages.
	app.OnUsageError = func(cliCtx *cli.Context, err error, isSubcommand bool) error {
		if !cliCtx.App.HideHelp {
			_ = cli.ShowAppHelp(cliCtx)
		}
		return err
	}
	app.Action = func(cliCtx *cli.Context) error {
		return runOpUp(cliCtx.Context, cliCtx.App.ErrWriter, cliCtx.String(dirFlag.Name))
	}
	return app.RunContext(ctx, args)
}

func runOpUp(ctx context.Context, stderr io.Writer, opUpDir string) error {
	fmt.Fprintf(stderr, "%s\n", asciiArt)

	if err := os.MkdirAll(opUpDir, 0o755); err != nil {
		return fmt.Errorf("create the op-up dir: %w", err)
	}
	deployerCacheDir := filepath.Join(opUpDir, "deployer", "cache")
	if err := os.MkdirAll(deployerCacheDir, 0o755); err != nil {
		return fmt.Errorf("create the deployer cache dir: %w", err)
	}

	devtest.RootContext = ctx

	p := newP(ctx, stderr)
	defer p.Close()

	ids := sysgo.NewDefaultMinimalSystemIDs(sysgo.DefaultL1ID, sysgo.DefaultL2AID)

	// Determine L2 block gas limit: use OP_UP_GAS_LIMIT env var if set, otherwise standard default.
	l2GasLimit := standard.GasLimit
	if v := os.Getenv("OP_UP_GAS_LIMIT"); v != "" {
		parsed, err := strconv.ParseUint(v, 10, 64)
		if err != nil {
			return fmt.Errorf("invalid OP_UP_GAS_LIMIT value %q: %w", v, err)
		}
		l2GasLimit = parsed
	}

	opts := stack.Combine(
		sysgo.WithMnemonicKeys(devkeys.TestMnemonic),

		sysgo.WithDeployer(),
		sysgo.WithDeployerOptions(
			sysgo.WithEmbeddedContractSources(),
			sysgo.WithCommons(ids.L1.ChainID()),
			sysgo.WithPrefundedL2(ids.L1.ChainID(), ids.L2.ChainID()),
			sysgo.WithL2GasLimit(ids.L2.ChainID(), l2GasLimit),
		),
		sysgo.WithDeployerPipelineOption(sysgo.WithDeployerCacheDir(deployerCacheDir)),

		sysgo.WithL1Nodes(ids.L1EL, ids.L1CL),

		sysgo.WithL2ELNode(ids.L2EL),
		sysgo.WithL2CLNode(ids.L2CL, ids.L1CL, ids.L1EL, ids.L2EL, sysgo.L2CLSequencer()),
		sysgo.WithL2MetricsDashboard(),

		sysgo.WithBatcher(ids.L2Batcher, ids.L1EL, ids.L2CL, ids.L2EL),
		sysgo.WithProposer(ids.L2Proposer, ids.L1EL, &ids.L2CL, nil),

		sysgo.WithFaucets([]stack.L1ELNodeID{ids.L1EL}, []stack.L2ELNodeID{ids.L2EL}),
	)

	orch := sysgo.NewOrchestrator(p, opts)
	stack.ApplyOptionLifecycle[*sysgo.Orchestrator](opts, orch)
	if err := runSysgo(ctx, stderr, orch); err != nil {
		return err
	}
	fmt.Fprintf(stderr, "\nPlease consider filling out this survey to influence future development: https://www.surveymonkey.com/r/JTGHFK3\n")
	return nil
}

func newP(ctx context.Context, stderr io.Writer) devtest.P {
	logHandler := oplog.NewLogHandler(stderr, oplog.DefaultCLIConfig())
	logHandler = logfilter.WrapFilterHandler(logHandler)
	logHandler.(logfilter.FilterHandler).Set(logfilter.DefaultMute())
	logHandler = logfilter.WrapContextHandler(logHandler)
	logger := log.NewLogger(logHandler)
	oplog.SetGlobalLogHandler(logHandler)
	logger.SetContext(ctx)
	onFail := func(now bool) {
		logger.Error("Main failed")
		debug.PrintStack()
		if now {
			panic("critical Main fail")
		}
	}
	p := devtest.NewP(ctx, logger, onFail, func() {
		onFail(true)
	})
	return p
}

func runSysgo(ctx context.Context, stderr io.Writer, orch *sysgo.Orchestrator) error {
	// Print available account.
	hd, err := devkeys.NewMnemonicDevKeys(devkeys.TestMnemonic)
	if err != nil {
		return fmt.Errorf("new mnemonic dev keys: %w", err)
	}
	const funderIndex = 10_000 // see sysgo/deployer.go.
	funderUserKey := devkeys.UserKey(funderIndex)
	funderAddress, err := hd.Address(funderUserKey)
	if err != nil {
		return fmt.Errorf("address: %w", err)
	}
	funderPrivKey, err := hd.Secret(funderUserKey)
	if err != nil {
		return fmt.Errorf("secret: %w", err)
	}

	fmt.Fprintf(stderr, "Test Account Address: %s\n", funderAddress)
	fmt.Fprintf(stderr, "Test Account Private Key: %s\n", "0x"+common.Bytes2Hex(crypto.FromECDSA(funderPrivKey)))
	fmt.Fprintf(stderr, "EL Node URL: %s\n", "http://localhost:8545")

	t := &testingT{
		ctx:      ctx,
		cleanups: make([]func(), 0),
	}
	defer t.doCleanup()
	sys := shim.NewSystem(t)
	orch.Hydrate(sys)
	l2Networks := sys.L2Networks()
	if len(l2Networks) != 1 {
		return fmt.Errorf("need one l2 network, got: %d", len(l2Networks))
	}
	l2Net := l2Networks[0]
	elNode := l2Net.L2ELNode(match.FirstL2EL)

	// Log on new blocks.
	go func() {
		const blockPollInterval = 500 * time.Millisecond
		var lastBlock uint64
		for {
			select {
			case <-ctx.Done():
				return
			case <-time.After(blockPollInterval):
				unsafe, err := elNode.EthClient().BlockRefByLabel(ctx, eth.Unsafe)
				if err != nil {
					continue
				}
				if unsafe.Number != lastBlock {
					fmt.Fprintf(stderr, "New L2 block: number %d, hash %s\n", unsafe.Number, unsafe.Hash)
					lastBlock = unsafe.Number
				}
			}
		}
	}()

	// Proxy L2 EL requests.
	go func() {
		if err := proxyEL(stderr, elNode.L2EthClient().RPC()); err != nil {
			fmt.Fprintf(stderr, "error: %v", err)
		}
	}()

	<-ctx.Done()

	return nil
}

// parseRPCParams extracts the method and call parameters from a JSON RPC request map.
// Returns an error response map if the request is malformed.
func parseRPCParams(req map[string]any) (method string, args []any, errResp map[string]any) {
	method, ok := req["method"].(string)
	if !ok {
		return "", nil, map[string]any{
			"jsonrpc": "2.0",
			"id":      req["id"],
			"error": map[string]any{
				"code":    -32600,
				"message": "Missing or invalid 'method' field in JSON RPC request",
			},
		}
	}

	if p, ok := req["params"]; ok && p != nil {
		if arr, isArray := p.([]any); isArray {
			args = arr
		} else if obj, isObject := p.(map[string]any); isObject {
			args = []any{obj}
		} else {
			return "", nil, map[string]any{
				"jsonrpc": "2.0",
				"id":      req["id"],
				"error": map[string]any{
					"code":    -32600,
					"message": "Invalid 'params' field in JSON RPC request (must be array, object, or null)",
				},
			}
		}
	}
	return method, args, nil
}

// handleSingleRPC processes a single JSON RPC request via CallContext and returns the response.
func handleSingleRPC(ctx context.Context, stderr io.Writer, cl client.RPC, req map[string]any) map[string]any {
	method, callParams, errResp := parseRPCParams(req)
	if errResp != nil {
		return errResp
	}

	id := req["id"]
	var rpcResult json.RawMessage
	fmt.Fprintf(stderr, "%s\n", method)

	err := cl.CallContext(ctx, &rpcResult, method, callParams...)
	if err != nil {
		message := fmt.Sprintf("RPC call to backend failed for method '%s': %v", method, err)
		fmt.Fprintf(stderr, "RPC error: %s\n", message)
		return map[string]any{
			"jsonrpc": "2.0",
			"id":      id,
			"error": map[string]any{
				"code":    -32000,
				"message": message,
			},
		}
	}

	return map[string]any{
		"jsonrpc": "2.0",
		"id":      id,
		"result":  rpcResult,
	}
}

// handleBatchRPC processes a batch of JSON RPC requests via BatchCallContext
// and returns the responses as a slice of maps.
func handleBatchRPC(ctx context.Context, stderr io.Writer, cl client.RPC, reqs []map[string]any) []map[string]any {
	// First pass: parse all requests and build BatchElems for valid ones.
	type parsed struct {
		id     any
		method string
		elem   *rpc.BatchElem // nil if the request was malformed
		errRes map[string]any // non-nil if the request was malformed
	}
	items := make([]parsed, len(reqs))
	var batchElems []rpc.BatchElem
	// Map from batchElems index back to items index.
	var batchIdx []int

	for i, req := range reqs {
		method, args, errResp := parseRPCParams(req)
		if errResp != nil {
			items[i] = parsed{id: req["id"], errRes: errResp}
			continue
		}
		fmt.Fprintf(stderr, "%s\n", method)
		result := new(json.RawMessage)
		elem := rpc.BatchElem{
			Method: method,
			Args:   args,
			Result: result,
		}
		items[i] = parsed{id: req["id"], method: method, elem: &elem}
		batchIdx = append(batchIdx, i)
		batchElems = append(batchElems, elem)
	}

	// Send all valid requests as a single batch to the upstream.
	if len(batchElems) > 0 {
		if err := cl.BatchCallContext(ctx, batchElems); err != nil {
			// If the entire batch call fails, mark all valid requests as errors.
			fmt.Fprintf(stderr, "RPC batch error: %v\n", err)
			for _, idx := range batchIdx {
				items[idx].errRes = map[string]any{
					"jsonrpc": "2.0",
					"id":      items[idx].id,
					"error": map[string]any{
						"code":    -32000,
						"message": fmt.Sprintf("RPC batch call to backend failed: %v", err),
					},
				}
				items[idx].elem = nil
			}
		} else {
			// Copy per-element results/errors back.
			for j, idx := range batchIdx {
				elem := batchElems[j]
				if elem.Error != nil {
					message := fmt.Sprintf("RPC call to backend failed for method '%s': %v", elem.Method, elem.Error)
					fmt.Fprintf(stderr, "RPC error: %s\n", message)
					items[idx].errRes = map[string]any{
						"jsonrpc": "2.0",
						"id":      items[idx].id,
						"error": map[string]any{
							"code":    -32000,
							"message": message,
						},
					}
					items[idx].elem = nil
				}
			}
		}
	}

	// Build final response array.
	responses := make([]map[string]any, len(reqs))
	for i, item := range items {
		if item.errRes != nil {
			responses[i] = item.errRes
		} else {
			responses[i] = map[string]any{
				"jsonrpc": "2.0",
				"id":      item.id,
				"result":  *(item.elem.Result.(*json.RawMessage)),
			}
		}
	}
	return responses
}

// proxyEL is a hacky way to intercept EL json rpc requests for logging to get around log filtering
// bugs.
func proxyEL(stderr io.Writer, cl client.RPC) error {
	// Set up the HTTP handler for all incoming requests.
	http.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		// Ensure the request method is POST, as JSON RPC typically uses POST.
		if r.Method != http.MethodPost {
			http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
			return
		}

		// Read the entire request body.
		requestBody, err := io.ReadAll(r.Body)
		if err != nil {
			http.Error(w, "Failed to read request body", http.StatusInternalServerError)
			return
		}
		defer r.Body.Close()

		// Create a context with a timeout for the RPC call(s) to the backend.
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()

		// Detect whether this is a batch request (JSON array) or a single request (JSON object).
		// Per JSON-RPC 2.0 spec, batch requests are sent as a JSON array of request objects.
		var jsonResponse []byte

		// Trim whitespace to find the first significant character.
		trimmed := bytes.TrimLeft(requestBody, " \t\r\n")
		if len(trimmed) > 0 && trimmed[0] == '[' {
			// Batch request: parse as array of request objects.
			var reqs []map[string]any
			if err := json.Unmarshal(requestBody, &reqs); err != nil {
				http.Error(w, "Invalid JSON RPC batch request format", http.StatusBadRequest)
				return
			}
			if len(reqs) == 0 {
				// Per JSON-RPC 2.0 spec, an empty batch is an invalid request.
				errResp := map[string]any{
					"jsonrpc": "2.0",
					"id":      nil,
					"error": map[string]any{
						"code":    -32600,
						"message": "Invalid Request: empty batch",
					},
				}
				jsonResponse, _ = json.Marshal(errResp)
			} else {
				responses := handleBatchRPC(ctx, stderr, cl, reqs)
				jsonResponse, err = json.Marshal(responses)
				if err != nil {
					http.Error(w, "Failed to marshal RPC batch response", http.StatusInternalServerError)
					return
				}
			}
		} else {
			// Single request: parse as a single request object.
			var req map[string]any
			if err := json.Unmarshal(requestBody, &req); err != nil {
				http.Error(w, "Invalid JSON RPC request format", http.StatusBadRequest)
				return
			}
			resp := handleSingleRPC(ctx, stderr, cl, req)
			jsonResponse, err = json.Marshal(resp)
			if err != nil {
				http.Error(w, "Failed to marshal RPC response", http.StatusInternalServerError)
				return
			}
		}

		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write(jsonResponse)
	})

	// Start the HTTP server.
	if err := http.ListenAndServe("localhost:8545", nil); err != nil {
		return fmt.Errorf("listen and server: %w", err)
	}
	return nil
}

type testingT struct {
	mu       sync.Mutex
	ctx      context.Context
	cleanups []func()
}

var _ devtest.T = (*testingT)(nil)
var _ testreq.TestingT = (*testingT)(nil)

func (t *testingT) doCleanup() {
	t.mu.Lock()
	defer t.mu.Unlock()
	for _, cleanup := range slices.Backward(t.cleanups) {
		cleanup()
	}
}

// Cleanup implements devtest.T.
func (t *testingT) Cleanup(fn func()) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.cleanups = append(t.cleanups, fn)
}

// Ctx implements devtest.T.
func (t *testingT) Ctx() context.Context {
	return t.ctx
}

// Deadline implements devtest.T.
func (t *testingT) Deadline() (deadline time.Time, ok bool) {
	return time.Time{}, false
}

// Error implements devtest.T.
func (t *testingT) Error(args ...any) {
}

// Errorf implements devtest.T.
func (t *testingT) Errorf(format string, args ...any) {
}

// Fail implements devtest.T.
func (t *testingT) Fail() {
}

// FailNow implements devtest.T.
func (t *testingT) FailNow() {
}

// Gate implements devtest.T.
func (t *testingT) Gate() *testreq.Assertions {
	return testreq.New(t)
}

// Helper implements devtest.T.
func (t *testingT) Helper() {
}

// Log implements devtest.T.
func (t *testingT) Log(args ...any) {
}

// Logf implements devtest.T.
func (t *testingT) Logf(format string, args ...any) {
}

func (t *testingT) Logger() log.Logger {
	return log.NewLogger(slog.NewTextHandler(io.Discard, nil))
}

func (t *testingT) Name() string {
	return "dev"
}

func (t *testingT) Parallel() {
}

func (t *testingT) Require() *testreq.Assertions {
	return testreq.New(t)
}

func (t *testingT) Run(name string, fn func(devtest.T)) {
	panic("unimplemented")
}

func (t *testingT) Skip(args ...any) {
	panic("unimplemented")
}

func (t *testingT) SkipNow() {
	panic("unimplemented")
}

// Skipf implements devtest.T.
func (t *testingT) Skipf(format string, args ...any) {
	panic("unimplemented")
}

// Skipped implements devtest.T.
func (t *testingT) Skipped() bool {
	return false
}

// TempDir implements devtest.T.
func (t *testingT) TempDir() string {
	panic("unimplemented")
}

// Tracer implements devtest.T.
func (t *testingT) Tracer() trace.Tracer {
	panic("unimplemented")
}

// WithCtx implements devtest.T.
func (t *testingT) WithCtx(ctx context.Context) devtest.T {
	return t
}

// _TestOnly implements devtest.T.
func (t *testingT) TestOnly() {
}
