package sources

import (
	"context"
	"fmt"

	"github.com/ethereum/go-ethereum/common"

	"github.com/ethereum-optimism/optimism/op-service/client"
)

// XLayerClient wraps an RPC client to call xlayer_ namespace methods.
// Shares the same underlying RPC connection as RollupClient.
type XLayerClient struct {
	rpc client.RPC
}

func NewXLayerClient(rpc client.RPC) *XLayerClient {
	return &XLayerClient{rpc: rpc}
}

// XLayerRuntimeConfigResponse is the response from xlayer_runtimeConfig.
type XLayerRuntimeConfigResponse struct {
	P2PSequencerAddress common.Address `json:"p2pSequencerAddress"`
}

func (c *XLayerClient) RuntimeConfig(ctx context.Context) (*XLayerRuntimeConfigResponse, error) {
	var result XLayerRuntimeConfigResponse
	err := c.rpc.CallContext(ctx, &result, "xlayer_runtimeConfig")
	if err != nil {
		return nil, fmt.Errorf("failed to fetch xlayer_runtimeConfig: %w", err)
	}
	return &result, nil
}
