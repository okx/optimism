// X Layer: production KMSClient backed by the company-wide ok-kms-go SDK
// (gitlab.okg.com/okcoin-commons/ok-kms-go), which unifies AWS KMS and Aliyun
// KMS behind a single API. The SDK reads its configuration from the
// KMS_PROVIDER, KMS_REGION and KMS_SECRET_NAME environment variables.

package kms

import (
	okkms "gitlab.okg.com/okcoin-commons/ok-kms-go"
)

// SDKClient is the production KMSClient implementation backed by ok-kms-go.
// Importing it has no side effects; nothing connects to KMS until Init is
// called (lazily, on the first KMS reference).
type SDKClient struct{}

// Init fetches and decrypts the configured KMS secret(s) for the provider
// selected by KMS_PROVIDER. It is idempotent, so repeated calls are cheap.
func (*SDKClient) Init() error {
	return okkms.Init()
}

// GetSecretValue returns the plaintext value of a secret key from the secret(s)
// loaded at Init.
func (*SDKClient) GetSecretValue(key string) (string, error) {
	return okkms.GetSecretValue(key)
}
