//go:build !kms

package signer

import (
	"fmt"

	"github.com/ethereum/go-ethereum/log"
)

func resolveSecretKeyFromKMS(_ log.Logger, _ string) (string, error) {
	return "", fmt.Errorf("KMS support not compiled in: rebuild with -tags kms and ensure the KMS dependency is available")
}
