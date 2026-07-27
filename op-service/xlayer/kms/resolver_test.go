package kms

import (
	"errors"
	"testing"

	"github.com/stretchr/testify/require"
)

// mockClient is a test KMSClient that records calls and returns canned results.
type mockClient struct {
	initErr error
	getErr  error
	value   string

	initCalls int
	getArg    string
	getHits   int
}

func (m *mockClient) Init() error { m.initCalls++; return m.initErr }

func (m *mockClient) GetSecretValue(key string) (string, error) {
	m.getHits++
	m.getArg = key
	if m.getErr != nil {
		return "", m.getErr
	}
	return m.value, nil
}

// withClient swaps the package-level client for the duration of a test.
func withClient(t *testing.T, c KMSClient) {
	t.Helper()
	prev := client
	client = c
	t.Cleanup(func() { client = prev })
}

func TestIsKMSRef(t *testing.T) {
	require.True(t, IsKMSRef("kms:my-key"))
	require.True(t, IsKMSRef("  kms:my-key"), "leading whitespace should be tolerated")
	require.False(t, IsKMSRef("deadbeef"))
	require.False(t, IsKMSRef(""))
	require.False(t, IsKMSRef("{encrypt_3}abc"), "ciphertext prefix is not a kms ref")
}

func TestMaybeResolve_PlaintextPassthrough(t *testing.T) {
	m := &mockClient{value: "should-not-be-used"}
	withClient(t, m)

	for _, in := range []string{"", "deadbeef", "0xabc123", "/path/to/jwt"} {
		out, err := MaybeResolve(in)
		require.NoError(t, err)
		require.Equal(t, in, out, "plaintext must pass through unchanged")
	}
	require.Zero(t, m.initCalls, "Init must not be called for plaintext")
	require.Zero(t, m.getHits, "GetSecretValue must not be called for plaintext")
}

func TestMaybeResolve_ResolvesRef(t *testing.T) {
	m := &mockClient{value: "abcdef0123"}
	withClient(t, m)

	out, err := MaybeResolve("kms:rpcarcnode-p2p-priv-raw")
	require.NoError(t, err)
	require.Equal(t, "abcdef0123", out)
	require.Equal(t, 1, m.initCalls, "Init must run before lookup")
	require.Equal(t, "rpcarcnode-p2p-priv-raw", m.getArg, "prefix is stripped, bare key name is looked up")
}

func TestMaybeResolve_TrimsWhitespace(t *testing.T) {
	m := &mockClient{value: "plain"}
	withClient(t, m)

	out, err := MaybeResolve("  kms:my-key\n")
	require.NoError(t, err)
	require.Equal(t, "plain", out)
	require.Equal(t, "my-key", m.getArg, "key name is trimmed before lookup")
}

func TestMaybeResolve_EmptyKey(t *testing.T) {
	m := &mockClient{}
	withClient(t, m)

	_, err := MaybeResolve("kms:")
	require.ErrorContains(t, err, "empty KMS key name")
	require.Zero(t, m.initCalls, "Init must not run for an empty key name")
}

func TestMaybeResolve_EmptyValueFailsFast(t *testing.T) {
	// A kms: ref that resolves to empty (or whitespace-only) must error, not
	// silently pass an empty string through to the caller.
	for _, v := range []string{"", "  ", "\n"} {
		m := &mockClient{value: v}
		withClient(t, m)
		_, err := MaybeResolve("kms:my-key")
		require.ErrorContains(t, err, "resolved to an empty value")
	}
}

func TestMaybeResolve_InitError(t *testing.T) {
	m := &mockClient{initErr: errors.New("boom")}
	withClient(t, m)

	_, err := MaybeResolve("kms:x")
	require.ErrorContains(t, err, "kms.Init() failed")
	require.Zero(t, m.getHits, "GetSecretValue must not run if Init fails")
}

func TestMaybeResolve_GetError(t *testing.T) {
	m := &mockClient{getErr: errors.New("not found")}
	withClient(t, m)

	_, err := MaybeResolve("kms:x")
	require.ErrorContains(t, err, "kms.GetSecretValue")
}
