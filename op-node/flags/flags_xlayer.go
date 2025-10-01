package flags

import (
	"github.com/urfave/cli/v2"
)

var (
	// Realtime feature
	RealtimeSequencerEnableFlag = &cli.BoolFlag{
		Name:  "realtime.sequencer-enabled",
		Usage: "Kafka sync enable flag",
		Value: true,
	}
	RealtimeKafkaSyncBootstrapServers = &cli.StringFlag{
		Name:  "realtime.kafka-sync-bootstrap-servers",
		Usage: "Kafka sync bootstrap servers",
		Value: "",
	}
	RealtimeKafkaSyncHeaderTopic = &cli.StringFlag{
		Name:  "realtime.kafka-sync-header-topic",
		Usage: "Kafka header topic",
		Value: "",
	}
	RealtimeKafkaSyncBlockTopic = &cli.StringFlag{
		Name:  "realtime.kafka-sync-block-topic",
		Usage: "Kafka block topic",
		Value: "",
	}
	RealtimeKafkaSyncErrorTopic = &cli.StringFlag{
		Name:  "realtime.kafka-sync-error-topic",
		Usage: "Kafka error trigger topic",
		Value: "",
	}
	RealtimeKafkaSyncClientID = &cli.StringFlag{
		Name:  "realtime.kafka-sync-client-id",
		Usage: "Kafka sync client id",
		Value: "",
	}
	RealtimeKafkaSyncGroupID = &cli.StringFlag{
		Name:  "realtime.kafka-sync-group-id",
		Usage: "Kafka sync group id",
		Value: "",
	}

	// XLayerFlags are the default flags for X Layer features
	XLayerFlags = []cli.Flag{
		RealtimeSequencerEnableFlag,
		RealtimeKafkaSyncBootstrapServers,
		RealtimeKafkaSyncHeaderTopic,
		RealtimeKafkaSyncBlockTopic,
		RealtimeKafkaSyncErrorTopic,
		RealtimeKafkaSyncClientID,
		RealtimeKafkaSyncGroupID,
	}
)
