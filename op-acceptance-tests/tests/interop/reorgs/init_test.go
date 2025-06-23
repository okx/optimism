package reorgs

import (
	"log/slog"
	"testing"

	"github.com/ethereum-optimism/optimism/op-devstack/presets"
	"github.com/ethereum-optimism/optimism/op-service/log/logfilter"
)

// TestMain creates the test-setups against the shared backend
func TestMain(m *testing.M) {
	// Other setups may be added here, hydrated from the same orchestrator
	presets.DoMain(m, presets.WithSimpleInterop(), presets.WithLogFilter(logfilter.DefaultMute(logfilter.LevelExact(slog.LevelError).Show())))
}
