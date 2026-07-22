// X Layer: JWT secret loading with KMS support, shared across op-node config
// load sites (L2 engine endpoint, interop RPC).

package kms

import (
	"fmt"
	"os"
	"strings"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/log"

	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-service/rpc"
	xlayerkms "github.com/ethereum-optimism/optimism/op-service/xlayer/kms"
)

// ResolveJWTSecret loads a JWT secret. A kms:<name> reference may be supplied
// either directly as value or as the sole content of the file at value; either
// way it is fetched from KMS and parsed as 32 hex-formatted bytes. Otherwise
// value is treated as a file path via rpc.ObtainJWTSecret (which honors
// generateMissing). Real file read errors (e.g. permissions) are surfaced
// rather than masked by falling through to the generate-missing path.
func ResolveJWTSecret(logger log.Logger, value string, generateMissing bool) (eth.Bytes32, error) {
	// Case 1: the value itself is a KMS reference.
	if xlayerkms.IsKMSRef(value) {
		return jwtFromKMS(value)
	}
	// Case 2: the value is a file path whose content is a KMS reference.
	if trimmed := strings.TrimSpace(value); trimmed != "" {
		data, err := os.ReadFile(trimmed)
		switch {
		case err == nil && xlayerkms.IsKMSRef(string(data)):
			return jwtFromKMS(strings.TrimSpace(string(data)))
		case err != nil && !os.IsNotExist(err):
			// A real read error (permissions, is-a-directory, I/O) must not be
			// masked by the generate-missing fallthrough below.
			return eth.Bytes32{}, fmt.Errorf("failed to read jwt secret file %q: %w", trimmed, err)
		}
	}
	// Case 3: plaintext file (or, if allowed, generate-missing) — upstream path.
	return rpc.ObtainJWTSecret(logger, value, generateMissing)
}

// jwtFromKMS resolves a kms:<name> reference to a 32-byte JWT secret.
func jwtFromKMS(ref string) (eth.Bytes32, error) {
	plain, err := xlayerkms.MaybeResolve(ref)
	if err != nil {
		return eth.Bytes32{}, fmt.Errorf("failed to resolve JWT secret from KMS: %w", err)
	}
	b := common.FromHex(strings.TrimSpace(plain)) // FromHex handles optional '0x' prefix
	if len(b) != 32 {
		return eth.Bytes32{}, fmt.Errorf("invalid jwt secret from KMS, not 32 hex-formatted bytes")
	}
	return eth.Bytes32(b), nil
}
