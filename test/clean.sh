#!/bin/bash
set -e

# Parse command line arguments
FORCE_ENV=false
while [[ $# -gt 0 ]]; do
    case $1 in
        --force-env|-f)
            FORCE_ENV=true
            shift
            ;;
        --help|-h)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --force-env, -f    Force update .env file from example.env"
            echo "  --help, -h         Show this help message"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

echo " 🧹 Cleaning up Optimism test environment..."

echo " 📦 Stopping Docker containers..."
[ -f .env ] && docker compose down

# Handle .env file
if [ "$FORCE_ENV" = true ]; then
    echo " 🔄 Force updating .env from example.env..."
    if [ -f example.env ]; then
        cp example.env .env
        echo "   ✅ .env has been force updated from example.env"
    else
        echo "   ⚠️  example.env not found, skipping .env update"
    fi
elif [ ! -f .env ]; then
    echo " 📝 .env file not found, creating from example.env..."
    if [ -f example.env ]; then
        cp example.env .env
        echo "   ✅ .env created from example.env"
    else
        echo "   ⚠️  example.env not found, please create .env manually"
    fi
else
    echo " ✓ .env file exists, keeping current configuration"
    echo "   💡 Use --force-env flag to update .env from example.env"
fi

echo " 🗑️  Removing generated files..."
rm -rf data
rm -rf config-op/genesis.json
rm -rf config-op/genesis-reth.json
rm -rf config-op/gen.test.reth.rpc.config.toml
rm -rf config-op/gen.test.geth.rpc.config.toml
rm -rf config-op/genesis.json.gz
rm -rf config-op/implementations.json
rm -rf config-op/intent.toml
rm -rf config-op/rollup.json
rm -rf config-op/state.json
rm -rf config-op/superchain.json
rm -rf config-op/195-*
rm -rf l1-geth/consensus/beacondata/
rm -rf l1-geth/consensus/genesis.ssz
rm -rf l1-geth/consensus/validatordata/
rm -rf l1-geth/execution/genesis.json
rm -rf l1-geth/execution/geth/
rm -rf init.log

echo " ✅ Cleanup completed!"
