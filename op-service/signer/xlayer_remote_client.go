package signer

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"math/big"
	"net/http"
	"net/url"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/common/hexutil"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/log"
	"github.com/google/uuid"
)

// Contract method signatures (4-byte selectors)
const (
	// DisputeGameFactory.create(uint32 _gameType, bytes32 _rootClaim, bytes calldata _extraData)
	MethodSigDGFCreate = "0x82ecf2f6"

	// FaultDisputeGame.resolveClaim(uint256 _claimIndex, uint256 _numToResolve)
	MethodSigResolveClaim = "0x03c2924d"

	// FaultDisputeGame.resolve()
	MethodSigResolve = "0x2810e1d6"

	// FaultDisputeGame.claimCredit(address _recipient)
	MethodSigClaimCredit = "0x60e27464"
)

// OperateType represents the operation type for XLayer remote signer
type OperateType int

const (
	// OperateTypeLegacy represents legacy transaction operation type
	OperateTypeLegacy OperateType = 0

	// OperateTypeDynamicFee represents EIP-1559 dynamic fee transaction operation type
	OperateTypeDynamicFee OperateType = 1

	// OperateTypeBatcherLegacy represents batcher legacy/EIP-1559 transaction operation type
	OperateTypeBatcherLegacy OperateType = 3

	// OperateTypeBatcherBlob represents batcher EIP-4844 blob transaction operation type
	OperateTypeBatcherBlob OperateType = 19

	// OperateTypeProposer represents proposer transaction operation type
	OperateTypeProposer OperateType = 20

	// OperateTypeChallengerResolveClaim represents challenger resolveClaim operation type
	OperateTypeChallengerResolveClaim OperateType = 21

	// OperateTypeChallengerResolve represents challenger resolve operation type
	OperateTypeChallengerResolve OperateType = 22

	// OperateTypeChallengerClaimCredit represents challenger claimCredit operation type
	OperateTypeChallengerClaimCredit OperateType = 23
)

// ComponentRole represents the role/type of blockchain component
type ComponentRole string

const (
	// ComponentRoleBatcher represents the batcher component
	ComponentRoleBatcher ComponentRole = "batcher"

	// ComponentRoleProposer represents the proposer component
	ComponentRoleProposer ComponentRole = "proposer"

	// ComponentRoleChallenger represents the challenger component
	ComponentRoleChallenger ComponentRole = "challenger"

	// ComponentRoleUnknown represents an unknown component
	ComponentRoleUnknown ComponentRole = "unknown"
)

// Polling configuration constants
const (
	// SignResultPollInterval is the polling interval for querying signing results
	SignResultPollInterval = 1 * time.Second

	// MaxPollAttempts is the maximum number of polling attempts before giving up
	// With 1 second interval, 120 attempts = 2 minutes max wait time
	MaxPollAttempts = 120
)

// Response status codes
const (
	// HTTPStatusSuccess represents successful HTTP response status
	HTTPStatusSuccess = 200
)

// XLayerSignRequest represents the signing request structure for XLayer remote signer API
type XLayerSignRequest struct {
	UserID          int            `json:"userId"`
	OperateType     OperateType    `json:"operateType"` // Use OperateType enum for type safety
	OperateAddress  common.Address `json:"operateAddress"`
	Symbol          int            `json:"symbol"`
	ProjectSymbol   int            `json:"projectSymbol"`
	RefOrderID      string         `json:"refOrderId"`
	OperateSymbol   int            `json:"operateSymbol"`
	OperateAmount   string         `json:"operateAmount"`
	SysFrom         int            `json:"sysFrom"`
	OtherInfo       string         `json:"otherInfo"` // JSON-encoded transaction parameters
	DepositeAddress string         `json:"depositeAddress"`
	ToAddress       string         `json:"toAddress"`
	BatchID         int            `json:"batchId"`
}

// XLayerSignResponse represents the signing response structure
type XLayerSignResponse struct {
	Code           int    `json:"code"`
	Data           string `json:"data"` // Signed transaction hex data
	DetailMessages string `json:"detailMsg"`
	Msg            string `json:"msg"`
	Status         int    `json:"status"`
	Success        bool   `json:"success"`
}

// XLayerQueryRequest represents the query request for signature result
type XLayerQueryRequest struct {
	UserID        int    `json:"userId"`
	OrderID       string `json:"orderId"`
	ProjectSymbol int    `json:"projectSymbol"`
}

// XLayerOtherInfo contains transaction parameters for OtherInfo field
type XLayerOtherInfo struct {
	ContractAddress string `json:"contractAddress"`
	GasLimit        uint64 `json:"gasLimit"`
	GasPrice        string `json:"gasPrice"`
	Nonce           uint64 `json:"nonce"`
	// EIP-4844 specific parameters
	BlobVersionedHashes []common.Hash `json:"blobVersionedHashes,omitempty"`
	BlobFeeCap          string        `json:"maxFeePerBlobGas,omitempty"`
	// EIP-1559 parameters
	MaxFeePerGas         string `json:"maxFeePerGas,omitempty"`
	MaxPriorityFeePerGas string `json:"maxPriorityFeePerGas,omitempty"`
	// Transaction data
	TxData string `json:"data,omitempty"`
	Value  string `json:"value,omitempty"`
}

// XLayerRemoteClient is the client for XLayer remote signing service
// It is safe for concurrent use by multiple goroutines.
type XLayerRemoteClient struct {
	logger   log.Logger
	endpoint string
	config   XLayerConfig
	client   *http.Client
	mu       sync.Mutex // Protects against concurrent signing requests to ensure serialization
}

// XLayerConfig contains configuration for XLayer remote signer
type XLayerConfig struct {
	Endpoint        string
	Address         string
	UserID          int
	Symbol          int
	ProjectSymbol   int
	OperateSymbol   int
	OperateAmount   string
	SysFrom         int
	RequestSignURI  string
	QuerySignURI    string
	DepositeAddress string
	AccessKey       string
	SecretKey       string
	Timeout         time.Duration
}

// NewXLayerRemoteClient creates a new XLayer remote signing client
func NewXLayerRemoteClient(logger log.Logger, config XLayerConfig) *XLayerRemoteClient {
	return &XLayerRemoteClient{
		logger:   logger,
		endpoint: config.Endpoint,
		config:   config,
		client: &http.Client{
			Timeout: config.Timeout,
		},
	}
}

