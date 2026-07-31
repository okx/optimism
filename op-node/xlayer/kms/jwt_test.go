package kms

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/ethereum/go-ethereum/log"
	"github.com/stretchr/testify/require"

	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-service/testlog"
	xlayerkms "github.com/ethereum-optimism/optimism/op-service/xlayer/kms"
)

// withKMSClient swaps the package-level client for the duration of a test.
func withKMSClient(t *testing.T, c xlayerkms.KMSClient) {
	t.Helper()
	xlayerkms.SetClient(c)
	t.Cleanup(func() { xlayerkms.SetClient(&xlayerkms.SDKClient{}) })
}

// 32 bytes (64 hex chars) of 0xaa.
const jwtHex = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

func wantSecret() eth.Bytes32 {
	var b eth.Bytes32
	for i := range b {
		b[i] = 0xaa
	}
	return b
}

func TestResolveJWTSecret_DirectKMSRef(t *testing.T) {
	// KMS returns the value with a trailing newline to exercise trimming.
	m := &xlayerkms.MockKMSClient{Value: jwtHex + "\n"}
	withKMSClient(t, m)
	secret, err := ResolveJWTSecret(testlog.Logger(t, log.LevelError), "kms:jwt-key", false)
	require.NoError(t, err)
	require.Equal(t, wantSecret(), secret)
	require.Equal(t, "jwt-key", m.LastGetArg())
}

func TestResolveJWTSecret_FileContainsKMSRef(t *testing.T) {
	m := &xlayerkms.MockKMSClient{Value: jwtHex}
	withKMSClient(t, m)
	path := filepath.Join(t.TempDir(), "jwt.txt")
	require.NoError(t, os.WriteFile(path, []byte("kms:jwt-key\n"), 0o600))

	secret, err := ResolveJWTSecret(testlog.Logger(t, log.LevelError), path, false)
	require.NoError(t, err)
	require.Equal(t, wantSecret(), secret)
	require.Equal(t, "jwt-key", m.LastGetArg())
}

func TestResolveJWTSecret_PlaintextFile(t *testing.T) {
	m := &xlayerkms.MockKMSClient{Value: "unused"}
	withKMSClient(t, m)
	path := filepath.Join(t.TempDir(), "jwt.txt")
	require.NoError(t, os.WriteFile(path, []byte(jwtHex), 0o600))

	secret, err := ResolveJWTSecret(testlog.Logger(t, log.LevelError), path, false)
	require.NoError(t, err)
	require.Equal(t, wantSecret(), secret)
	require.Zero(t, m.GetHits, "plaintext file must not consult KMS")
}

func TestResolveJWTSecret_NotKMSBytes(t *testing.T) {
	withKMSClient(t, &xlayerkms.MockKMSClient{Value: "aabb"}) // 2 bytes, not 32
	_, err := ResolveJWTSecret(testlog.Logger(t, log.LevelError), "kms:jwt-key", false)
	require.ErrorContains(t, err, "not 32 hex-formatted bytes")
}

func TestResolveJWTSecret_MissingFileFailsFast(t *testing.T) {
	// generateMissing=false: a missing file must error, not silently generate.
	path := filepath.Join(t.TempDir(), "absent.txt")
	_, err := ResolveJWTSecret(testlog.Logger(t, log.LevelError), path, false)
	require.Error(t, err)

	_, statErr := os.Stat(path)
	require.True(t, os.IsNotExist(statErr), "no secret file should have been generated")
}

func TestResolveJWTSecret_ReadErrorSurfaced(t *testing.T) {
	// A directory path triggers a non-NotExist read error, which must be
	// surfaced rather than masked by the generate-missing fallthrough.
	dir := t.TempDir()
	_, err := ResolveJWTSecret(testlog.Logger(t, log.LevelError), dir, true)
	require.ErrorContains(t, err, "failed to read jwt secret file")
}
