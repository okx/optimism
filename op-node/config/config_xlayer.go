package config

import (
	"os"
	"strings"

	"github.com/ethereum-optimism/optimism/op-node/flags"
	"github.com/ethereum-optimism/optimism/op-node/rollup"
	realtimeKafka "github.com/ethereum/go-ethereum/realtime/kafka"
	"github.com/urfave/cli/v2"
)

const EnvKafkaConsumerGroupID = "REALTIME_KAFKA_CONSUMER_GROUP_ID"
const EnvKafkaConsumerClientID = "REALTIME_KAFKA_CONSUMER_CLIENT_ID"

type ApolloConfig struct {
	Enable    bool
	AppID     string
	IP        string
	Cluster   string
	Namespace string
}

func ApplyXLayerFlags(ctx *cli.Context, cfg *Config) {
	applyApolloFlags(ctx, cfg)
	applyRealtimeFlags(ctx, cfg)
}

func applyApolloFlags(ctx *cli.Context, cfg *Config) {
	cfg.Apollo = ApolloConfig{
		Enable:    ctx.Bool(flags.ApolloEnabledFlag.Name),
		AppID:     ctx.String(flags.ApolloAppIDFlag.Name),
		IP:        ctx.String(flags.ApolloIPFlag.Name),
		Cluster:   ctx.String(flags.ApolloClusterFlag.Name),
		Namespace: ctx.String(flags.ApolloNamespaceFlag.Name),
	}
}

func applyRealtimeFlags(ctx *cli.Context, cfg *Config) {
	realtimeCfg := rollup.DefaultRealtimeConfig
	seqRealtimeEnabled := ctx.Bool(flags.RealtimeSequencerEnableFlag.Name)
	if seqRealtimeEnabled {
		realtimeCfg.SequencerEnable = seqRealtimeEnabled
		groupID := ctx.String(flags.RealtimeKafkaSyncGroupID.Name)
		if envGroupID := os.Getenv(EnvKafkaConsumerGroupID); envGroupID != "" {
			// Override consumer group id if env variable is set
			groupID = envGroupID
		}
		clientID := ctx.String(flags.RealtimeKafkaSyncClientID.Name)
		if envClientID := os.Getenv(EnvKafkaConsumerClientID); envClientID != "" {
			// Override consumer client id if env variable is set
			clientID = envClientID
		}
		realtimeCfg.Kafka = realtimeKafka.KafkaConfig{
			BootstrapServers: strings.Split(ctx.String(flags.RealtimeKafkaSyncBootstrapServers.Name), ","),
			BlockTopic:       ctx.String(flags.RealtimeKafkaSyncBlockTopic.Name),
			ErrorTopic:       ctx.String(flags.RealtimeKafkaSyncErrorTopic.Name),
			ClientID:         clientID,
			GroupID:          groupID,
		}
	}
	cfg.Rollup.Realtime = &realtimeCfg
}