// SignTransaction signs a transaction using XLayer remote signing service
// This method is safe for concurrent use - requests are serialized internally to prevent
// concurrent calls to the remote signing service, which may not support parallel requests.
func (c *XLayerRemoteClient) SignTransaction(ctx context.Context, chainId *big.Int, from common.Address, tx *types.Transaction) (*types.Transaction, error) {
	// Validate input parameters
	if tx == nil {
		return nil, fmt.Errorf("transaction is nil")
	}

	// Serialize all signing requests to prevent concurrent calls to remote signer
	c.mu.Lock()
	defer c.mu.Unlock()

	c.logger.Debug("Acquired signing lock, proceeding with remote signing",
		"from", from.Hex(),
		"nonce", tx.Nonce(),
		"to", tx.To())

	// 1. Extract blob sidecar if it's a blob transaction
	sidecar := tx.BlobTxSidecar()

	// 2. Determine component type by sender address and build OtherInfo
	var otherInfo string
	var operateType OperateType
	var err error
	componentType := c.detectComponentType(tx)
	switch componentType {
	case ComponentRoleBatcher:
		otherInfo, err = c.buildBatcherOtherInfo(tx)
		if err != nil {
			return nil, fmt.Errorf("failed to build batcher other info: %w", err)
		}
		operateType = c.getBatcherOperateType(tx)

	case ComponentRoleProposer:
		otherInfo, err = c.buildProposerOtherInfo(tx)
		if err != nil {
			return nil, fmt.Errorf("failed to build proposer other info: %w", err)
		}
		operateType = OperateTypeProposer

	case ComponentRoleChallenger:
		otherInfo, err = c.buildChallengerOtherInfo(tx)
		if err != nil {
			return nil, fmt.Errorf("failed to build challenger other info: %w", err)
		}
		operateType = c.getChallengerOperateType(tx)

	default:
		// Unknown component type, reject to avoid sending incorrect operateType
		// which could cause the remote signer to process incorrectly or block the address
		var methodSig string
		if len(tx.Data()) >= 4 {
			methodSig = hexutil.Encode(tx.Data()[:4])
		}
		c.logger.Error("Unknown component type detected, refusing to sign transaction",
			"componentType", componentType,
			"txType", tx.Type(),
			"txHash", tx.Hash().Hex(),
			"from", from.Hex(),
			"to", func() string {
				if tx.To() == nil {
					return "<nil>"
				}
				return tx.To().Hex()
			}(),
			"nonce", tx.Nonce(),
			"value", func() string {
				if tx.Value() != nil {
					return tx.Value().String()
				}
				return "<nil>"
			}(),
			"gas", tx.Gas(),
			"gasPrice", func() string {
				if tx.GasPrice() != nil {
					return tx.GasPrice().String()
				}
				return "<nil>"
			}(),
			"gasTipCap", func() string {
				if tx.GasTipCap() != nil {
					return tx.GasTipCap().String()
				}
				return "<nil>"
			}(),
			"gasFeeCap", func() string {
				if tx.GasFeeCap() != nil {
					return tx.GasFeeCap().String()
				}
				return "<nil>"
			}(),
			"dataLen", len(tx.Data()),
			"methodSig", methodSig,
			"data", hexutil.Encode(tx.Data()),
			"blobHashes", len(tx.BlobHashes()),
			"chainId", func() string {
				if chainId != nil {
					return chainId.String()
				}
				return "<nil>"
			}())
		toAddr := "<nil>"
		if tx.To() != nil {
			toAddr = tx.To().Hex()
		}
		return nil, fmt.Errorf("unknown component type %q: refusing to sign transaction (txType=%d, to=%s, nonce=%d, methodSig=%s, dataLen=%d) to prevent address blocking",
			componentType, tx.Type(), toAddr, tx.Nonce(), methodSig, len(tx.Data()))
	}

	toAddress := ""
	depositeAddress := ""
	if tx.To() != nil {
		toAddress = strings.ToLower(tx.To().Hex())
		depositeAddress = strings.ToLower(tx.To().Hex())
	}

	operateAmount := convertValueToOperateAmount(tx.Value())

	fromLower := common.HexToAddress(strings.ToLower(from.Hex()))

	// Generate UUID for order tracking (using NewRandom to avoid potential panic)
	refOrderID, err := uuid.NewRandom()
	if err != nil {
		return nil, fmt.Errorf("failed to generate order ID: %w", err)
	}

	signReq := &XLayerSignRequest{
		UserID:          c.config.UserID,
		OperateType:     operateType,
		OperateAddress:  fromLower,
		Symbol:          c.config.Symbol,
		ProjectSymbol:   c.config.ProjectSymbol,
		RefOrderID:      refOrderID.String(),
		OperateSymbol:   c.config.OperateSymbol,
		OperateAmount:   operateAmount,
		SysFrom:         c.config.SysFrom,
		OtherInfo:       otherInfo,
		DepositeAddress: depositeAddress,
		ToAddress:       toAddress,
		BatchID:         0,
	}

	// Log signing request details
	c.logger.Info("Sending sign request to remote signer",
		"operateType", operateType,
		"from", from.Hex(),
		"depositeAddress", depositeAddress,
		"to", tx.To(),
		"toAddress", toAddress,
		"refOrderId", signReq.RefOrderID,
		"userId", signReq.UserID,
		"symbol", signReq.Symbol,
		"projectSymbol", signReq.ProjectSymbol,
		"operateSymbol", signReq.OperateSymbol,
		"operateAmount", operateAmount,
		"tx_value_wei", tx.Value().String(),
		"tx_value_eth", new(big.Float).Quo(new(big.Float).SetInt(tx.Value()), new(big.Float).SetInt64(1e18)).String(),
		"otherInfo", otherInfo)

	c.logger.Debug("Full sign request details",
		"depositAddress_in_struct", signReq.DepositeAddress,
		"toAddress_in_struct", signReq.ToAddress,
		"tx_to_is_nil", tx.To() == nil)

	// 4. Send signing request and wait for result
	// Note: Retry logic is handled by upstream txmgr, no internal retry needed here
	signedTx, err := c.postSignRequestAndWaitResult(ctx, signReq, tx)
	if err != nil {
		c.logger.Error("Remote signing failed",
			"error", err,
			"nonce", tx.Nonce())
		return nil, fmt.Errorf("remote signing failed: %w", err)
	}

	// Log signed transaction details with safe nil handling
	c.logger.Info("Remote signing completed successfully",
		"tx_hash", signedTx.Hash().Hex(),
		"tx_type", signedTx.Type(),
		"nonce", signedTx.Nonce(),
		"to", func() string {
			if signedTx.To() == nil {
				return "<nil>"
			}
			return signedTx.To().Hex()
		}(),
		"value", func() string {
			if signedTx.Value() == nil {
				return "<nil>"
			}
			return signedTx.Value().String()
		}(),
		"gas", signedTx.Gas(),
		"gas_price", func() string {
			if signedTx.GasPrice() == nil {
				return "<nil>"
			}
			return signedTx.GasPrice().String()
		}(),
		"gas_fee_cap", func() string {
			if signedTx.GasFeeCap() == nil {
				return "<nil>"
			}
			return signedTx.GasFeeCap().String()
		}(),
		"gas_tip_cap", func() string {
			if signedTx.GasTipCap() == nil {
				return "<nil>"
			}
			return signedTx.GasTipCap().String()
		}(),
		"data_len", len(signedTx.Data()),
		"chain_id", func() string {
			if signedTx.ChainId() == nil {
				return "<nil>"
			}
			return signedTx.ChainId().String()
		}())

	// Extract and log signature components with nil checks
	v, r, s := signedTx.RawSignatureValues()
	c.logger.Debug("Signed transaction signature details",
		"v", func() string {
			if v == nil {
				return "<nil>"
			}
			return v.String()
		}(),
		"r", func() string {
			if r == nil {
				return "<nil>"
			}
			return r.String()
		}(),
		"s", func() string {
			if s == nil {
				return "<nil>"
			}
			return s.String()
		}())

	// Check if signature is valid
	if v == nil || r == nil || s == nil {
		c.logger.Error("Signed transaction has invalid signature components",
			"v_nil", v == nil,
			"r_nil", r == nil,
			"s_nil", s == nil)
		return nil, fmt.Errorf("signed transaction has invalid signature components: v_nil=%v, r_nil=%v, s_nil=%v",
			v == nil, r == nil, s == nil)
	}

	// 5. Verify signed transaction consistency
	if err := c.verifySignedTransaction(tx, signedTx); err != nil {
		c.logger.Error("Signed transaction verification failed",
			"error", err,
			"original_to", tx.To(),
			"signed_to", signedTx.To(),
			"original_nonce", tx.Nonce(),
			"signed_nonce", signedTx.Nonce(),
			"original_data_len", len(tx.Data()),
			"signed_data_len", len(signedTx.Data()))
		return nil, fmt.Errorf("signed transaction verification failed: %w", err)
	}

	// 6. Re-attach blob sidecar if present
	if sidecar != nil {
		if err := signedTx.SetBlobTxSidecar(sidecar); err != nil {
			return nil, fmt.Errorf("failed to attach sidecar to signed blob tx: %w", err)
		}
	}

	return signedTx, nil
}

