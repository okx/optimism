package sources

import (
	"context"
	"fmt"

	"github.com/ethereum-optimism/optimism/op-service/client"
	"github.com/ethereum-optimism/optimism/op-service/eth"
)

type FollowClient struct {
	rollupClient *RollupClient
	xlayerClient *XLayerClient
}

type FollowStatus struct {
	SafeL2      eth.L2BlockRef
	FinalizedL2 eth.L2BlockRef
	CurrentL1   eth.L1BlockRef

	// XLayer: L1 state from upstream, used when skip-l1-check is enabled
	HeadL1      eth.L1BlockRef
	SafeL1      eth.L1BlockRef
	FinalizedL1 eth.L1BlockRef
}

func NewFollowClient(rpcClient client.RPC) (*FollowClient, error) {
	return &FollowClient{
		rollupClient: NewRollupClient(rpcClient),
		xlayerClient: NewXLayerClient(rpcClient),
	}, nil
}

func (s *FollowClient) GetFollowStatus(ctx context.Context) (*FollowStatus, error) {
	status, err := s.rollupClient.SyncStatus(ctx)
	if err != nil {
		return nil, fmt.Errorf("failed to fetch external syncStatus: %w", err)
	}
	return &FollowStatus{
		FinalizedL2: status.FinalizedL2,
		SafeL2:      status.SafeL2,
		CurrentL1:   status.CurrentL1,
		HeadL1:      status.HeadL1,
		SafeL1:      status.SafeL1,
		FinalizedL1: status.FinalizedL1,
	}, nil
}

func (s *FollowClient) GetRuntimeConfig(ctx context.Context) (*XLayerRuntimeConfigResponse, error) {
	return s.xlayerClient.RuntimeConfig(ctx)
}
