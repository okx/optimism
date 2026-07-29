package signer

import (
	"errors"
	"testing"

	xlayerkms "github.com/ethereum-optimism/optimism/op-service/xlayer/kms"
	"github.com/stretchr/testify/require"
)

type kmsMockClient struct {
	initErr error
	getErr  error
	values  map[string]string

	initCalls int
	getArgs   []string
	getHits   int
}

func (m *kmsMockClient) Init() error { m.initCalls++; return m.initErr }

func (m *kmsMockClient) GetSecretValue(key string) (string, error) {
	m.getHits++
	m.getArgs = append(m.getArgs, key)
	if m.getErr != nil {
		return "", m.getErr
	}
	if v, ok := m.values[key]; ok {
		return v, nil
	}
	return "", errors.New("key not found")
}

func withMockKMS(t *testing.T, c xlayerkms.KMSClient) {
	t.Helper()
	xlayerkms.SetClient(c)
	t.Cleanup(func() { xlayerkms.SetClient(&xlayerkms.SDKClient{}) })
}

func TestResolveKMS_BothKMSRefs(t *testing.T) {
	m := &kmsMockClient{values: map[string]string{
		"xlayer-signer.access-key": "resolved-ak",
		"xlayer-signer.secret-key": "resolved-sk",
	}}
	withMockKMS(t, m)

	c := XLayerCLIConfig{
		Enabled:   true,
		Endpoint:  "https://signer.example.com",
		Address:   "0xabc",
		AccessKey: "kms:xlayer-signer.access-key",
		SecretKey: "kms:xlayer-signer.secret-key",
	}
	err := c.ResolveKMS()
	require.NoError(t, err)
	require.Equal(t, "resolved-ak", c.AccessKey)
	require.Equal(t, "resolved-sk", c.SecretKey)
	require.GreaterOrEqual(t, m.initCalls, 1)
	require.Equal(t, 2, m.getHits)
}

func TestResolveKMS_AccessKeyEmptyValue(t *testing.T) {
	m := &kmsMockClient{values: map[string]string{
		"xlayer-signer.access-key": "",
	}}
	withMockKMS(t, m)

	c := XLayerCLIConfig{
		Enabled:   true,
		AccessKey: "kms:xlayer-signer.access-key",
		SecretKey: "kms:xlayer-signer.secret-key",
	}
	err := c.ResolveKMS()
	require.Error(t, err)
	require.ErrorContains(t, err, "xlayer-signer.access-key")
	require.ErrorContains(t, err, "resolved to an empty value")
}

func TestResolveKMS_SecretKeyEmptyValue(t *testing.T) {
	m := &kmsMockClient{values: map[string]string{
		"ak-key": "valid-ak",
		"sk-key": "",
	}}
	withMockKMS(t, m)

	c := XLayerCLIConfig{
		Enabled:   true,
		AccessKey: "kms:ak-key",
		SecretKey: "kms:sk-key",
	}
	err := c.ResolveKMS()
	require.Error(t, err)
	require.ErrorContains(t, err, "xlayer-signer.secret-key")
	require.ErrorContains(t, err, "resolved to an empty value")
	require.Equal(t, "valid-ak", c.AccessKey)
}

func TestResolveKMS_AccessKeyEmptyKeyName(t *testing.T) {
	m := &kmsMockClient{}
	withMockKMS(t, m)

	c := XLayerCLIConfig{
		Enabled:   true,
		AccessKey: "kms:",
		SecretKey: "kms:valid-key",
	}
	err := c.ResolveKMS()
	require.Error(t, err)
	require.ErrorContains(t, err, "xlayer-signer.access-key")
	require.ErrorContains(t, err, "empty KMS key name")
	require.Equal(t, 0, m.initCalls)
}

func TestResolveKMS_SecretKeyEmptyKeyName(t *testing.T) {
	m := &kmsMockClient{values: map[string]string{
		"ak-key": "resolved-ak",
	}}
	withMockKMS(t, m)

	c := XLayerCLIConfig{
		Enabled:   true,
		AccessKey: "kms:ak-key",
		SecretKey: "kms:",
	}
	err := c.ResolveKMS()
	require.Error(t, err)
	require.ErrorContains(t, err, "xlayer-signer.secret-key")
	require.ErrorContains(t, err, "empty KMS key name")
	require.Equal(t, "resolved-ak", c.AccessKey)
}