func (c *XLayerRemoteClient) detectComponentType(tx *types.Transaction) ComponentRole {
	data := tx.Data()
	dataSize := len(data)

	if tx.To() == nil {
		return ComponentRoleUnknown
	}

	if len(tx.BlobHashes()) > 0 {
		return ComponentRoleBatcher
	}

	if dataSize < 4 {
		return ComponentRoleUnknown
	}

	methodSig := data[:4]

	if componentType := c.detectProposerMethod(methodSig); componentType != ComponentRoleUnknown {
		return componentType
	}

	if componentType := c.detectChallengerMethod(methodSig); componentType != ComponentRoleUnknown {
		return componentType
	}
	return ComponentRoleUnknown
}

func (c *XLayerRemoteClient) detectProposerMethod(methodSig []byte) ComponentRole {
	methodSigHex := hexutil.Encode(methodSig)
	if methodSigHex == MethodSigDGFCreate {
		return ComponentRoleProposer
	}
	return ComponentRoleUnknown
}

func (c *XLayerRemoteClient) detectChallengerMethod(methodSig []byte) ComponentRole {
	methodSigHex := hexutil.Encode(methodSig)
	switch methodSigHex {
	case MethodSigResolveClaim, MethodSigResolve, MethodSigClaimCredit:
		return ComponentRoleChallenger
	}
	return ComponentRoleUnknown
}

// postSignRequestAndWaitResult sends signing request and waits for the result
func (c *XLayerRemoteClient) postSignRequestAndWaitResult(ctx context.Context, req *XLayerSignRequest, originalTx *types.Transaction) (*types.Transaction, error) {
	// 1. Send signing request
	if err := c.postSignRequest(ctx, req); err != nil {
		return nil, fmt.Errorf("failed to post sign request: %w", err)
	}

	// 2. Wait for signing result
	result, err := c.waitSignResult(ctx, req.RefOrderID)
	if err != nil {
		return nil, fmt.Errorf("failed to wait sign result: %w", err)
	}

	// Log raw response from remote signer
	c.logger.Info("Received signing result from remote signer",
		"refOrderId", req.RefOrderID,
		"status", result.Status,
		"success", result.Success,
		"msg", result.Msg,
		"data_length", len(result.Data),
		"data_preview", func() string {
			if len(result.Data) > 100 {
				return result.Data[:100] + "..."
			}
			return result.Data
		}())

	// Log full signed transaction hex for debugging
	c.logger.Debug("Full signed transaction hex from remote signer",
		"full_hex", result.Data)

	// 3. Parse signed transaction
	txData, err := hexutil.Decode(result.Data)
	if err != nil {
		c.logger.Error("Failed to decode signed transaction hex",
			"error", err,
			"data", result.Data)
		return nil, fmt.Errorf("failed to decode signed transaction: %w", err)
	}

	c.logger.Debug("Decoded transaction binary",
		"binary_length", len(txData),
		"binary_hex", hexutil.Encode(txData))

	var signedTx types.Transaction
	if err := signedTx.UnmarshalBinary(txData); err != nil {
		c.logger.Error("Failed to unmarshal signed transaction",
			"error", err,
			"binary_length", len(txData))
		return nil, fmt.Errorf("failed to unmarshal signed transaction: %w", err)
	}

	// Check if signedTx is properly initialized
	if signedTx.Type() == 0 && signedTx.Nonce() == 0 && signedTx.Gas() == 0 {
		c.logger.Error("Signed transaction appears to be uninitialized or invalid",
			"tx_type", signedTx.Type(),
			"nonce", signedTx.Nonce(),
			"gas", signedTx.Gas(),
			"to", signedTx.To(),
			"value", signedTx.Value())
		return nil, fmt.Errorf("signed transaction  is uninitialized or invalid: type=%d, nonce=%d, gas=%d",
			signedTx.Type(), signedTx.Nonce(), signedTx.Gas())
	}

	c.logger.Info("Successfully parsed signed transaction from remote signer",
		"tx_hash", signedTx.Hash().Hex(),
		"tx_type", signedTx.Type(),
		"nonce", signedTx.Nonce(),
		"to", func() string {
			if signedTx.To() == nil {
				return "<nil>"
			}
			return signedTx.To().Hex()
		}(),
		"value", signedTx.Value().String(),
		"gas", signedTx.Gas(),
		"data_len", len(signedTx.Data()))

	// 4. For blob transactions, attach the original sidecar if not present in signed tx.
	// NOTE: Remote signer only signs the transaction core fields without blob data (Sidecar).
	// We must attach the original Sidecar back to the signed transaction, as it's required
	// for broadcasting EIP-4844 blob transactions.
	// This is NOT reassembling transaction fields - we only restore the blob data that was
	// intentionally excluded from the signing request due to its large size (~128KB per blob).
	if originalTx.Type() == types.BlobTxType && signedTx.BlobTxSidecar() == nil {
		c.logger.Info("Attaching sidecar to signed blob transaction")
		if originalTx.BlobTxSidecar() != nil {
			if err := signedTx.SetBlobTxSidecar(originalTx.BlobTxSidecar()); err != nil {
				return nil, fmt.Errorf("failed to attach sidecar: %w", err)
			}
		}
	}

	return &signedTx, nil
}

