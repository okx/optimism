package signer

import (
	"context"
	"fmt"
	"math/big"
	"time"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/log"
)

// XLayerSignerClient implements SignerClient interface using XLayer remote signing service
type XLayerSignerClient struct {
	logger log.Logger
	client *XLayerRemoteClient
	config XLayerConfig
}

// NewXLayerSignerClient creates a new XLayer signing client
func NewXLayerSignerClient(logger log.Logger, config XLayerConfig) (*XLayerSignerClient, error) {
	client := NewXLayerRemoteClient(logger, config)

	return &XLayerSignerClient{
		logger: logger,
		client: client,
		config: config,
	}, nil
}

func (s *XLayerSignerClient) SignTransaction(ctx context.Context, chainId *big.Int, from common.Address, tx *types.Transaction) (*types.Transaction, error) {
	s.logger.Debug("XLayer remote signing transaction",
		"type", tx.Type(),
		"nonce", tx.Nonce(),
		"to", tx.To(),
		"value", tx.Value(),
		"gas", tx.Gas(),
		"chainId", chainId)

	expectedFrom := common.HexToAddress(s.config.Address)
	if from != expectedFrom {
		return nil, fmt.Errorf("signing address mismatch: expected %s, got %s", expectedFrom.Hex(), from.Hex())
	}

	signedTx, err := s.client.SignTransaction(ctx, chainId, from, tx)
	if err != nil {
		return nil, fmt.Errorf("XLayer remote signing failed: %w", err)
	}

	s.logger.Info("XLayer remote signing completed",
		"txHash", signedTx.Hash().Hex(),
		"type", signedTx.Type())

	return signedTx, nil
}

func (s *XLayerSignerClient) SignBlockPayload(ctx context.Context, args *BlockPayloadArgs) ([65]byte, error) {
	return [65]byte{}, fmt.Errorf("XLayer signer does not support block payload signing")
}

func (s *XLayerSignerClient) SignBlockPayloadV2(ctx context.Context, args *BlockPayloadArgsV2) ([65]byte, error) {
	return [65]byte{}, fmt.Errorf("XLayer signer does not support block payload signing V2")
}

func (s *XLayerSignerClient) Close() {
	if s.client != nil {
		s.client.Close()
	}
}

// StartVerifyServer starts the refOrderId verify HTTP server (For xlayer).
// The server exposes GET /signer/get?refOrderId=<id> so the asset management
// service can confirm the ID before completing the signature.
// Returns an error if addr is empty or the server fails to bind.
func (s *XLayerSignerClient) StartVerifyServer(logger log.Logger, addr string) (*XLayerSignerVerifyServer, error) {
	if addr == "" {
		return nil, fmt.Errorf("verify server address must not be empty")
	}
	return NewXLayerSignerVerifyServer(logger, addr, s.client.HasRefOrderID)
}

// XLayerCLIConfig contains CLI configuration for XLayer remote signer
type XLayerCLIConfig struct {
	Enabled       bool   `json:"enabled"`
	Endpoint      string `json:"endpoint"`
	Address       string `json:"address"`
	UserID        int    `json:"userId"`
	Symbol        int    `json:"symbol"`
	ProjectSymbol int    `json:"projectSymbol"`
	OperateSymbol int    `json:"operateSymbol"`
	OperateAmount string `json:"operateAmount"`
	SysFrom       int    `json:"sysFrom"`
	AccessKey     string `json:"accessKey"`
	SecretKey     string `json:"secretKey"`
	Timeout       string `json:"timeout"`
	EnableKMS     bool   `json:"enableKms"`
	// For xlayer: listen address for the refOrderId verify server (e.g. "0.0.0.0:8546").
	// Leave empty to disable the server.
	VerifyAddr string `json:"verifyAddr"`
}

// NewXLayerCLIConfig creates a new XLayer CLI configuration with default values
func NewXLayerCLIConfig() XLayerCLIConfig {
	return XLayerCLIConfig{
		Enabled:       false,
		Symbol:        2882, // Default value for devnet
		ProjectSymbol: 3011,
		OperateSymbol: 2,
		OperateAmount: "0",
		SysFrom:       3,
		Timeout:       "30s",
	}
}

// Check validates the XLayer configuration
func (c XLayerCLIConfig) Check() error {
	if !c.Enabled {
		return nil
	}

	if c.Endpoint == "" {
		return fmt.Errorf("XLayer endpoint is required when enabled")
	}

	if c.Address == "" {
		return fmt.Errorf("XLayer address is required when enabled")
	}

	return nil
}

// ToXLayerConfig converts CLI config to XLayerConfig
func (c XLayerCLIConfig) ToXLayerConfig() (XLayerConfig, error) {
	if err := c.Check(); err != nil {
		return XLayerConfig{}, err
	}

	timeout, err := time.ParseDuration(c.Timeout)
	if err != nil {
		timeout = 30 * time.Second
	}

	return XLayerConfig{
		Endpoint:       c.Endpoint,
		Address:        c.Address,
		UserID:         c.UserID,
		Symbol:         c.Symbol,
		ProjectSymbol:  c.ProjectSymbol,
		OperateSymbol:  c.OperateSymbol,
		OperateAmount:  c.OperateAmount,
		SysFrom:        c.SysFrom,
		RequestSignURI: "/priapi/v1/assetonchain/ecology/ecologyOperate",
		QuerySignURI:   "/priapi/v1/assetonchain/ecology/querySignDataByOrderNo",
		AccessKey:      c.AccessKey,
		SecretKey:      c.SecretKey,
		Timeout:        timeout,
	}, nil
}

// NewXLayerSignerClientFromConfig creates an XLayer signing client from CLI configuration
func NewXLayerSignerClientFromConfig(logger log.Logger, config XLayerCLIConfig) (*XLayerSignerClient, error) {
	if !config.Enabled {
		return nil, fmt.Errorf("XLayer signer is not enabled")
	}

	// SecretKey holds the KMS secret name; replace it with the fetched plaintext value.
	if config.EnableKMS {
		resolved, err := resolveSecretKeyFromKMS(logger, config.SecretKey)
		if err != nil {
			return nil, fmt.Errorf("failed to resolve secret key from KMS: %w", err)
		}
		config.SecretKey = resolved
	}

	xlayerConfig, err := config.ToXLayerConfig()
	if err != nil {
		return nil, fmt.Errorf("invalid XLayer config: %w", err)
	}

	return NewXLayerSignerClient(logger, xlayerConfig)
}
