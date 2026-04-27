//go:build kms

// Build instructions for KMS-enabled binaries (internal network only):
//
// Step 1 — add the dependency (one-time, requires internal network access):
//
//	GOPRIVATE=gitlab.okg.com go get gitlab.okg.com/okcoin-commons/ok-aliyun-kms-go@v1.0.0
//
// Step 2 — install the binaries with the kms build tag:
//
//	go install -tags kms ./op-proposer/cmd/op-proposer
//	go install -tags kms ./op-challenger/cmd/op-challenger
//
// Runtime prerequisites:
//   - KMS_REGION env var must be set.
//   - The host machine must have a RAM role bound that grants KMS access
//   - --xlayer-signer.secret-key must be set to the KMS secret name (not the raw key)
//   - --xlayer-signer.enable-kms=true

package signer

import (
	"fmt"

	"github.com/ethereum/go-ethereum/log"
	"gitlab.okg.com/okcoin-commons/ok-aliyun-kms-go/kms"
)

// resolveSecretKeyFromKMS initializes Aliyun KMS and fetches the plaintext AES key
// stored under secretName. The host environment must have KMS_REGION set; the machine
// role grants access without explicit credentials.
//
// Build with -tags kms to enable this implementation.
func resolveSecretKeyFromKMS(logger log.Logger, secretName string) (string, error) {
	logger.Info("Initializing Aliyun KMS", "secretName", secretName)

	if err := kms.Init(); err != nil {
		return "", fmt.Errorf("kms init failed: %w", err)
	}

	value, err := kms.GetSecretValue(secretName)
	if err != nil {
		return "", fmt.Errorf("kms get secret %q failed: %w", secretName, err)
	}
	if value == "" {
		return "", fmt.Errorf("kms returned empty value for secret %q", secretName)
	}

	logger.Info("Successfully resolved secret key from KMS")
	return value, nil
}