// postSignRequest sends a signing request to XLayer remote signer
func (c *XLayerRemoteClient) postSignRequest(ctx context.Context, req *XLayerSignRequest) error {
	// 1. Serialize request with sorted keys
	payload, err := c.sortedMarshal(req)
	if err != nil {
		return fmt.Errorf("failed to marshal request: %w", err)
	}

	// Debug: 打印实际发送的JSON
	c.logger.Debug("Serialized sign request JSON",
		"payload", string(payload),
		"depositAddress_field", req.DepositeAddress,
		"toAddress_field", req.ToAddress)

	// 2. Build request URL
	reqURL, err := url.JoinPath(c.endpoint, c.config.RequestSignURI)
	if err != nil {
		return fmt.Errorf("failed to join URL: %w", err)
	}

	// 3. Create HTTP request
	httpReq, err := http.NewRequestWithContext(ctx, "POST", reqURL, bytes.NewBuffer(payload))
	if err != nil {
		return fmt.Errorf("failed to create request: %w", err)
	}

	httpReq.Header.Set("Content-Type", "application/json")

	// 4. Add authentication if configured
	if err := c.addAuth(httpReq); err != nil {
		return fmt.Errorf("failed to add auth: %w", err)
	}

	// Log request details (Debug level only, with sensitive data filtered)
	c.logger.Debug("HTTP Request Details",
		"method", httpReq.Method,
		"url", httpReq.URL.String(),
		"headers", func() map[string]string {
			headers := make(map[string]string)
			// Filter sensitive headers
			sensitiveHeaders := map[string]bool{
				"accesskey":     true,
				"sign":          true,
				"authorization": true,
			}
			for k, v := range httpReq.Header {
				keyLower := strings.ToLower(k)
				if sensitiveHeaders[keyLower] {
					headers[k] = "***REDACTED***"
				} else if len(v) > 0 {
					headers[k] = v[0]
				}
			}
			return headers
		}(),
		"body_length", len(payload))

	// 5. Send request
	resp, err := c.client.Do(httpReq)
	if err != nil {
		return fmt.Errorf("failed to send request: %w", err)
	}
	defer resp.Body.Close()

	// 6. Parse response
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return fmt.Errorf("failed to read response: %w", err)
	}

	// Log response for debugging
	c.logger.Debug("Post sign request response",
		"http_status", resp.StatusCode,
		"content_type", resp.Header.Get("Content-Type"),
		"body_length", len(body),
		"body_preview", func() string {
			if len(body) > 200 {
				return string(body[:200]) + "..."
			}
			return string(body)
		}())

	// Check HTTP status code
	if resp.StatusCode != http.StatusOK {
		c.logger.Error("Post sign request HTTP error",
			"status_code", resp.StatusCode,
			"status", resp.Status,
			"body", string(body))
		return fmt.Errorf("HTTP error: status=%d, body=%s", resp.StatusCode, string(body))
	}

	var signResp XLayerSignResponse
	if err := json.Unmarshal(body, &signResp); err != nil {
		c.logger.Error("Failed to unmarshal sign response",
			"error", err,
			"body", string(body),
			"body_length", len(body))
		return fmt.Errorf("failed to unmarshal response (body: %s): %w", string(body), err)
	}

	if signResp.Status != HTTPStatusSuccess || !signResp.Success {
		c.logger.Error("Sign request failed",
			"response_status", signResp.Status,
			"response_msg", signResp.Msg,
			"response_detail", signResp.DetailMessages,
			"response_code", signResp.Code)
		return fmt.Errorf("sign request failed: status=%d, msg=%s, detail=%s",
			signResp.Status, signResp.Msg, signResp.DetailMessages)
	}

	c.logger.Info("Post sign request successful",
		"order_id", signResp.Data,
		"msg", signResp.Msg)

	return nil
}

// waitSignResult waits for the signing result
func (c *XLayerRemoteClient) waitSignResult(ctx context.Context, orderID string) (*XLayerSignResponse, error) {
	queryReq := &XLayerQueryRequest{
		UserID:        c.config.UserID,
		OrderID:       orderID,
		ProjectSymbol: c.config.ProjectSymbol,
	}

	ticker := time.NewTicker(SignResultPollInterval)
	defer ticker.Stop()

	attempts := 0
	for {
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-ticker.C:
			attempts++
			if attempts > MaxPollAttempts {
				return nil, fmt.Errorf("exceeded maximum poll attempts (%d) waiting for sign result, orderID: %s", MaxPollAttempts, orderID)
			}

			result, err := c.querySignResult(ctx, queryReq)
			if err != nil {
				c.logger.Debug("Query sign result failed, retrying",
					"attempt", attempts,
					"maxAttempts", MaxPollAttempts,
					"orderID", orderID,
					"error", err)
				continue
			}

			if result.Success && len(result.Data) > 0 {
				return result, nil
			}
			// Continue waiting for result
		}
	}
}

// querySignResult queries the signing result from remote signer
func (c *XLayerRemoteClient) querySignResult(ctx context.Context, req *XLayerQueryRequest) (*XLayerSignResponse, error) {
	// Build query URL
	queryURL, err := url.JoinPath(c.endpoint, c.config.QuerySignURI)
	if err != nil {
		return nil, fmt.Errorf("failed to join URL: %w", err)
	}

	params := url.Values{}
	params.Add("orderId", req.OrderID)
	params.Add("projectSymbol", fmt.Sprintf("%d", req.ProjectSymbol))
	fullURL := fmt.Sprintf("%s?%s", queryURL, params.Encode())

	httpReq, err := http.NewRequestWithContext(ctx, "GET", fullURL, nil)
	if err != nil {
		return nil, fmt.Errorf("failed to create request: %w", err)
	}

	if err := c.addAuth(httpReq); err != nil {
		return nil, fmt.Errorf("failed to add auth: %w", err)
	}

	// Log request details (Debug level only, with sensitive data filtered)
	c.logger.Debug("HTTP Query Request Details",
		"method", httpReq.Method,
		"url", httpReq.URL.String(),
		"headers", func() map[string]string {
			headers := make(map[string]string)
			// Filter sensitive headers
			sensitiveHeaders := map[string]bool{
				"accesskey":     true,
				"sign":          true,
				"authorization": true,
			}
			for k, v := range httpReq.Header {
				keyLower := strings.ToLower(k)
				if sensitiveHeaders[keyLower] {
					headers[k] = "***REDACTED***"
				} else if len(v) > 0 {
					headers[k] = v[0]
				}
			}
			return headers
		}())

	resp, err := c.client.Do(httpReq)
	if err != nil {
		return nil, fmt.Errorf("failed to send request: %w", err)
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("failed to read response: %w", err)
	}

	// Log response for debugging
	c.logger.Debug("Query sign result HTTP response",
		"http_status", resp.StatusCode,
		"content_type", resp.Header.Get("Content-Type"),
		"body_length", len(body),
		"body_preview", func() string {
			if len(body) > 200 {
				return string(body[:200]) + "..."
			}
			return string(body)
		}())

	// Check HTTP status code
	if resp.StatusCode != http.StatusOK {
		c.logger.Error("Query sign result HTTP error",
			"status_code", resp.StatusCode,
			"status", resp.Status,
			"body", string(body))
		return nil, fmt.Errorf("HTTP error: status=%d, body=%s", resp.StatusCode, string(body))
	}

	var result XLayerSignResponse
	if err := json.Unmarshal(body, &result); err != nil {
		c.logger.Error("Failed to unmarshal query response",
			"error", err,
			"body", string(body),
			"body_length", len(body))
		return nil, fmt.Errorf("failed to unmarshal response (body: %s): %w", string(body), err)
	}

	c.logger.Debug("Query sign result parsed",
		"status", result.Status,
		"success", result.Success,
		"msg", result.Msg,
		"code", result.Code,
		"data_length", len(result.Data),
		"has_signature", len(result.Data) > 0)

	return &result, nil
}

