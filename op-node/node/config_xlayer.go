package node

import (
	"os"
	"strings"

	"github.com/ethereum-optimism/optimism/op-node/flags"
	"github.com/ethereum-optimism/optimism/op-node/rollup"
	realtimeKafka "github.com/ethereum/go-ethereum/realtime/kafka"
	"github.com/urfave/cli/v2"
)

const EnvKafkaConsumerGroupID = "REALTIME_KAFKA_CONSUMER_GROUP_ID"

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
	groupID := ctx.String(flags.RealtimeKafkaSyncGroupID.Name)
	if envGroupID := os.Getenv(EnvKafkaConsumerGroupID); envGroupID != "" {
		// Override consumer group id if env variable is set
		groupID = envGroupID
	}
	realtimeCfg := rollup.DefaultRealtimeConfig
	realtimeCfg.SequencerEnable = ctx.Bool(flags.RealtimeSequencerEnableFlag.Name)
	realtimeCfg.Kafka = realtimeKafka.KafkaConfig{
		BootstrapServers: strings.Split(ctx.String(flags.RealtimeKafkaSyncBootstrapServers.Name), ","),
		BlockTopic:       ctx.String(flags.RealtimeKafkaSyncBlockTopic.Name),
		ErrorTopic:       ctx.String(flags.RealtimeKafkaSyncErrorTopic.Name),
		ClientID:         ctx.String(flags.RealtimeKafkaSyncClientID.Name),
		GroupID:          groupID,
	}
	cfg.Rollup.Realtime = &realtimeCfg
}
