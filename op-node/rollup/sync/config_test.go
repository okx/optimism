package sync

import (
	"testing"

	"github.com/stretchr/testify/require"
)

func TestShouldSkipFollowSourceL1Check(t *testing.T) {
	tests := []struct {
		name                    string
		skipFollowSourceL1Check bool
		l2FollowSourceEndpoint  string
		expected                bool
	}{
		{
			name:                    "both disabled",
			skipFollowSourceL1Check: false,
			l2FollowSourceEndpoint:  "",
			expected:                false,
		},
		{
			name:                    "skip enabled but no follow source",
			skipFollowSourceL1Check: true,
			l2FollowSourceEndpoint:  "",
			expected:                false,
		},
		{
			name:                    "follow source enabled but skip disabled",
			skipFollowSourceL1Check: false,
			l2FollowSourceEndpoint:  "http://localhost:8545",
			expected:                false,
		},
		{
			name:                    "both enabled",
			skipFollowSourceL1Check: true,
			l2FollowSourceEndpoint:  "http://localhost:8545",
			expected:                true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := &Config{
				SkipFollowSourceL1Check: tt.skipFollowSourceL1Check,
				L2FollowSourceEndpoint:  tt.l2FollowSourceEndpoint,
			}
			require.Equal(t, tt.expected, cfg.ShouldSkipFollowSourceL1Check())
		})
	}
}

func TestFollowSourceEnabled(t *testing.T) {
	tests := []struct {
		name     string
		endpoint string
		expected bool
	}{
		{"empty endpoint", "", false},
		{"set endpoint", "http://localhost:8545", true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := &Config{L2FollowSourceEndpoint: tt.endpoint}
			require.Equal(t, tt.expected, cfg.FollowSourceEnabled())
		})
	}
}
