package driver

import (
	"context"

	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-service/sources"
)

// L1FollowSource provides access to L1 block references for upstream following.
type L1FollowSource interface {
	L1BlockRefByNumber(ctx context.Context, num uint64) (eth.L1BlockRef, error)
}

// UpstreamFollowSource combines L1 and L2 follow sources.
// L2 following may be optionally disabled.
type UpstreamFollowSource interface {
	L1FollowSource
	GetFollowStatus(ctx context.Context) (*sources.FollowStatus, error)
	// XLayer: fetch runtime config from upstream (P2PSequencerAddress)
	GetRuntimeConfig(ctx context.Context) (*sources.XLayerRuntimeConfigResponse, error)
}

type L2FollowSource struct {
	l2Source *sources.FollowClient
	l1Source L1FollowSource
}

var _ UpstreamFollowSource = (*L2FollowSource)(nil)

func NewL2FollowSource(client *sources.FollowClient, l1Source L1FollowSource) *L2FollowSource {
	if client == nil {
		panic("NewL2FollowSource: l2Source must not be nil")
	}
	// l1Source may be nil when skip-l1-check is enabled (L1 verification is skipped)
	return &L2FollowSource{l2Source: client, l1Source: l1Source}
}

func (fs *L2FollowSource) GetFollowStatus(ctx context.Context) (*sources.FollowStatus, error) {
	return fs.l2Source.GetFollowStatus(ctx)
}

func (fs *L2FollowSource) L1BlockRefByNumber(ctx context.Context, num uint64) (eth.L1BlockRef, error) {
	return fs.l1Source.L1BlockRefByNumber(ctx, num)
}

func (fs *L2FollowSource) GetRuntimeConfig(ctx context.Context) (*sources.XLayerRuntimeConfigResponse, error) {
	return fs.l2Source.GetRuntimeConfig(ctx)
}
