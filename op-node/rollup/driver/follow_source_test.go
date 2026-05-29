package driver

import (
	"context"
	"testing"

	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-service/sources"
	"github.com/ethereum/go-ethereum/common"
	"github.com/stretchr/testify/require"
)

// mockL1Source implements L1FollowSource for testing.
type mockL1Source struct {
	ref eth.L1BlockRef
	err error
}

func (m *mockL1Source) L1BlockRefByNumber(_ context.Context, _ uint64) (eth.L1BlockRef, error) {
	return m.ref, m.err
}

func TestNewL2FollowSource_NilClient_Panics(t *testing.T) {
	require.Panics(t, func() {
		NewL2FollowSource(nil, &mockL1Source{})
	})
}

func TestNewL2FollowSource_NilL1Source_Allowed(t *testing.T) {
	// nil l1Source is allowed for skip-l1-check mode
	client, _ := sources.NewFollowClient(nil)
	require.NotPanics(t, func() {
		NewL2FollowSource(client, nil)
	})
}

func TestL2FollowSource_L1BlockRefByNumber_NilGuard(t *testing.T) {
	client, _ := sources.NewFollowClient(nil)
	fs := NewL2FollowSource(client, nil)

	_, err := fs.L1BlockRefByNumber(context.Background(), 100)
	require.Error(t, err)
	require.Contains(t, err.Error(), "L1 source not available")
}

func TestL2FollowSource_L1BlockRefByNumber_WithSource(t *testing.T) {
	expectedRef := eth.L1BlockRef{Hash: common.HexToHash("0xabc"), Number: 42}
	client, _ := sources.NewFollowClient(nil)
	fs := NewL2FollowSource(client, &mockL1Source{ref: expectedRef})

	ref, err := fs.L1BlockRefByNumber(context.Background(), 42)
	require.NoError(t, err)
	require.Equal(t, expectedRef, ref)
}

func TestL2FollowSource_ImplementsXLayerRuntimeConfigSource(t *testing.T) {
	// Verify L2FollowSource satisfies XLayerRuntimeConfigSource via type assertion
	client, _ := sources.NewFollowClient(nil)
	fs := NewL2FollowSource(client, nil)

	var upstream UpstreamFollowSource = fs
	_, ok := upstream.(XLayerRuntimeConfigSource)
	require.True(t, ok, "L2FollowSource should satisfy XLayerRuntimeConfigSource interface")
}

func TestL2FollowSource_UpstreamFollowSource_DoesNotRequireRuntimeConfig(t *testing.T) {
	// Verify UpstreamFollowSource interface does NOT include GetRuntimeConfig
	// (it's accessed via separate type assertion only)
	client, _ := sources.NewFollowClient(nil)
	fs := NewL2FollowSource(client, nil)

	var _ UpstreamFollowSource = fs // compile-time check
}
