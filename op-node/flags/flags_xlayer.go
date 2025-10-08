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
	// Apollo
	ApolloEnabledFlag = &cli.BoolFlag{
		Name:  "apollo.enabled",
		Usage: "Enable Apollo configuration service",
		Value: false,
	}
	ApolloAppIDFlag = &cli.StringFlag{
		Name:  "apollo.app-id",
		Usage: "Apollo app ID",
		Value: "",
	}
	ApolloIPFlag = &cli.StringFlag{
		Name:  "apollo.ip",
		Usage: "Apollo IP",
		Value: "",
	}
	ApolloClusterFlag = &cli.StringFlag{
		Name:  "apollo.cluster",
		Usage: "Apollo cluster name",
		Value: "default",
	}
	ApolloNamespaceFlag = &cli.StringFlag{
		Name:  "apollo.namespace",
		Usage: "Apollo namespace",
		Value: "application",
	}

	// XLayerFlags are the default flags for X Layer features
	XLayerFlags = []cli.Flag{
		RealtimeSequencerEnableFlag,
		RealtimeKafkaSyncBootstrapServers,
		RealtimeKafkaSyncBlockTopic,
		RealtimeKafkaSyncErrorTopic,
		RealtimeKafkaSyncClientID,
		RealtimeKafkaSyncGroupID,
		ApolloEnabledFlag,
		ApolloAppIDFlag,
		ApolloIPFlag,
		ApolloClusterFlag,
		ApolloNamespaceFlag,
	}
)
