package tee

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"sync/atomic"
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
		require.Equal(t, "/tee/task/", r.URL.Path)
		require.Equal(t, "application/json", r.Header.Get("Content-Type"))

		var req ProveRequest
		err := json.NewDecoder(r.Body).Decode(&req)
		require.NoError(t, err)
		require.Equal(t, common.Hash{0x01}, req.StartBlkStateHash)
		require.Equal(t, common.Hash{0x02}, req.EndBlkStateHash)
		require.Equal(t, uint64(100), req.StartBlkHeight)
		require.Equal(t, uint64(200), req.EndBlkHeight)
		require.Equal(t, common.Hash{0x03}, req.StartBlkHash)
		require.Equal(t, common.Hash{0x04}, req.EndBlkHash)

		data, _ := json.Marshal(CreateTaskData{TaskID: expectedTaskID})
		resp := ProverResponse{Code: codeOK, Message: "ok", Data: data}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	client := NewProverClient(server.URL, time.Second, testlog.Logger(t, log.LvlInfo))
	taskID, err := client.Prove(context.Background(), ProveRequest{
		StartBlkStateHash: common.Hash{0x01},
		EndBlkStateHash:   common.Hash{0x02},
		StartBlkHeight:    100,
		EndBlkHeight:      200,
		StartBlkHash:      common.Hash{0x03},
		EndBlkHash:        common.Hash{0x04},
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

func TestProveNonRetryableError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		resp := ProverResponse{Code: codeInvalidParams, Message: "invalid params"}
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	client := NewProverClient(server.URL, time.Second, testlog.Logger(t, log.LvlInfo))
	_, err := client.Prove(context.Background(), ProveRequest{})
	require.Error(t, err)
	require.ErrorIs(t, err, errNonRetryable)
}

func TestProveRetryableErrorCode(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		resp := ProverResponse{Code: codeInternalError, Message: "internal error"}
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	client := NewProverClient(server.URL, time.Second, testlog.Logger(t, log.LvlInfo))
	_, err := client.Prove(context.Background(), ProveRequest{})
	require.Error(t, err)
	require.NotErrorIs(t, err, errNonRetryable, "internal error should be retryable")
}

func TestGetTaskFinished(t *testing.T) {
	proofHex := "0xdeadbeef"
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		require.Equal(t, http.MethodGet, r.Method)
		require.Equal(t, "/tee/task/task-123", r.URL.Path)
		data, _ := json.Marshal(TaskResultData{
			Status:     TaskStatusFinished,
			ProofBytes: proofHex,
		})
		resp := ProverResponse{Code: codeOK, Message: "ok", Data: data}
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	client := NewProverClient(server.URL, time.Second, testlog.Logger(t, log.LvlInfo))
	result, err := client.GetTaskResult(context.Background(), "task-123")
	require.NoError(t, err)
	require.Equal(t, TaskStatusFinished, result.Status)
	require.Equal(t, proofHex, result.ProofBytes)
}

func TestGetTaskRunning(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		data, _ := json.Marshal(TaskResultData{Status: TaskStatusRunning})
		resp := ProverResponse{Code: codeOK, Message: "ok", Data: data}
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	client := NewProverClient(server.URL, time.Second, testlog.Logger(t, log.LvlInfo))
	result, err := client.GetTaskResult(context.Background(), "task-456")
	require.NoError(t, err)
	require.Equal(t, TaskStatusRunning, result.Status)
	require.Empty(t, result.ProofBytes)
}

func TestGetTaskNotFound(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		resp := ProverResponse{Code: codeTaskNotFound, Message: "not found"}
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	client := NewProverClient(server.URL, time.Second, testlog.Logger(t, log.LvlInfo))
	_, err := client.GetTaskResult(context.Background(), "task-789")
	require.Error(t, err)
	require.Contains(t, err.Error(), "not found")
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
	var getCount atomic.Int32
	expectedProof := []byte{0xde, 0xad, 0xbe, 0xef}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.Method {
		case http.MethodPost:
			data, _ := json.Marshal(CreateTaskData{TaskID: "task-wait"})
			resp := ProverResponse{Code: codeOK, Message: "ok", Data: data}
			json.NewEncoder(w).Encode(resp)
		case http.MethodGet:
			count := getCount.Add(1)
			var data []byte
			if count >= 2 {
				data, _ = json.Marshal(TaskResultData{
					Status:     TaskStatusFinished,
					ProofBytes: fmt.Sprintf("0x%x", expectedProof),
				})
			} else {
				data, _ = json.Marshal(TaskResultData{Status: TaskStatusRunning})
			}
			resp := ProverResponse{Code: codeOK, Message: "ok", Data: data}
			json.NewEncoder(w).Encode(resp)
		}
	}))
	defer server.Close()

	client := NewProverClient(server.URL, 10*time.Millisecond, testlog.Logger(t, log.LvlInfo))
	proof, err := client.ProveAndWait(context.Background(), ProveRequest{})
	require.NoError(t, err)
	require.Equal(t, expectedProof, proof)
	require.GreaterOrEqual(t, int(getCount.Load()), 2)
}

