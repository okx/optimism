//go:build kms

// X Layer: production KMSClient backed by the company-wide ok-kms-go SDK
// (gitlab.okg.com/okcoin-commons/ok-kms-go), which unifies AWS KMS and Aliyun
// KMS behind a single API. The SDK reads its configuration from the
// KMS_PROVIDER, KMS_REGION and KMS_SECRET_NAME environment variables.
//
// ok-kms-go lives in a private GitLab group that most people cannot reach, so
// this file — the only place in the repo that imports it — sits behind the
// "kms" build tag. Everything else compiles client_nokms.go instead and never
// needs the private module.
//
// The build tag alone is not enough, because a `require` line in go.mod is
// consulted by the module-graph commands regardless of build tags: leaving it
// there broke `go get`, `go mod download`, `go list -m all` and every
// downstream import of this repo for anyone without gitlab.okg.com
// credentials. So the require is kept out of go.mod and lives in a generated
// alternate module file instead. Build with `KMS=1 just <target>`, which
// combines -tags kms with -modfile=go.kms.mod; see the `kms-modfile` recipe in
// the root justfile and the KMS block in justfiles/go.just.

package kms

import (
	okkms "gitlab.okg.com/okcoin-commons/ok-kms-go"
)

// Enabled reports whether this binary was built with KMS support.
const Enabled = true

// SDKClient is the production KMSClient implementation backed by ok-kms-go.
// Importing it has no side effects; nothing connects to KMS until init is
// called (lazily and at most once per process, by MaybeResolve's once guard —
// the method is unexported precisely so no other caller exists).
type SDKClient struct{}

// init fetches and decrypts the configured KMS secret(s) for the provider
// selected by KMS_PROVIDER.
func (*SDKClient) init() error {
	return okkms.Init()
}

// GetSecretValue returns the plaintext value of a secret key from the secret(s)
// loaded at Init.
func (*SDKClient) GetSecretValue(key string) (string, error) {
	return okkms.GetSecretValue(key)
}
