package client

import "time"

func (w *PollingClient) SetPollRate(rate time.Duration) {
	w.pollRate = rate
}

func (w *PollingClient) GetPollRate() time.Duration {
	return w.pollRate
}