// sortedMarshal serializes data with sorted keys
func (c *XLayerRemoteClient) sortedMarshal(v interface{}) ([]byte, error) {
	data, err := json.Marshal(v)
	if err != nil {
		return nil, err
	}

	var jsonMap map[string]interface{}
	if err := json.Unmarshal(data, &jsonMap); err != nil {
		return nil, err
	}

	// Sort keys alphabetically
	keys := make([]string, 0, len(jsonMap))
	for k := range jsonMap {
		keys = append(keys, k)
	}
	sort.Strings(keys)

	// Rebuild map with sorted keys
	sortedMap := make(map[string]interface{})
	for _, key := range keys {
		sortedMap[key] = jsonMap[key]
	}

	return json.Marshal(sortedMap)
}

// buildProposerOtherInfo builds Proposer's OtherInfo by unpacking ABI-encoded business parameters
func (c *XLayerRemoteClient) buildProposerOtherInfo(tx *types.Transaction) (string, error) {
	// Base transaction parameters
	baseInfo := c.buildBaseOtherInfo(tx)

	// Try to unpack proposer transaction to get business parameters
	c.logger.Info("Unpacking proposer transaction",
		"txDataLen", len(tx.Data()),
		"txTo", tx.To())

	proposerArgs, err := c.unpackProposerTransaction(tx)
	if err != nil {
		c.logger.Warn("Failed to unpack proposer tx, using base info only",
			"error", err,
			"txHash", tx.Hash(),
			"txDataLen", len(tx.Data()))
		// Return base info on unpacking failure
		return c.marshalOtherInfo(baseInfo)
	}

	c.logger.Info("Successfully unpacked proposer transaction",
		"gameType", proposerArgs.GameType,
		"rootClaim", proposerArgs.RootClaim.Hex(),
		"extraDataLen", len(proposerArgs.ExtraData))

	enhancedInfo := struct {
		XLayerOtherInfo
		GameType  *uint32 `json:"gameType,omitempty"`
		RootClaim *string `json:"rootClaim,omitempty"`
		ExtraData *string `json:"extraData,omitempty"`
	}{
		XLayerOtherInfo: baseInfo,
	}

	// Add DisputeGameFactory.create business parameters
	enhancedInfo.GameType = &proposerArgs.GameType
	rootClaimHex := proposerArgs.RootClaim.Hex()
	enhancedInfo.RootClaim = &rootClaimHex
	if len(proposerArgs.ExtraData) > 0 {
		extraDataHex := hexutil.Encode(proposerArgs.ExtraData)
		enhancedInfo.ExtraData = &extraDataHex
	}

	data, err := json.Marshal(enhancedInfo)
	if err != nil {
		return "", fmt.Errorf("failed to marshal enhanced proposer other info: %w", err)
	}

	return string(data), nil
}

// buildBatcherOtherInfo builds Batcher's OtherInfo
func (c *XLayerRemoteClient) buildBatcherOtherInfo(tx *types.Transaction) (string, error) {
	otherInfo := struct {
		ContractAddress      string  `json:"contractAddress"`
		GasLimit             uint64  `json:"gasLimit"`
		GasPrice             string  `json:"gasPrice"`
		Nonce                uint64  `json:"nonce"`
		MaxFeePerGas         *string `json:"maxFeePerGas,omitempty"`
		MaxPriorityFeePerGas *string `json:"maxPriorityFeePerGas,omitempty"`
		// EIP-4844 specific parameters
		EIP               *int     `json:"eip,omitempty"`
		MaxFeePerBlobGas  *string  `json:"maxFeePerBlobGas,omitempty"`
		BlobVersionHashes []string `json:"blobVersionHashes,omitempty"`
		// Other parameters
		Data  *string `json:"data,omitempty"`
		Value *string `json:"value,omitempty"`
	}{
		ContractAddress: strings.ToLower(tx.To().Hex()),
		GasLimit:        tx.Gas(),
		Nonce:           tx.Nonce(),
	}

	dataHex := hexutil.Encode(tx.Data())
	otherInfo.Data = &dataHex
	valueStr := tx.Value().String()
	otherInfo.Value = &valueStr

	switch tx.Type() {
	case types.BlobTxType:
		eip := 4844
		otherInfo.EIP = &eip

		maxFeePerGasWei := tx.GasFeeCap()
		maxFeePerGasEth := new(big.Float).Quo(new(big.Float).SetInt(maxFeePerGasWei), new(big.Float).SetInt64(1e18))
		maxFeePerGasStr := maxFeePerGasEth.Text('f', 18)
		otherInfo.MaxFeePerGas = &maxFeePerGasStr

		maxPriorityFeePerGasWei := tx.GasTipCap()
		maxPriorityFeePerGasEth := new(big.Float).Quo(new(big.Float).SetInt(maxPriorityFeePerGasWei), new(big.Float).SetInt64(1e18))
		maxPriorityFeePerGasStr := maxPriorityFeePerGasEth.Text('f', 18)
		otherInfo.MaxPriorityFeePerGas = &maxPriorityFeePerGasStr

		blobFeeWei := tx.BlobGasFeeCap()
		blobFeeEth := new(big.Float).Quo(new(big.Float).SetInt(blobFeeWei), new(big.Float).SetInt64(1e18))
		blobFeeStr := blobFeeEth.Text('f', 18)
		otherInfo.MaxFeePerBlobGas = &blobFeeStr

		blobHashes := tx.BlobHashes()
		hashStrings := make([]string, len(blobHashes))
		for i, hash := range blobHashes {
			hashStrings[i] = hash.Hex()
		}
		otherInfo.BlobVersionHashes = hashStrings

		otherInfo.GasPrice = ""

	case types.DynamicFeeTxType:
		maxFeePerGas := tx.GasFeeCap().String()
		otherInfo.MaxFeePerGas = &maxFeePerGas
		maxPriorityFeePerGas := tx.GasTipCap().String()
		otherInfo.MaxPriorityFeePerGas = &maxPriorityFeePerGas
		otherInfo.GasPrice = ""

	default:
		// Legacy transaction
		otherInfo.GasPrice = tx.GasPrice().String()
	}

	data, err := json.Marshal(otherInfo)
	if err != nil {
		return "", fmt.Errorf("failed to marshal batcher other info: %w", err)
	}

	return string(data), nil
}

