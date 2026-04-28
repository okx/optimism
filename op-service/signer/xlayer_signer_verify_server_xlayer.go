package signer

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"

	"github.com/ethereum-optimism/optimism/op-service/httputil"
	"github.com/ethereum/go-ethereum/log"
)

// verifyResponseResult matches the Java ResponseResult<T> envelope expected by the asset management service.
// code=0 means the refOrderId exists; any other code means it does not.
type verifyResponseResult struct {
	Status    int    `json:"status"`
	Code      int    `json:"code"`
	Msg       string `json:"msg"`
	DetailMsg string `json:"detailMsg"`
	Data      any    `json:"data"`
}

// XLayerSignerVerifyServer exposes GET /signer/get so the asset management service can
// confirm that a refOrderId was actually issued by this node before approving the signature.
type XLayerSignerVerifyServer struct {
	logger   log.Logger
	srv      *httputil.HTTPServer
	hasOrder func(string) bool
}

// NewXLayerSignerVerifyServer starts an HTTP server on addr and registers the
// GET /signer/get handler.  hasOrder is called with the refOrderId query
// parameter; it should return true iff that ID was issued by the local signer.
func NewXLayerSignerVerifyServer(logger log.Logger, addr string, hasOrder func(string) bool) (*XLayerSignerVerifyServer, error) {
	s := &XLayerSignerVerifyServer{
		logger:   logger,
		hasOrder: hasOrder,
	}

	mux := http.NewServeMux()
	mux.HandleFunc("/signer/get", s.handleGetRefOrderID)

	srv, err := httputil.StartHTTPServer(addr, mux)
	if err != nil {
		return nil, fmt.Errorf("failed to start signer verify server on %s: %w", addr, err)
	}
	s.srv = srv

	logger.Info("XLayer signer verify server started", "endpoint", srv.HTTPEndpoint())
	return s, nil
}

// handleGetRefOrderID handles GET /signer/get?refOrderId=<id>.
// It returns {"status":200,"code":0} when the ID is found and {"status":200,"code":1} otherwise,
// matching the Java ResponseResult contract (isSuccess() ↔ status==200 && code==0).
func (s *XLayerSignerVerifyServer) handleGetRefOrderID(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		writeVerifyResponse(w, http.StatusMethodNotAllowed, verifyResponseResult{
			Status: http.StatusMethodNotAllowed,
			Code:   1,
			Msg:    "method not allowed",
		})
		return
	}

	refOrderID := r.URL.Query().Get("refOrderId")
	if refOrderID == "" {
		writeVerifyResponse(w, http.StatusBadRequest, verifyResponseResult{
			Status: http.StatusBadRequest,
			Code:   1,
			Msg:    "refOrderId is required",
		})
		return
	}

	if s.hasOrder(refOrderID) {
		writeVerifyResponse(w, http.StatusOK, verifyResponseResult{
			Status: http.StatusOK,
			Code:   0,
			Msg:    "success",
		})
	} else {
		s.logger.Warn("XLayer verify: refOrderId not found", "refOrderId", refOrderID)
		writeVerifyResponse(w, http.StatusOK, verifyResponseResult{
			Status: http.StatusOK,
			Code:   1,
			Msg:    "not found",
		})
	}
}

func writeVerifyResponse(w http.ResponseWriter, httpStatus int, result verifyResponseResult) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(httpStatus)
	_ = json.NewEncoder(w).Encode(result)
}

// Stop gracefully shuts down the server, waiting until ctx is cancelled if needed.
func (s *XLayerSignerVerifyServer) Stop(ctx context.Context) error {
	return s.srv.Stop(ctx)
}

// Close force-closes the server and all active connections.
func (s *XLayerSignerVerifyServer) Close() error {
	return s.srv.Close()
}