func TestProveAndWaitRetryAfterFailed(t *testing.T) {
	var postCount atomic.Int32
	var getCount atomic.Int32
	expectedProof := []byte{0xca, 0xfe}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.Method {
		case http.MethodPost:
			count := postCount.Add(1)
			taskID := fmt.Sprintf("task-%d", count)
			data, _ := json.Marshal(CreateTaskData{TaskID: taskID})
			resp := ProverResponse{Code: codeOK, Message: "ok", Data: data}
			json.NewEncoder(w).Encode(resp)
		case http.MethodGet:
			count := getCount.Add(1)
			var data []byte
			if postCount.Load() == 1 {
				// First task: always fail
				data, _ = json.Marshal(TaskResultData{
					Status: TaskStatusFailed,
					Detail: "compute error",
				})
			} else if count >= 3 {
				// Second task: finish after a poll
				data, _ = json.Marshal(TaskResultData{
					Status:     TaskStatusFinished,
					ProofBytes: fmt.Sprintf("0x%x", expectedProof),
				})
			} else {
				data, _ = json.Marshal(TaskResultData{Status: TaskStatusRunning})
			}
			resp := ProverResponse{Code: codeOK, Message: "ok", Data: data}
			json.NewEncoder(w).Encode(resp)
		}
	}))
	defer server.Close()

	client := NewProverClient(server.URL, 10*time.Millisecond, testlog.Logger(t, log.LvlInfo))
	proof, err := client.ProveAndWait(context.Background(), ProveRequest{})
	require.NoError(t, err)
	require.Equal(t, expectedProof, proof)
	require.GreaterOrEqual(t, int(postCount.Load()), 2, "should have re-submitted after failure")
}

func TestProveAndWaitNonRetryableError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Return code=10001 (invalid params) — non-retryable
		resp := ProverResponse{Code: codeInvalidParams, Message: "bad params"}
		json.NewEncoder(w).Encode(resp)
	}))
	defer server.Close()

	client := NewProverClient(server.URL, 10*time.Millisecond, testlog.Logger(t, log.LvlInfo))
	_, err := client.ProveAndWait(context.Background(), ProveRequest{})
	require.Error(t, err)
	require.ErrorIs(t, err, errNonRetryable)
}

func TestProveAndWaitRetryAfterPostError(t *testing.T) {
	var postCount atomic.Int32
	expectedProof := []byte{0xbb}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.Method {
		case http.MethodPost:
			count := postCount.Add(1)
			if count == 1 {
				// First POST: server error (retryable)
				w.WriteHeader(http.StatusInternalServerError)
				w.Write([]byte("overloaded"))
				return
			}
			data, _ := json.Marshal(CreateTaskData{TaskID: "task-retry"})
			resp := ProverResponse{Code: codeOK, Message: "ok", Data: data}
			json.NewEncoder(w).Encode(resp)
		case http.MethodGet:
			data, _ := json.Marshal(TaskResultData{
				Status:     TaskStatusFinished,
				ProofBytes: fmt.Sprintf("0x%x", expectedProof),
			})
			resp := ProverResponse{Code: codeOK, Message: "ok", Data: data}
			json.NewEncoder(w).Encode(resp)
		}
	}))
	defer server.Close()

	client := NewProverClient(server.URL, 10*time.Millisecond, testlog.Logger(t, log.LvlInfo))
	proof, err := client.ProveAndWait(context.Background(), ProveRequest{})
	require.NoError(t, err)
	require.Equal(t, expectedProof, proof)
	require.GreaterOrEqual(t, int(postCount.Load()), 2)
}

func TestProveAndWaitContextCancel(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.Method {
		case http.MethodPost:
			data, _ := json.Marshal(CreateTaskData{TaskID: "task-cancel"})
			resp := ProverResponse{Code: codeOK, Message: "ok", Data: data}
			json.NewEncoder(w).Encode(resp)
		case http.MethodGet:
			data, _ := json.Marshal(TaskResultData{Status: TaskStatusRunning})
			resp := ProverResponse{Code: codeOK, Message: "ok", Data: data}
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