// buildChallengerOtherInfo builds Challenger's OtherInfo based on method signature
func (c *XLayerRemoteClient) buildChallengerOtherInfo(tx *types.Transaction) (string, error) {
	baseInfo := c.buildBaseOtherInfo(tx)

	// Cannot parse method signature if data is less than 4 bytes
	if len(tx.Data()) < 4 {
		return c.marshalOtherInfo(baseInfo)
	}

	// Extract method signature (first 4 bytes)
	methodSig := tx.Data()[:4]
	methodSigHex := hexutil.Encode(methodSig)

	// Determine method type by signature and route to appropriate handler
	// We use hardcoded method signatures instead of ABI parsing to avoid dependencies
	switch methodSigHex {
	case MethodSigResolveClaim: // resolveClaim
		return c.buildChallengerResolveClaimOtherInfo(tx, baseInfo)
	case MethodSigResolve: // resolve
		return c.buildChallengerResolveOtherInfo(tx, baseInfo)
	case MethodSigClaimCredit: // claimCredit
		return c.buildChallengerClaimCreditOtherInfo(tx, baseInfo)
	default:
		// Unknown method, return base info with method signature
		c.logger.Warn("Unknown challenger method signature", "signature", methodSigHex)
		enhancedInfo := struct {
			XLayerOtherInfo
			MethodSignature string `json:"methodSignature,omitempty"`
		}{
			XLayerOtherInfo: baseInfo,
			MethodSignature: methodSigHex,
		}
		return c.marshalOtherInfo(enhancedInfo)
	}
}

// buildChallengerResolveClaimOtherInfo builds OtherInfo for resolveClaim method
func (c *XLayerRemoteClient) buildChallengerResolveClaimOtherInfo(tx *types.Transaction, baseInfo XLayerOtherInfo) (string, error) {
	// resolveClaim(uint256 _claimIndex, uint256 _numToResolve)
	// Parse two uint256 parameters
	var claimIndex *uint64
	var numToResolve *uint64

	if len(tx.Data()) >= 36 { // 4 bytes signature + 32 bytes uint256
		claimIndexBig := new(big.Int).SetBytes(tx.Data()[4:36])
		claimIndexVal := claimIndexBig.Uint64()
		claimIndex = &claimIndexVal
	}

	if len(tx.Data()) >= 68 { // 4 bytes signature + 32 bytes + 32 bytes
		numToResolveBig := new(big.Int).SetBytes(tx.Data()[36:68])
		numToResolveVal := numToResolveBig.Uint64()
		numToResolve = &numToResolveVal
	}

	enhancedInfo := struct {
		XLayerOtherInfo
		Method       string  `json:"method"`
		ClaimIndex   *uint64 `json:"claimIndex,omitempty"`
		NumToResolve *uint64 `json:"numToResolve,omitempty"`
	}{
		XLayerOtherInfo: baseInfo,
		Method:          "resolveClaim",
		ClaimIndex:      claimIndex,
		NumToResolve:    numToResolve,
	}

	return c.marshalOtherInfo(enhancedInfo)
}

// buildChallengerResolveOtherInfo builds OtherInfo for resolve method
func (c *XLayerRemoteClient) buildChallengerResolveOtherInfo(tx *types.Transaction, baseInfo XLayerOtherInfo) (string, error) {
	// resolve() - no parameters
	enhancedInfo := struct {
		XLayerOtherInfo
		Method string `json:"method"`
	}{
		XLayerOtherInfo: baseInfo,
		Method:          "resolve",
	}

	return c.marshalOtherInfo(enhancedInfo)
}

// buildChallengerClaimCreditOtherInfo builds OtherInfo for claimCredit method
func (c *XLayerRemoteClient) buildChallengerClaimCreditOtherInfo(tx *types.Transaction, baseInfo XLayerOtherInfo) (string, error) {
	// claimCredit(address _recipient)
	// Parse recipient parameter
	var recipient *string
	if len(tx.Data()) >= 36 { // 4 bytes signature + 32 bytes address
		recipientAddr := common.BytesToAddress(tx.Data()[16:36]) // Address in last 20 bytes
		recipientHex := recipientAddr.Hex()
		recipient = &recipientHex
	}

	enhancedInfo := struct {
		XLayerOtherInfo
		Method    string  `json:"method"`
		Recipient *string `json:"recipient,omitempty"`
	}{
		XLayerOtherInfo: baseInfo,
		Method:          "claimCredit",
		Recipient:       recipient,
	}

	return c.marshalOtherInfo(enhancedInfo)
}

// buildBaseOtherInfo builds base OtherInfo parameters
func (c *XLayerRemoteClient) buildBaseOtherInfo(tx *types.Transaction) XLayerOtherInfo {
	otherInfo := XLayerOtherInfo{
		ContractAddress: strings.ToLower(tx.To().Hex()),
		GasLimit:        tx.Gas(),
		Nonce:           tx.Nonce(),
		TxData:          hexutil.Encode(tx.Data()),
		Value:           tx.Value().String(),
	}

	// Set gas parameters based on transaction type
	switch tx.Type() {
	case types.BlobTxType:
		otherInfo.MaxFeePerGas = tx.GasFeeCap().String()
		otherInfo.MaxPriorityFeePerGas = tx.GasTipCap().String()
		otherInfo.BlobFeeCap = tx.BlobGasFeeCap().String()
		otherInfo.BlobVersionedHashes = tx.BlobHashes()
	case types.DynamicFeeTxType:
		otherInfo.MaxFeePerGas = tx.GasFeeCap().String()
		otherInfo.MaxPriorityFeePerGas = tx.GasTipCap().String()
	default:
		otherInfo.GasPrice = tx.GasPrice().String()
	}

	return otherInfo
}

// marshalOtherInfo serializes OtherInfo to JSON string
func (c *XLayerRemoteClient) marshalOtherInfo(info interface{}) (string, error) {
	data, err := json.Marshal(info)
	if err != nil {
		return "", fmt.Errorf("failed to marshal other info: %w", err)
	}
	return string(data), nil
}

// getBatcherOperateType returns the operate type for Batcher transactions
func (c *XLayerRemoteClient) getBatcherOperateType(tx *types.Transaction) OperateType {
	switch tx.Type() {
	case types.BlobTxType:
		return OperateTypeBatcherBlob
	default:
		return OperateTypeBatcherLegacy
	}
}

// getChallengerOperateType returns the operate type for Challenger transactions based on method signature
func (c *XLayerRemoteClient) getChallengerOperateType(tx *types.Transaction) OperateType {
	methodSigHex := hexutil.Encode(tx.Data()[:4])
	switch methodSigHex {
	case MethodSigResolveClaim:
		return OperateTypeChallengerResolveClaim
	case MethodSigResolve:
		return OperateTypeChallengerResolve
	case MethodSigClaimCredit:
		return OperateTypeChallengerClaimCredit
	default:
		c.logger.Warn("Unknown challenger method, using default operateType", "signature", methodSigHex)
		return OperateTypeChallengerResolveClaim
	}
}

