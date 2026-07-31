package kms

import (
	"errors"
	"testing"

	"github.com/stretchr/testify/require"
)

// withClient swaps the package-level client for the duration of a test, going
// through SetClient so the one-time initialization is re-armed for the new
// client.
func withClient(t *testing.T, c KMSClient) {
	t.Helper()
	prev := client
	SetClient(c)
	t.Cleanup(func() { SetClient(prev) })
}

func TestIsKMSRef(t *testing.T) {
	require.True(t, IsKMSRef("kms:my-key"))
	require.True(t, IsKMSRef("  kms:my-key"), "leading whitespace should be tolerated")
	require.False(t, IsKMSRef("deadbeef"))
	require.False(t, IsKMSRef(""))
	require.False(t, IsKMSRef("{encrypt_3}abc"), "ciphertext prefix is not a kms ref")
}

func TestMaybeResolve_PlaintextPassthrough(t *testing.T) {
	m := &MockKMSClient{Value: "should-not-be-used"}
	withClient(t, m)

	for _, in := range []string{"", "deadbeef", "0xabc123", "/path/to/jwt"} {
		out, err := MaybeResolve(in)
		require.NoError(t, err)
		require.Equal(t, in, out, "plaintext must pass through unchanged")
	}
	require.Zero(t, m.InitCalls, "Init must not be called for plaintext")
	require.Zero(t, m.GetHits, "GetSecretValue must not be called for plaintext")
}

func TestMaybeResolve_ResolvesRef(t *testing.T) {
	m := &MockKMSClient{Value: "abcdef0123"}
	withClient(t, m)

	out, err := MaybeResolve("kms:rpcarcnode-p2p-priv-raw")
	require.NoError(t, err)
	require.Equal(t, "abcdef0123", out)
	require.Equal(t, 1, m.InitCalls, "Init must run before lookup")
	require.Equal(t, "rpcarcnode-p2p-priv-raw", m.LastGetArg(), "prefix is stripped, bare key name is looked up")
}

func TestMaybeResolve_TrimsWhitespace(t *testing.T) {
	m := &MockKMSClient{Value: "plain"}
	withClient(t, m)

	out, err := MaybeResolve("  kms:my-key\n")
	require.NoError(t, err)
	require.Equal(t, "plain", out)
	require.Equal(t, "my-key", m.LastGetArg(), "key name is trimmed before lookup")
}

func TestMaybeResolve_EmptyKey(t *testing.T) {
	m := &MockKMSClient{}
	withClient(t, m)

	_, err := MaybeResolve("kms:")
	require.ErrorContains(t, err, "empty KMS key name")
	require.Zero(t, m.InitCalls, "Init must not run for an empty key name")
}

func TestMaybeResolve_EmptyValueFailsFast(t *testing.T) {
	// A kms: ref that resolves to empty (or whitespace-only) must error, not
	// silently pass an empty string through to the caller.
	for _, v := range []string{"", "  ", "\n"} {
		m := &MockKMSClient{Value: v}
		withClient(t, m)
		_, err := MaybeResolve("kms:my-key")
		require.ErrorContains(t, err, "resolved to an empty value")
	}
}

func TestMaybeResolve_InitError(t *testing.T) {
	m := &MockKMSClient{InitErr: errors.New("boom")}
	withClient(t, m)

	_, err := MaybeResolve("kms:x")
	require.ErrorContains(t, err, "kms.Init() failed")
	require.Zero(t, m.GetHits, "GetSecretValue must not run if Init fails")
}

func TestMaybeResolve_GetError(t *testing.T) {
	m := &MockKMSClient{GetErr: errors.New("not found")}
	withClient(t, m)

	_, err := MaybeResolve("kms:x")
	require.ErrorContains(t, err, "kms.GetSecretValue")
}
