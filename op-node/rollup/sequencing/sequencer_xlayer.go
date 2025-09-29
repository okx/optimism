package sequencing

import (
	"context"
	"fmt"

	"github.com/ethereum/go-ethereum/log"
	realtimeKafka "github.com/ethereum/go-ethereum/realtime/kafka"
)

func (s *Sequencer) InitRealtimeXLayer() {
	if s.rollupCfg.Realtime.SequencerEnable {
		kafkaProducer, err := realtimeKafka.NewKafkaProducer(s.rollupCfg.Realtime.Kafka, context.Background(), nil)
		if err != nil {
			kafkaProducer = nil
			log.Warn("[Realtime] Failed to initialize kafka producer", "error", err)
		}
		s.realtimeProducer = kafkaProducer
		log.Info("[Realtime] Realtime initialized on op-node sequencer")
	}
}

func (s *Sequencer) SendRealtimeErrorTrigger(height uint64) {
	if s.active.Load() && s.rollupCfg != nil && s.rollupCfg.Realtime != nil && s.rollupCfg.Realtime.SequencerEnable {
		if err := s.realtimeProducer.SendKafkaErrorTrigger(height); err != nil {
			log.Error(fmt.Sprintf("[Realtime] Failed to send kafka error trigger message. error: %v", err))
		}
	}
}

func (s *Sequencer) isRealtimeEnabled() bool {
	if s.active.Load() && s.rollupCfg != nil && s.rollupCfg.Realtime != nil {
		return s.rollupCfg.Realtime.SequencerEnable
	}
	return false
}
