package sources

import (
	"time"

	"github.com/ethereum-optimism/optimism/op-service/client"
)

// PollRateConfigurable is an interface for clients that can have their poll rate configured
type PollRateConfigurable interface {
	SetPollRate(duration time.Duration)
}

func (lc *limitClient) SetPollRate(duration time.Duration) {
	lc.c.(*client.PollingClient).SetPollRate(duration)
}