type ProposerTxArgs struct {
	// DisputeGameFactory.create parameters
	GameType  uint32      `json:"gameType"`
	RootClaim common.Hash `json:"rootClaim"`
	ExtraData []byte      `json:"extraData"`
}

func (c *XLayerRemoteClient) unpackProposerTransaction(tx *types.Transaction) (*ProposerTxArgs, error) {
	if tx == nil || len(tx.Data()) < 4 {
		return nil, fmt.Errorf("empty or invalid transaction")
	}

	data := tx.Data()
	methodSig := data[:4]
	methodData := data[4:]

	// Check if this is DisputeGameFactory.create method
	methodSigHex := hexutil.Encode(methodSig)
	if methodSigHex != MethodSigDGFCreate {
		return nil, fmt.Errorf("unknown proposer transaction method signature: %s (only DisputeGameFactory.create supported)", methodSigHex)
	}

	// Manually parse ABI-encoded parameters for create(uint32 _gameType, bytes32 _rootClaim, bytes calldata _extraData)
	if len(methodData) < 96 {
		return nil, fmt.Errorf("method data too short: %d bytes", len(methodData))
	}

	// Parse uint32 _gameType (first 32 bytes, right-aligned)
	gameTypeBytes := methodData[28:32]
	gameType := uint32(gameTypeBytes[0])<<24 | uint32(gameTypeBytes[1])<<16 |
		uint32(gameTypeBytes[2])<<8 | uint32(gameTypeBytes[3])

	// Parse bytes32 _rootClaim (next 32 bytes)
	var rootClaim common.Hash
	copy(rootClaim[:], methodData[32:64])

	// Parse bytes calldata _extraData (dynamic type)
	extraDataOffset := new(big.Int).SetBytes(methodData[64:96]).Uint64()
	if extraDataOffset+32 > uint64(len(methodData)) {
		return nil, fmt.Errorf("invalid extraData offset: %d", extraDataOffset)
	}

	extraDataLength := new(big.Int).SetBytes(methodData[extraDataOffset : extraDataOffset+32]).Uint64()
	if extraDataOffset+32+extraDataLength > uint64(len(methodData)) {
		return nil, fmt.Errorf("invalid extraData length: %d", extraDataLength)
	}

	extraData := make([]byte, extraDataLength)
	copy(extraData, methodData[extraDataOffset+32:extraDataOffset+32+extraDataLength])

	c.logger.Debug("Successfully unpacked DisputeGameFactory.create",
		"gameType", gameType,
		"rootClaim", rootClaim.Hex(),
		"extraData", hexutil.Encode(extraData))

	return &ProposerTxArgs{
		GameType:  gameType,
		RootClaim: rootClaim,
		ExtraData: extraData,
	}, nil
}

func (c *XLayerRemoteClient) verifySignedTransaction(originalTx *types.Transaction, signedTx *types.Transaction) error {
	if originalTx.To() != nil && signedTx.To() != nil {
		if *originalTx.To() != *signedTx.To() {
			return fmt.Errorf("to address mismatch: original=%s, signed=%s",
				originalTx.To().Hex(), signedTx.To().Hex())
		}
	} else if originalTx.To() != signedTx.To() {
		return fmt.Errorf("to address nil mismatch: original=%v, signed=%v",
			originalTx.To(), signedTx.To())
	}

	if !bytes.Equal(originalTx.Data(), signedTx.Data()) {
		return fmt.Errorf("transaction data mismatch: original_len=%d, signed_len=%d",
			len(originalTx.Data()), len(signedTx.Data()))
	}

	if originalTx.Value().Cmp(signedTx.Value()) != 0 {
		return fmt.Errorf("transaction value mismatch: original=%s, signed=%s",
			originalTx.Value().String(), signedTx.Value().String())
	}

	// 4. Verification gas limit
	if originalTx.Gas() != signedTx.Gas() {
		return fmt.Errorf("gas limit mismatch: original=%d, signed=%d",
			originalTx.Gas(), signedTx.Gas())
	}

	// 5. nonce
	if originalTx.Nonce() != signedTx.Nonce() {
		return fmt.Errorf("nonce mismatch: original=%d, signed=%d",
			originalTx.Nonce(), signedTx.Nonce())
	}

	// 6. Verification ID
	if originalTx.ChainId().Cmp(signedTx.ChainId()) != 0 {
		return fmt.Errorf("chain ID mismatch: original=%s, signed=%s",
			originalTx.ChainId().String(), signedTx.ChainId().String())
	}

	// 7. Verify the transaction type (allowing EIP-1559 conversion to Legacy)
	if originalTx.Type() != signedTx.Type() {
		// Allow EIP-1559 (type 2) to be converted to Legacy (type 0), but record a warning
		if originalTx.Type() == types.DynamicFeeTxType && signedTx.Type() == types.LegacyTxType {
			c.logger.Warn("Transaction type converted by remote signer",
				"original_type", originalTx.Type(),
				"signed_type", signedTx.Type(),
				"reason", "EIP-1559 converted to Legacy")
		} else {
			return fmt.Errorf("unexpected transaction type conversion: original=%d, signed=%d",
				originalTx.Type(), signedTx.Type())
		}
	}

	// 8. Verify the fee parameters (perform intelligent verification based on the transaction type after signing)
	if err := c.verifyGasFields(originalTx, signedTx); err != nil {
		return fmt.Errorf("gas fields verification failed: %w", err)
	}

	// 9.Verify whether the signature is valid
	if err := c.verifyTransactionSignature(signedTx); err != nil {
		return fmt.Errorf("transaction signature verification failed: %w", err)
	}

	c.logger.Info("Signed transaction verification passed",
		"original_hash", originalTx.Hash().Hex(),
		"signed_hash", signedTx.Hash().Hex(),
		"type", signedTx.Type(),
		"to", signedTx.To(),
		"nonce", signedTx.Nonce())

	return nil
}

func (c *XLayerRemoteClient) verifyBlobTxFields(originalTx *types.Transaction, signedTx *types.Transaction) error {
	// gas fee cap
	if originalTx.GasFeeCap().Cmp(signedTx.GasFeeCap()) != 0 {
		return fmt.Errorf("gas fee cap mismatch: original=%s, signed=%s",
			originalTx.GasFeeCap().String(), signedTx.GasFeeCap().String())
	}

	// gas tip cap
	if originalTx.GasTipCap().Cmp(signedTx.GasTipCap()) != 0 {
		return fmt.Errorf("gas tip cap mismatch: original=%s, signed=%s",
			originalTx.GasTipCap().String(), signedTx.GasTipCap().String())
	}

	// blob gas fee cap
	if originalTx.BlobGasFeeCap().Cmp(signedTx.BlobGasFeeCap()) != 0 {
		return fmt.Errorf("blob gas fee cap mismatch: original=%s, signed=%s",
			originalTx.BlobGasFeeCap().String(), signedTx.BlobGasFeeCap().String())
	}

	// blob hash
	originalHashes := originalTx.BlobHashes()
	signedHashes := signedTx.BlobHashes()
	if len(originalHashes) != len(signedHashes) {
		return fmt.Errorf("blob hashes count mismatch: original=%d, signed=%d",
			len(originalHashes), len(signedHashes))
	}

	for i, originalHash := range originalHashes {
		if originalHash != signedHashes[i] {
			return fmt.Errorf("blob hash mismatch at index %d: original=%s, signed=%s",
				i, originalHash.Hex(), signedHashes[i].Hex())
		}
	}

	return nil
}

