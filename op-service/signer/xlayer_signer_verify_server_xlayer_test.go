package signer

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/ethereum/go-ethereum/log"
	"github.com/stretchr/testify/require"

	"github.com/ethereum-optimism/optimism/op-service/testlog"
)

// --- handler unit tests (no real TCP listener) ---

func newTestVerifyServer(t *testing.T, hasOrder func(string) bool) *XLayerSignerVerifyServer {
	t.Helper()
	logger := testlog.Logger(t, log.LevelDebug)
	srv, err := NewXLayerSignerVerifyServer(logger, "127.0.0.1:0", hasOrder)
	require.NoError(t, err)
	t.Cleanup(func() { _ = srv.Close() })
	return srv
}

func callHandler(t *testing.T, s *XLayerSignerVerifyServer, method, url string) *httptest.ResponseRecorder {
	t.Helper()
	req := httptest.NewRequest(method, url, nil)
	rr := httptest.NewRecorder()
	s.handleGetRefOrderID(rr, req)
	return rr
}

func decodeResult(t *testing.T, rr *httptest.ResponseRecorder) verifyResponseResult {
	t.Helper()
	var result verifyResponseResult
	require.NoError(t, json.NewDecoder(rr.Body).Decode(&result))
	return result
}

func TestVerifyHandler_Found(t *testing.T) {
	srv := newTestVerifyServer(t, func(id string) bool { return id == "order-123" })

	rr := callHandler(t, srv, http.MethodGet, "/signer/get?refOrderId=order-123")

	require.Equal(t, http.StatusOK, rr.Code)
	result := decodeResult(t, rr)
	require.Equal(t, 200, result.Status)
	require.Equal(t, 0, result.Code, "code must be 0 when refOrderId is found")
	require.Equal(t, "success", result.Msg)
}

func TestVerifyHandler_NotFound(t *testing.T) {
	srv := newTestVerifyServer(t, func(string) bool { return false })

	rr := callHandler(t, srv, http.MethodGet, "/signer/get?refOrderId=unknown-id")

	require.Equal(t, http.StatusOK, rr.Code)
	result := decodeResult(t, rr)
	require.Equal(t, 200, result.Status)
	require.Equal(t, 1, result.Code, "code must be non-zero when refOrderId is not found")
}

func TestVerifyHandler_MissingParam(t *testing.T) {
	srv := newTestVerifyServer(t, func(string) bool { return true })

	rr := callHandler(t, srv, http.MethodGet, "/signer/get")

	require.Equal(t, http.StatusBadRequest, rr.Code)
	result := decodeResult(t, rr)
	require.Equal(t, 1, result.Code)
}

func TestVerifyHandler_WrongMethod(t *testing.T) {
	srv := newTestVerifyServer(t, func(string) bool { return true })

	rr := callHandler(t, srv, http.MethodPost, "/signer/get?refOrderId=order-123")

	require.Equal(t, http.StatusMethodNotAllowed, rr.Code)
	result := decodeResult(t, rr)
	require.Equal(t, 1, result.Code)
}

// --- integration test: real HTTP listener ---

func TestVerifyServer_RealHTTP(t *testing.T) {
	knownID := "real-order-abc"
	srv := newTestVerifyServer(t, func(id string) bool { return id == knownID })

	endpoint := srv.srv.HTTPEndpoint()
	require.NotEmpty(t, endpoint)

	// Found
	resp, err := http.Get(endpoint + "/signer/get?refOrderId=" + knownID)
	require.NoError(t, err)
	defer resp.Body.Close()
	require.Equal(t, http.StatusOK, resp.StatusCode)
	var found verifyResponseResult
	require.NoError(t, json.NewDecoder(resp.Body).Decode(&found))
	require.Equal(t, 0, found.Code)

	// Not found
	resp2, err := http.Get(endpoint + "/signer/get?refOrderId=other")
	require.NoError(t, err)
	defer resp2.Body.Close()
	var notFound verifyResponseResult
	require.NoError(t, json.NewDecoder(resp2.Body).Decode(&notFound))
	require.Equal(t, 1, notFound.Code)
}

func TestVerifyServer_StopGraceful(t *testing.T) {
	srv := newTestVerifyServer(t, func(string) bool { return false })
	require.NoError(t, srv.Stop(context.Background()))
}

// --- LRU cache tests on XLayerRemoteClient ---

func TestXLayerRemoteClient_HasRefOrderID(t *testing.T) {
	cfg := XLayerConfig{Timeout: 0}
	client := NewXLayerRemoteClient(testlog.Logger(t, log.LevelDebug), cfg)

	const id = "uuid-test-1234"
	require.False(t, client.HasRefOrderID(id), "should not find ID before it is stored")

	client.refOrderCache.Add(id, struct{}{})
	require.True(t, client.HasRefOrderID(id), "should find ID after it is stored")
}

func TestXLayerRemoteClient_CacheEviction(t *testing.T) {
	cfg := XLayerConfig{Timeout: 0}
	client := NewXLayerRemoteClient(testlog.Logger(t, log.LevelDebug), cfg)

	// Fill the cache beyond capacity so the first entry is evicted.
	first := "id-0"
	client.refOrderCache.Add(first, struct{}{})

	for i := 1; i <= refOrderCacheCapacity; i++ {
		client.refOrderCache.Add(fmt.Sprintf("id-%d", i), struct{}{})
	}

	require.False(t, client.HasRefOrderID(first), "oldest entry should be evicted after cache is full")
	require.True(t, client.HasRefOrderID(fmt.Sprintf("id-%d", refOrderCacheCapacity)), "newest entry should still be present")
}

// --- StartVerifyServer wiring test ---

func TestXLayerSignerClient_StartVerifyServer(t *testing.T) {
	cfg := XLayerConfig{Address: "0x0000000000000000000000000000000000000001", Timeout: 0}
	inner := NewXLayerRemoteClient(testlog.Logger(t, log.LevelDebug), cfg)
	client := &XLayerSignerClient{
		logger: testlog.Logger(t, log.LevelDebug),
		client: inner,
		config: cfg,
	}

	// Seed the cache directly
	const id = "wired-order-xyz"
	inner.refOrderCache.Add(id, struct{}{})

	srv, err := client.StartVerifyServer(testlog.Logger(t, log.LevelDebug), "127.0.0.1:0")
	require.NoError(t, err)
	defer srv.Close()

	endpoint := srv.srv.HTTPEndpoint()
	resp, err := http.Get(endpoint + "/signer/get?refOrderId=" + id)
	require.NoError(t, err)
	defer resp.Body.Close()

	var result verifyResponseResult
	require.NoError(t, json.NewDecoder(resp.Body).Decode(&result))
	require.Equal(t, 0, result.Code, "wired client must see IDs seeded into its remote client cache")
}

func TestXLayerSignerClient_StartVerifyServer_EmptyAddr(t *testing.T) {
	cfg := XLayerConfig{Timeout: 0}
	client := &XLayerSignerClient{
		logger: testlog.Logger(t, log.LevelDebug),
		client: NewXLayerRemoteClient(testlog.Logger(t, log.LevelDebug), cfg),
		config: cfg,
	}
	_, err := client.StartVerifyServer(testlog.Logger(t, log.LevelDebug), "")
	require.Error(t, err, "empty address must be rejected")
}
