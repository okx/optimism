package driver

import "github.com/ethereum-optimism/optimism/op-service/eth"

func (s *SyncDeriver) SetRealtimeXLayer(envelope *eth.ExecutionPayloadEnvelope) {
	if s.Config != nil && s.Config.Realtime != nil {
		envelope.ExecutionPayload.RealtimeEnabled = s.Config.Realtime.Enable
	}
}
