package tee

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/log"

	"github.com/ethereum-optimism/optimism/op-service/testlog"
	"github.com/stretchr/testify/require"
)

func TestProveSuccess(t *testing.T) {
	expectedTaskID := "task-abc-123"
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		require.Equal(t, http.MethodPost, r.Method)
		require.Equal(t, "/prove", r.URL.Path)
		require.Equal(t, "application/json", r.Header.Get("Content-Type"))

		var req ProveRequest
		err := json.NewDecoder(r.Body).Decode(&req)
		require.NoError(t, err)
		require.Equal(t, common.Hash{0x01}, req.PreAppHash)
		require.Equal(t, common.Hash{0x02}, req.PostAppHash)
		require.Equal(t, uint64(100), req.StartBlockHeight)
		require.Equal(t, uint64(200), req.EndBlockHeight)
		require.Equal(t, common.Hash{0x03}, req.StartBlockHash)
		require.Equal(t, common.Hash{0x04}, req.EndBlockHash)

		resp := ProveResponse{Code: "0", Message: "ok"}
		resp.Data.TaskID = expectedTaskID
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	client := NewProverClient(server.URL, time.Second, testlog.Logger(t, log.LvlInfo))
	taskID, err := client.Prove(context.Background(), ProveRequest{
		PreAppHash:       common.Hash{0x01},
		PostAppHash:      common.Hash{0x02},
		StartBlockHeight: 100,
		EndBlockHeight:   200,
		StartBlockHash:   common.Hash{0x03},
		EndBlockHash:     common.Hash{0x04},
	})
	require.NoError(t, err)
	require.Equal(t, expectedTaskID, taskID)
}

func TestProveServerError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
		w.Write([]byte("internal error"))
	}))
	defer server.Close()

	client := NewProverClient(server.URL, time.Second, testlog.Logger(t, log.LvlInfo))
	_, err := client.Prove(context.Background(), ProveRequest{})
	require.Error(t, err)
	require.Contains(t, err.Error(), "500")
}

func TestProveBadResponse(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Write([]byte("not json"))
	}))
	defer server.Close()

	client := NewProverClient(server.URL, time.Second, testlog.Logger(t, log.LvlInfo))
	_, err := client.Prove(context.Background(), ProveRequest{})
	require.Error(t, err)
	require.Contains(t, err.Error(), "unmarshal")
}

func TestGetTaskFinished(t *testing.T) {
	proofHex := "0xdeadbeef"
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		require.Equal(t, http.MethodGet, r.Method)
		require.Equal(t, "/task/task-123", r.URL.Path)
		resp := TaskResult{Code: "0", Message: "ok"}
		resp.Data.Status = TaskStatusFinished
		resp.Data.ProofBytes = proofHex
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	client := NewProverClient(server.URL, time.Second, testlog.Logger(t, log.LvlInfo))
	result, err := client.GetTaskResult(context.Background(), "task-123")
	require.NoError(t, err)
	require.Equal(t, TaskStatusFinished, result.Data.Status)
	require.Equal(t, proofHex, result.Data.ProofBytes)
}

func TestGetTaskPending(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		resp := TaskResult{Code: "0", Message: "ok"}
		resp.Data.Status = TaskStatusPending
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	client := NewProverClient(server.URL, time.Second, testlog.Logger(t, log.LvlInfo))
	result, err := client.GetTaskResult(context.Background(), "task-456")
	require.NoError(t, err)
	require.Equal(t, TaskStatusPending, result.Data.Status)
	require.Empty(t, result.Data.ProofBytes)
}

func TestGetTaskNotFound(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		resp := TaskResult{Code: "0", Message: "ok"}
		resp.Data.Status = TaskStatusNotFound
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	client := NewProverClient(server.URL, time.Second, testlog.Logger(t, log.LvlInfo))
	result, err := client.GetTaskResult(context.Background(), "task-789")
	require.NoError(t, err)
	require.Equal(t, TaskStatusNotFound, result.Data.Status)
}

func TestGetTaskServerError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusBadGateway)
		w.Write([]byte("bad gateway"))
	}))
	defer server.Close()

	client := NewProverClient(server.URL, time.Second, testlog.Logger(t, log.LvlInfo))
	_, err := client.GetTaskResult(context.Background(), "task-000")
	require.Error(t, err)
	require.Contains(t, err.Error(), "502")
}

func TestProveAndWaitSuccess(t *testing.T) {
	callCount := 0
	expectedProof := []byte{0xde, 0xad, 0xbe, 0xef}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.Method {
		case http.MethodPost:
			resp := ProveResponse{Code: "0", Message: "ok"}
			resp.Data.TaskID = "task-wait"
			json.NewEncoder(w).Encode(resp)
		case http.MethodGet:
			callCount++
			resp := TaskResult{Code: "0", Message: "ok"}
			if callCount >= 2 {
				resp.Data.Status = TaskStatusFinished
				resp.Data.ProofBytes = fmt.Sprintf("0x%x", expectedProof)
			} else {
				resp.Data.Status = TaskStatusPending
			}
			json.NewEncoder(w).Encode(resp)
		}
	}))
	defer server.Close()

	client := NewProverClient(server.URL, 10*time.Millisecond, testlog.Logger(t, log.LvlInfo))
	proof, err := client.ProveAndWait(context.Background(), ProveRequest{})
	require.NoError(t, err)
	require.Equal(t, expectedProof, proof)
	require.GreaterOrEqual(t, callCount, 2)
}

func TestProveAndWaitContextCancel(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.Method {
		case http.MethodPost:
			resp := ProveResponse{Code: "0", Message: "ok"}
			resp.Data.TaskID = "task-cancel"
			json.NewEncoder(w).Encode(resp)
		case http.MethodGet:
			resp := TaskResult{Code: "0", Message: "ok"}
			resp.Data.Status = TaskStatusPending
			json.NewEncoder(w).Encode(resp)
		}
	}))
	defer server.Close()

	ctx, cancel := context.WithTimeout(context.Background(), 50*time.Millisecond)
	defer cancel()

	client := NewProverClient(server.URL, 10*time.Millisecond, testlog.Logger(t, log.LvlInfo))
	_, err := client.ProveAndWait(ctx, ProveRequest{})
	require.Error(t, err)
	require.ErrorIs(t, err, context.DeadlineExceeded)
}
