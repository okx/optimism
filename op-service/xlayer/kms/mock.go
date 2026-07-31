// X Layer: shared test double for KMSClient.
//
// KMSClient's init method is unexported, which seals the interface — no
// package outside this one can implement it. That is intentional (it forces
// all SDK initialization through MaybeResolve's process-wide once guard), but
// it also means external packages' tests cannot bring their own mock. This
// exported mock is the one implementation they are meant to use, swapped in
// via SetClient. It lives in a non-test file so other packages can import it;
// the linker drops it from binaries that never reference it.

package kms

import "errors"

// MockKMSClient is a configurable KMSClient for tests, in this package and
// others (via SetClient). The zero value is usable: init succeeds and every
// lookup returns an empty value.
type MockKMSClient struct {
	// InitErr, when set, is returned by init.
	InitErr error
	// GetErr, when set, is returned by every GetSecretValue call.
	GetErr error
	// Values, when non-nil, maps key names to secret values and unknown keys
	// fail with "key not found". When nil, every key resolves to Value.
	Values map[string]string
	// Value is the secret returned for any key while Values is nil.
	Value string

	// InitCalls counts init invocations.
	InitCalls int
	// GetHits counts GetSecretValue invocations.
	GetHits int
	// GetArgs records the key of each GetSecretValue call in order.
	GetArgs []string
}

func (m *MockKMSClient) init() error {
	m.InitCalls++
	return m.InitErr
}

// GetSecretValue implements KMSClient with the canned behavior described on
// the struct fields.
func (m *MockKMSClient) GetSecretValue(key string) (string, error) {
	m.GetHits++
	m.GetArgs = append(m.GetArgs, key)
	if m.GetErr != nil {
		return "", m.GetErr
	}
	if m.Values != nil {
		v, ok := m.Values[key]
		if !ok {
			return "", errors.New("key not found")
		}
		return v, nil
	}
	return m.Value, nil
}

// LastGetArg returns the key of the most recent GetSecretValue call, or ""
// if there has been none.
func (m *MockKMSClient) LastGetArg() string {
	if len(m.GetArgs) == 0 {
		return ""
	}
	return m.GetArgs[len(m.GetArgs)-1]
}
