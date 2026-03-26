package tee

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/log"
)

const (
	taskBasePath = "/tee/task/"
)

// Task statuses returned by the TEE Prover.
const (
	TaskStatusRunning  = "Running"
	TaskStatusFinished = "Finished"
	TaskStatusFailed   = "Failed"
)

// TEE Prover error codes.
const (
	codeOK             = 0
	codeInvalidSig     = 10000 // Invalid signature — retryable
	codeInvalidParams  = 10001 // Invalid parameters — NOT retryable
	codeTaskNotFound   = 10004 // Task not found — triggers re-POST
	codeInternalError  = 20001 // Internal error — retryable
)

// errNonRetryable is returned when the TEE Prover returns an error that should not be retried.
var errNonRetryable = errors.New("non-retryable prover error")

// ProveRequest is sent to the TEE Prover to initiate a proof task.
type ProveRequest struct {
	StartBlkHeight    uint64      `json:"startBlkHeight"`
	EndBlkHeight      uint64      `json:"endBlkHeight"`
	StartBlkHash      common.Hash `json:"startBlkHash"`
	EndBlkHash        common.Hash `json:"endBlkHash"`
	StartBlkStateHash common.Hash `json:"startBlkStateHash"`
	EndBlkStateHash   common.Hash `json:"endBlkStateHash"`
}

// ProverResponse is the generic response envelope from the TEE Prover.
type ProverResponse struct {
	Code    int             `json:"code"`
	Message string          `json:"message"`
	Data    json.RawMessage `json:"data"`
}

// CreateTaskData is the data returned from POST /tee/task/.
type CreateTaskData struct {
	TaskID string `json:"taskId"`
}

// TaskResultData is the data returned from GET /tee/task/{taskId}.
type TaskResultData struct {
	Status     string `json:"status"`
	ProofBytes string `json:"proofBytes"`
	Detail     any    `json:"detail"`
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

	url := c.baseURL + taskBasePath
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

	var proveResp ProverResponse
	if err := json.Unmarshal(respBody, &proveResp); err != nil {
		return "", fmt.Errorf("failed to unmarshal prove response: %w", err)
	}

	if proveResp.Code != codeOK {
		err := fmt.Errorf("prove request returned error code %d: %s", proveResp.Code, proveResp.Message)
		if proveResp.Code == codeInvalidParams {
			return "", fmt.Errorf("%w: %w", errNonRetryable, err)
		}
		return "", err
	}

	var data CreateTaskData
	if err := json.Unmarshal(proveResp.Data, &data); err != nil {
		return "", fmt.Errorf("failed to unmarshal prove response data: %w", err)
	}

	return data.TaskID, nil
}

// GetTaskResult queries the status of a prove task.
func (c *ProverClient) GetTaskResult(ctx context.Context, taskID string) (*TaskResultData, error) {
	url := c.baseURL + taskBasePath + taskID
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

	var envelope ProverResponse
	if err := json.Unmarshal(respBody, &envelope); err != nil {
		return nil, fmt.Errorf("failed to unmarshal task response: %w", err)
	}

	if envelope.Code == codeTaskNotFound {
		return nil, fmt.Errorf("task %s not found (code %d)", taskID, codeTaskNotFound)
	}
	if envelope.Code != codeOK {
		return nil, fmt.Errorf("task request returned error code %d: %s", envelope.Code, envelope.Message)
	}

	var data TaskResultData
	if err := json.Unmarshal(envelope.Data, &data); err != nil {
		return nil, fmt.Errorf("failed to unmarshal task result data: %w", err)
	}

	return &data, nil
}

// ProveAndWait submits a proof request and retries until it succeeds or the context is cancelled.
// On task failure (Failed status), it re-submits a new task. On non-retryable errors (code=10001),
// it returns immediately. The ctx should have a timeout set by the caller to bound total prove time.
func (c *ProverClient) ProveAndWait(ctx context.Context, req ProveRequest) ([]byte, error) {
	for {
		// 1. Submit task
		taskID, err := c.Prove(ctx, req)
		if err != nil {
			if errors.Is(err, errNonRetryable) {
				return nil, err
			}
			// Retryable error (10000, 20001, HTTP 5xx) — wait and retry
			c.logger.Warn("Prove request failed, will retry", "err", err)
			select {
			case <-ctx.Done():
				return nil, ctx.Err()
			case <-time.After(c.pollInterval):
				continue
			}
		}

		c.logger.Info("TEE prove task submitted", "taskID", taskID)

		// 2. Poll task status
		proofBytes, err := c.pollTask(ctx, taskID)
		if err == nil {
			return proofBytes, nil
		}

		// 3. Non-retryable → return immediately
		if errors.Is(err, errNonRetryable) {
			return nil, err
		}

		// 4. Retryable (Failed / task not found / HTTP error) → wait and re-POST
		c.logger.Warn("Task failed, will re-submit", "taskID", taskID, "err", err)
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-time.After(c.pollInterval):
			continue
		}
	}
}

// pollTask polls a task until it finishes, fails, or the context is cancelled.
func (c *ProverClient) pollTask(ctx context.Context, taskID string) ([]byte, error) {
	ticker := time.NewTicker(c.pollInterval)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-ticker.C:
			result, err := c.GetTaskResult(ctx, taskID)
			if err != nil {
				// HTTP error or task not found → return to outer retry loop
				return nil, err
			}

			switch result.Status {
			case TaskStatusFinished:
				c.logger.Info("TEE prove task finished", "taskID", taskID)
				return decodeProofBytes(result.ProofBytes)
			case TaskStatusFailed:
				return nil, fmt.Errorf("task %s failed: %v", taskID, result.Detail)
			case TaskStatusRunning:
				c.logger.Debug("TEE prove task still running", "taskID", taskID)
			default:
				c.logger.Warn("Unknown TEE prove task status", "taskID", taskID, "status", result.Status)
			}
		}
	}
}
