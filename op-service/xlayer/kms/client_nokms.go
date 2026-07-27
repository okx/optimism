//go:build !kms

// X Layer: stub KMSClient for builds without the "kms" build tag.
//
// The real client (client_kms.go) depends on the private module
// gitlab.okg.com/okcoin-commons/ok-kms-go, which is not fetchable outside the
// company network. Keeping the same exported API here means every call site
// compiles unchanged; only the ability to actually resolve a kms:<name>
// reference is gone, and asking for one fails fast with a clear message
// instead of silently falling back to some other secret source.

package kms

import "errors"

// Enabled reports whether this binary was built with KMS support.
const Enabled = false

// ErrDisabled is returned by the stub SDKClient for any KMS operation.
var ErrDisabled = errors.New("KMS support is not compiled into this binary: rebuild with `KMS=1 just <target>` (i.e. -tags kms against go.kms.mod) to use kms:<name> references")

// SDKClient is the no-op stand-in for the ok-kms-go backed client. It exists so
// that the package's exported API is identical with and without the build tag.
type SDKClient struct{}

// Init always fails: there is no KMS SDK linked into this binary.
func (*SDKClient) Init() error { return ErrDisabled }

// GetSecretValue always fails: there is no KMS SDK linked into this binary.
func (*SDKClient) GetSecretValue(string) (string, error) { return "", ErrDisabled }
