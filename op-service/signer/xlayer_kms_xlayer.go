//go:build kms

// Build instructions (internal network only):
//
//	GOPRIVATE=gitlab.okg.com go get gitlab.okg.com/okcoin-commons/ok-aliyun-kms-go@v1.0.0
//	go install -tags kms ./op-proposer/cmd/op-proposer
//	go install -tags kms ./op-challenger/cmd/op-challenger
//
// Runtime: set KMS_REGION, bind a RAM role, and pass:
//
//	--xlayer-signer.enable-kms=true
//	--xlayer-signer.secret-key=<kms-secret-name>

package signer

import (
	"fmt"

	"github.com/ethereum/go-ethereum/log"
	"gitlab.okg.com/okcoin-commons/ok-aliyun-kms-go/kms"
)

// resolveSecretKeyFromKMS initializes Aliyun KMS and fetches the plaintext AES key
// stored under secretName. The host environment must have KMS_REGION set; the machine
// role grants access without explicit credentials.
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
