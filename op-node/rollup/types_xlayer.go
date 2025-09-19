package rollup

import (
	realtimeKafka "github.com/ethereum/go-ethereum/realtime/kafka"
)

type RealtimeConfig struct {
	Enable bool
	Kafka  realtimeKafka.KafkaConfig
}

var DefaultRealtimeConfig = RealtimeConfig{
	Enable: false,
	Kafka:  realtimeKafka.KafkaConfig{},
}
