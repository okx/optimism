package tee

import (
	"bytes"
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/log"
)

const (
	defaultProvePath = "/prove"
	defaultTaskPath  = "/task/%s"
)

// Task statuses returned by the TEE Prover.
const (
	TaskStatusFinished = "FINISHED"
	TaskStatusPending  = "PENDING"
	TaskStatusNotFound = "NOT_FOUND"
)

// ProveRequest is sent to the TEE Prover to initiate a proof task.
type ProveRequest struct {
	PreAppHash       common.Hash `json:"preAppHash"`
	PostAppHash      common.Hash `json:"postAppHash"`
	StartBlockHeight uint64      `json:"startBlockHeight"`
	EndBlockHeight   uint64      `json:"endBlockHeight"`
	StartBlockHash   common.Hash `json:"startBlockHash"`
	EndBlockHash     common.Hash `json:"endBlockHash"`
}

// ProveResponse is the response from the TEE Prover after submitting a prove task.
type ProveResponse struct {
	Code    string `json:"code"`
	Message string `json:"message"`
	Data    struct {
		TaskID string `json:"taskId"`
	} `json:"data"`
}

// TaskResult is the response from polling a prove task's status.
type TaskResult struct {
	Code    string `json:"code"`
	Message string `json:"message"`
	Data    struct {
		Status     string `json:"status"`
		ProofBytes string `json:"proofBytes"`
	} `json:"data"`
}

// ProverClient communicates with the TEE Prover HTTP service.
type ProverClient struct {
	httpClient   *http.Client
	baseURL      string
	pollInterval time.Duration
	logger       log.Logger
}

// NewProverClient creates a new TEE Prover HTTP client.
func NewProverClient(baseURL string, pollInterval time.Duration, logger log.Logger) *ProverClient {
	return &ProverClient{
		httpClient:   &http.Client{Timeout: 30 * time.Second},
		baseURL:      strings.TrimRight(baseURL, "/"),
		pollInterval: pollInterval,
		logger:       logger,
	}
}

// Prove submits a proof request and returns the task ID.
func (c *ProverClient) Prove(ctx context.Context, req ProveRequest) (string, error) {
	body, err := json.Marshal(req)
	if err != nil {
		return "", fmt.Errorf("failed to marshal prove request: %w", err)
	}

	url := c.baseURL + defaultProvePath
	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(body))
	if err != nil {
		return "", fmt.Errorf("failed to create prove request: %w", err)
	}
	httpReq.Header.Set("Content-Type", "application/json")

	resp, err := c.httpClient.Do(httpReq)
	if err != nil {
		return "", fmt.Errorf("failed to send prove request: %w", err)
	}
	defer resp.Body.Close()

	respBody, err := io.ReadAll(resp.Body)
	if err != nil {
		return "", fmt.Errorf("failed to read prove response: %w", err)
	}

	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("prove request failed with status %d: %s", resp.StatusCode, string(respBody))
	}

	var proveResp ProveResponse
	if err := json.Unmarshal(respBody, &proveResp); err != nil {
		return "", fmt.Errorf("failed to unmarshal prove response: %w", err)
	}

	return proveResp.Data.TaskID, nil
}

// GetTaskResult polls the status of a prove task.
func (c *ProverClient) GetTaskResult(ctx context.Context, taskID string) (*TaskResult, error) {
	url := c.baseURL + fmt.Sprintf(defaultTaskPath, taskID)
	httpReq, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, fmt.Errorf("failed to create task request: %w", err)
	}

	resp, err := c.httpClient.Do(httpReq)
	if err != nil {
		return nil, fmt.Errorf("failed to send task request: %w", err)
	}
	defer resp.Body.Close()

	respBody, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("failed to read task response: %w", err)
	}

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("task request failed with status %d: %s", resp.StatusCode, string(respBody))
	}

	var result TaskResult
	if err := json.Unmarshal(respBody, &result); err != nil {
		return nil, fmt.Errorf("failed to unmarshal task response: %w", err)
	}

	return &result, nil
}

// ProveAndWait submits a proof request and polls until it is finished or the context is cancelled.
func (c *ProverClient) ProveAndWait(ctx context.Context, req ProveRequest) ([]byte, error) {
	taskID, err := c.Prove(ctx, req)
	if err != nil {
		return nil, fmt.Errorf("failed to submit prove task: %w", err)
	}

	c.logger.Info("TEE prove task submitted", "taskID", taskID)

	ticker := time.NewTicker(c.pollInterval)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-ticker.C:
			result, err := c.GetTaskResult(ctx, taskID)
			if err != nil {
				c.logger.Warn("Failed to poll TEE prove task", "taskID", taskID, "err", err)
				continue
			}

			switch result.Data.Status {
			case TaskStatusFinished:
				c.logger.Info("TEE prove task finished", "taskID", taskID)
				proofBytes, err := hex.DecodeString(strings.TrimPrefix(result.Data.ProofBytes, "0x"))
				if err != nil {
					return nil, fmt.Errorf("failed to decode proof bytes: %w", err)
				}
				return proofBytes, nil
			case TaskStatusPending:
				c.logger.Debug("TEE prove task still pending", "taskID", taskID)
			case TaskStatusNotFound:
				return nil, fmt.Errorf("TEE prove task not found: %s", taskID)
			default:
				c.logger.Warn("Unknown TEE prove task status", "taskID", taskID, "status", result.Data.Status)
			}
		}
	}
}
