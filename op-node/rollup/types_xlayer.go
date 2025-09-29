package rollup

import (
	realtimeKafka "github.com/ethereum/go-ethereum/realtime/kafka"
)

type RealtimeConfig struct {
	SequencerEnable bool
	Kafka           realtimeKafka.KafkaConfig
}

var DefaultRealtimeConfig = RealtimeConfig{
	SequencerEnable: false,
	Kafka:           realtimeKafka.KafkaConfig{},
}
