// X Layer: KMS-backed secret resolution.
//
// Secrets live in the company KMS Secrets Manager and are referenced by key
// name on the existing flags using a "kms:" prefix, e.g.
// --p2p.priv.raw=kms:rpcarcnode-p2p-priv-raw. At load time the referenced key
// is fetched (already decrypted by the SDK at Init) and its plaintext is used.
// Values without the prefix pass through untouched, so non-KMS deployments
// behave exactly as upstream.

package kms

import (
	"fmt"
	"strings"
)

// KMSRefPrefix marks a flag/file value as a KMS Secrets Manager key reference.
const KMSRefPrefix = "kms:"

// KMSClient abstracts the KMS SDK for testability.
type KMSClient interface {
	Init() error
	GetSecretValue(key string) (string, error)
}

// client is the package-level KMS client used by MaybeResolve. It defaults to
// the real ok-kms-go SDK adapter and can be swapped in tests via SetClient.
var client KMSClient = &SDKClient{}

// SetClient overrides the package-level KMS client. Intended for tests.
func SetClient(c KMSClient) { client = c }

// IsKMSRef reports whether s is a KMS key reference (carries the kms: prefix).
func IsKMSRef(s string) bool {
	return strings.HasPrefix(strings.TrimSpace(s), KMSRefPrefix)
}

// MaybeResolve returns s unchanged unless it is a "kms:<keyname>" reference, in
// which case it initializes the KMS SDK and returns the plaintext value of that
// secret key. The SDK reads its configuration (provider, region, secret names)
// from the KMS_PROVIDER / KMS_REGION / KMS_SECRET_NAME environment variables.
func MaybeResolve(s string) (string, error) {
	if !IsKMSRef(s) {
		return s, nil
	}
	key := strings.TrimSpace(strings.TrimPrefix(strings.TrimSpace(s), KMSRefPrefix))
	if key == "" {
		return "", fmt.Errorf("empty KMS key name in %q", s)
	}
	if err := client.Init(); err != nil {
		return "", fmt.Errorf("kms.Init() failed: %w", err)
	}
	val, err := client.GetSecretValue(key)
	if err != nil {
		return "", fmt.Errorf("kms.GetSecretValue(%q) failed: %w", key, err)
	}
	// Trim surrounding whitespace: KMS-stored secrets often carry a trailing
	// newline, which would otherwise break downstream hex parsing.
	val = strings.TrimSpace(val)
	// A kms: reference is an explicit request for that secret. An empty value
	// must fail fast rather than be passed through: e.g. an empty sequencer key
	// would otherwise silently leave the node with no signer, surfacing only at
	// sequencer promotion. (For p2p priv / JWT, empty already fails downstream,
	// but erroring here gives a clear, uniform message.)
	if val == "" {
		return "", fmt.Errorf("kms key %q resolved to an empty value", key)
	}
	return val, nil
}