// verifyDynamicFeeTxFields verify EIP-1559
func (c *XLayerRemoteClient) verifyDynamicFeeTxFields(originalTx *types.Transaction, signedTx *types.Transaction) error {
	// 验证gas fee cap
	if originalTx.GasFeeCap().Cmp(signedTx.GasFeeCap()) != 0 {
		return fmt.Errorf("gas fee cap mismatch: original=%s, signed=%s",
			originalTx.GasFeeCap().String(), signedTx.GasFeeCap().String())
	}

	// 验证gas tip cap
	if originalTx.GasTipCap().Cmp(signedTx.GasTipCap()) != 0 {
		return fmt.Errorf("gas tip cap mismatch: original=%s, signed=%s",
			originalTx.GasTipCap().String(), signedTx.GasTipCap().String())
	}

	return nil
}

func (c *XLayerRemoteClient) verifyTransactionSignature(signedTx *types.Transaction) error {
	v, r, s := signedTx.RawSignatureValues()
	if v == nil || r == nil || s == nil {
		return fmt.Errorf("transaction is not signed: v=%v, r=%v, s=%v", v, r, s)
	}

	if v.Sign() == 0 && r.Sign() == 0 && s.Sign() == 0 {
		return fmt.Errorf("transaction has zero signature values")
	}

	signer := types.LatestSignerForChainID(signedTx.ChainId())
	recoveredFrom, err := signer.Sender(signedTx)
	if err != nil {
		return fmt.Errorf("failed to recover sender from signature: %w", err)
	}

	expectedFrom := common.HexToAddress(c.config.Address)
	if recoveredFrom != expectedFrom {
		return fmt.Errorf("signature verification failed: expected signer=%s, recovered=%s",
			expectedFrom.Hex(), recoveredFrom.Hex())
	}

	c.logger.Debug("Transaction signature verification passed",
		"signer", recoveredFrom.Hex(),
		"tx_hash", signedTx.Hash().Hex())

	return nil
}

// verifyGasFields Intelligently verify the gas field and handle transaction type conversions
func (c *XLayerRemoteClient) verifyGasFields(originalTx *types.Transaction, signedTx *types.Transaction) error {
	// Verify based on the transaction type after signing
	switch signedTx.Type() {
	case types.BlobTxType:
		// After signing, it becomes a blob transaction, and the original transaction must also be a blob transaction
		if originalTx.Type() != types.BlobTxType {
			return fmt.Errorf("blob transaction type mismatch: original=%d, signed=%d",
				originalTx.Type(), signedTx.Type())
		}
		return c.verifyBlobTxFields(originalTx, signedTx)

	case types.DynamicFeeTxType:
		// After signing, it becomes an EIP-1559 transaction
		if originalTx.Type() != types.DynamicFeeTxType {
			return fmt.Errorf("dynamic fee transaction type mismatch: original=%d, signed=%d",
				originalTx.Type(), signedTx.Type())
		}
		return c.verifyDynamicFeeTxFields(originalTx, signedTx)

	case types.LegacyTxType:
		// After signing, it becomes a Legacy transaction, which may be converted from EIP-1559
		return c.verifyLegacyTxFields(originalTx, signedTx)

	default:
		return fmt.Errorf("unsupported signed transaction type: %d", signedTx.Type())
	}
}

// verifyLegacyTxFields Verify Legacy transaction fields (handle conversions from EIP-1559)
func (c *XLayerRemoteClient) verifyLegacyTxFields(originalTx *types.Transaction, signedTx *types.Transaction) error {
	switch originalTx.Type() {
	case types.LegacyTxType:
		// The original transaction was Legacy, directly comparing the gas price
		if originalTx.GasPrice().Cmp(signedTx.GasPrice()) != 0 {
			return fmt.Errorf("gas price mismatch: original=%s, signed=%s",
				originalTx.GasPrice().String(), signedTx.GasPrice().String())
		}

	case types.DynamicFeeTxType:
		// The original is EIP-1559. To convert it to Legacy, it is necessary to verify whether the gas price is reasonable
		// The gas price of Legacy should be equal to or close to the original gas fee cap
		originalGasFeeCap := originalTx.GasFeeCap()
		signedGasPrice := signedTx.GasPrice()

		// A certain margin of error (such as ±20%) is allowed as the remote signer may adjust
		tolerance := new(big.Int).Div(originalGasFeeCap, big.NewInt(5)) // 20% tolerance
		lowerBound := new(big.Int).Sub(originalGasFeeCap, tolerance)
		upperBound := new(big.Int).Add(originalGasFeeCap, tolerance)

		if signedGasPrice.Cmp(lowerBound) < 0 || signedGasPrice.Cmp(upperBound) > 0 {
			c.logger.Warn("Gas price conversion outside tolerance range",
				"original_gas_fee_cap", originalGasFeeCap.String(),
				"original_gas_tip_cap", originalTx.GasTipCap().String(),
				"signed_gas_price", signedGasPrice.String(),
				"tolerance", tolerance.String(),
				"lower_bound", lowerBound.String(),
				"upper_bound", upperBound.String())
			// No error is returned; only warnings are recorded because this conversion is allowed
		}

		c.logger.Debug("EIP-1559 to Legacy gas conversion verified",
			"original_gas_fee_cap", originalGasFeeCap.String(),
			"original_gas_tip_cap", originalTx.GasTipCap().String(),
			"signed_gas_price", signedGasPrice.String())

	default:
		return fmt.Errorf("unsupported original transaction type for legacy conversion: %d", originalTx.Type())
	}

	return nil
}

func convertValueToOperateAmount(valueWei *big.Int) string {
	if valueWei == nil || valueWei.Sign() == 0 {
		return "0"
	}

	oneEth := new(big.Float).SetInt(big.NewInt(1000000000000000000)) // 10^18
	valueFloat := new(big.Float).SetInt(valueWei)
	ethAmount := new(big.Float).Quo(valueFloat, oneEth)

	ethStr := ethAmount.Text('f', 18)

	ethStr = strings.TrimRight(ethStr, "0")
	ethStr = strings.TrimRight(ethStr, ".")

	return ethStr
}

func (c *XLayerRemoteClient) Close() {
	if c.client != nil {
		c.client.CloseIdleConnections()
	}
}
