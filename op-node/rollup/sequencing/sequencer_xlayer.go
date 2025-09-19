package sequencing

import (
	"context"
	"fmt"

	"github.com/ethereum-optimism/optimism/op-node/rollup"
	"github.com/ethereum/go-ethereum/log"
	realtimeKafka "github.com/ethereum/go-ethereum/realtime/kafka"
)

func RealtimeProducerXLayer(rollupCfg *rollup.Config) *realtimeKafka.KafkaProducer {
	if rollupCfg != nil && rollupCfg.Realtime != nil && rollupCfg.Realtime.Enable {
		kafkaProducer, err := realtimeKafka.NewKafkaProducer(rollupCfg.Realtime.Kafka, context.Background(), nil)
		if err != nil {
			log.Error("Failed to create realtime producer", "error", err)
			return nil
		}
		return kafkaProducer
	}
	return nil
}

func (s *Sequencer) SendRealtimeErrorTrigger() {
	if s.rollupCfg != nil && s.rollupCfg.Realtime != nil && s.rollupCfg.Realtime.Enable {
		if err := s.kafkaProducer.SendKafkaErrorTrigger(0); err != nil {
			log.Error(fmt.Sprintf("[Realtime] Failed to send kafka error trigger message. error: %v", err))
		}
	}
}
