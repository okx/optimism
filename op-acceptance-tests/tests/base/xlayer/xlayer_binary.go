package xlayer

import (
	"os"
	"os/exec"
	"strings"
	"testing"
)

// requireXLayerExecutionBinary resolves the XLayer execution client from
// XLAYER_RETH_BIN and records the version it reports.
//
// Every test in this package that builds the XLayer topology depends on that
// build: the producer and flashblocks flags the topology configures exist only
// in it, and the sequencer semantics the suite verifies are its semantics. When
// the binary is absent or cannot report a version there is nothing to verify,
// so the caller is BLOCKED — the test skips with that reason and can never
// report a pass. Letting an XLayer-named test go green without ever touching an
// XLayer client would misrepresent coverage, which is worse than reporting the
// gap.
func requireXLayerExecutionBinary(t *testing.T) string {
	t.Helper()
	path := os.Getenv("XLAYER_RETH_BIN")
	if path == "" {
		t.Skip("BLOCKED: XLAYER_RETH_BIN not available — cannot validate XLayer acceptance")
	}
	out, err := exec.Command(path, "--version").CombinedOutput()
	if err != nil {
		t.Skipf("BLOCKED: XLAYER_RETH_BIN at %s cannot report a version: %v (%s)",
			path, err, strings.TrimSpace(string(out)))
	}
	version := strings.TrimSpace(string(out))
	if version == "" {
		t.Skipf("BLOCKED: XLAYER_RETH_BIN at %s reported an empty version string", path)
	}
	t.Logf("XLayer execution binary: path=%s version=%s", path, version)
	return path
}
