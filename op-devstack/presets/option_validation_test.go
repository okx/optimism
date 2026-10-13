package presets

import (
	"testing"

	"github.com/stretchr/testify/require"

	"github.com/ethereum-optimism/optimism/op-devstack/sysgo"
)

func TestXLayerFixedPortsValidation(t *testing.T) {
	t.Run("valid distinct ports", func(t *testing.T) {
		plan := sysgo.XLayerFixedPortPlan{
			L1HTTPPort:        8545,
			ELHTTPPorts:       [3]int{8123, 8124, 8125},
			ConductorRPCPorts: [3]int{8547, 8548, 8549},
		}
		require.NoError(t, validateXLayerFixedPorts(plan))
	})
	t.Run("all zero ports are valid", func(t *testing.T) {
		require.NoError(t, validateXLayerFixedPorts(sysgo.XLayerFixedPortPlan{}))
	})
	t.Run("out-of-range port high", func(t *testing.T) {
		plan := sysgo.XLayerFixedPortPlan{L1HTTPPort: 65536}
		require.Error(t, validateXLayerFixedPorts(plan))
	})
	t.Run("negative port", func(t *testing.T) {
		plan := sysgo.XLayerFixedPortPlan{L1HTTPPort: -1}
		require.Error(t, validateXLayerFixedPorts(plan))
	})
	t.Run("duplicate L1 and EL port", func(t *testing.T) {
		plan := sysgo.XLayerFixedPortPlan{
			L1HTTPPort:  8545,
			ELHTTPPorts: [3]int{8545, 8124, 8125},
		}
		require.Error(t, validateXLayerFixedPorts(plan))
	})
	t.Run("duplicate within EL ports", func(t *testing.T) {
		plan := sysgo.XLayerFixedPortPlan{
			ELHTTPPorts: [3]int{8123, 8123, 8125},
		}
		require.Error(t, validateXLayerFixedPorts(plan))
	})
	t.Run("duplicate conductor and EL port", func(t *testing.T) {
		plan := sysgo.XLayerFixedPortPlan{
			ELHTTPPorts:       [3]int{8123, 8124, 8125},
			ConductorRPCPorts: [3]int{8123, 8548, 8549},
		}
		require.Error(t, validateXLayerFixedPorts(plan))
	})
}

func TestWithXLayerFixedPortsOptionKind(t *testing.T) {
	opt := WithXLayerFixedPorts(sysgo.XLayerFixedPortPlan{
		L1HTTPPort:        8545,
		ELHTTPPorts:       [3]int{8123, 8124, 8125},
		ConductorRPCPorts: [3]int{8547, 8548, 8549},
	})
	kinds := opt.optionKinds()
	require.NotZero(t, kinds&optionKindXLayerFixedPorts, "WithXLayerFixedPorts must set optionKindXLayerFixedPorts")
	require.Zero(t, kinds&^optionKindXLayerFixedPorts, "WithXLayerFixedPorts must only set optionKindXLayerFixedPorts")
}
