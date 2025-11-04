#!/bin/bash

# Strict mode: exit on command failure or undefined variable
set -eu

# =============================================================================
# This script performs checks tools required in linux/mac
# =============================================================================

# Check if jq is available for JSON parsing
if ! command -v jq >/dev/null 2>&1; then
    echo "❌ jq is required but not installed. Please install jq to parse JSON config files."
    exit 1
fi

# check md5sum
if [[ "$OSTYPE" == "darwin"* ]]; then
  MD5SUM_CMD=md5
else
  MD5SUM_CMD=md5sum
fi

if [ "$ENV" = "local" ]; then
    COMPOSE_FILE=docker-compose-local.yml
else
    COMPOSE_FILE=docker-compose.yml
fi

echo "COMPOSE_FILE: ${COMPOSE_FILE}"
DOCKER_COMPOSE=$(docker compose version >/dev/null 2>&1 && echo "docker compose" || echo "docker-compose")
DOCKER_COMPOSE_CMD="${DOCKER_COMPOSE} -f ${COMPOSE_FILE}"
echo "DOCKER_COMPOSE_CMD: ${DOCKER_COMPOSE_CMD}"