func TestResolveKMS_GetSecretValueError(t *testing.T) {
	m := &kmsMockClient{getErr: errors.New("key not found")}
	withMockKMS(t, m)

	c := XLayerCLIConfig{
		Enabled:   true,
		AccessKey: "kms:missing-key",
		SecretKey: "kms:sk-key",
	}
	err := c.ResolveKMS()
	require.Error(t, err)
	require.ErrorContains(t, err, "xlayer-signer.access-key")
	require.ErrorContains(t, err, "kms.GetSecretValue")
}

func TestResolveKMS_BothPlaintext(t *testing.T) {
	m := &kmsMockClient{}
	withMockKMS(t, m)

	c := XLayerCLIConfig{
		Enabled:   true,
		AccessKey: "plaintext-ak",
		SecretKey: "plaintext-sk",
	}
	err := c.ResolveKMS()
	require.NoError(t, err)
	require.Equal(t, "plaintext-ak", c.AccessKey)
	require.Equal(t, "plaintext-sk", c.SecretKey)
	require.Equal(t, 0, m.initCalls)
	require.Equal(t, 0, m.getHits)
}

func TestResolveKMS_MixedAccessKeyKMS(t *testing.T) {
	m := &kmsMockClient{values: map[string]string{
		"ak-key": "resolved-ak",
	}}
	withMockKMS(t, m)

	c := XLayerCLIConfig{
		Enabled:   true,
		AccessKey: "kms:ak-key",
		SecretKey: "plaintext-sk",
	}
	err := c.ResolveKMS()
	require.NoError(t, err)
	require.Equal(t, "resolved-ak", c.AccessKey)
	require.Equal(t, "plaintext-sk", c.SecretKey)
	require.Equal(t, 1, m.getHits)
}

func TestResolveKMS_InitFailure(t *testing.T) {
	m := &kmsMockClient{initErr: errors.New("KMS_PROVIDER not set")}
	withMockKMS(t, m)

	c := XLayerCLIConfig{
		Enabled:   true,
		AccessKey: "kms:ak",
		SecretKey: "kms:sk",
	}
	err := c.ResolveKMS()
	require.Error(t, err)
	require.ErrorContains(t, err, "kms.Init() failed")
	require.Equal(t, 0, m.getHits)
}

func TestResolveKMS_WhitespacePaddedRef(t *testing.T) {
	m := &kmsMockClient{values: map[string]string{
		"my-key": "resolved",
	}}
	withMockKMS(t, m)

	c := XLayerCLIConfig{
		Enabled:   true,
		AccessKey: "  kms:my-key\n",
		SecretKey: "plain-sk",
	}
	err := c.ResolveKMS()
	require.NoError(t, err)
	require.Equal(t, "resolved", c.AccessKey)
	require.Equal(t, "plain-sk", c.SecretKey)
	require.Contains(t, m.getArgs, "my-key")
}

// ToXLayerConfig is the single chokepoint every consumer converts through
// (via NewXLayerSignerClientFromConfig), and since the KMS hook moved out of
// txmgr's NewConfig it is the ONLY place credential references get resolved.
// These tests pin that guarantee.

func TestToXLayerConfig_ResolvesKMSRefs(t *testing.T) {
	m := &kmsMockClient{values: map[string]string{
		"xlayer-signer.access-key": "resolved-ak",
		"xlayer-signer.secret-key": "resolved-sk",
	}}
	withMockKMS(t, m)

	c := XLayerCLIConfig{
		Enabled:   true,
		Endpoint:  "https://signer.example.com",
		Address:   "0xabc",
		AccessKey: "kms:xlayer-signer.access-key",
		SecretKey: "kms:xlayer-signer.secret-key",
	}
	cfg, err := c.ToXLayerConfig()
	require.NoError(t, err)
	require.Equal(t, "resolved-ak", cfg.AccessKey)
	require.Equal(t, "resolved-sk", cfg.SecretKey)
	// The receiver is a value: the caller's config must keep its references,
	// resolution only lives in the converted copy handed to the client.
	require.Equal(t, "kms:xlayer-signer.access-key", c.AccessKey)
}

func TestToXLayerConfig_SurfacesKMSFailure(t *testing.T) {
	m := &kmsMockClient{getErr: errors.New("kms backend down")}
	withMockKMS(t, m)

	c := XLayerCLIConfig{
		Enabled:   true,
		Endpoint:  "https://signer.example.com",
		Address:   "0xabc",
		AccessKey: "kms:xlayer-signer.access-key",
		SecretKey: "plaintext-sk",
	}
	_, err := c.ToXLayerConfig()
	require.ErrorContains(t, err, "failed to resolve xlayer signer credentials from KMS")
}
