#!/bin/bash

# Check which containers are not running and start them
STARTED_COUNT=0
for i in {1..3}; do
    CONTAINER="op-seq"
    [ "$i" != "1" ] && CONTAINER="${CONTAINER}${i}"

    if ! docker ps --format "{{.Names}}" | grep -q "^${CONTAINER}$"; then
        echo "Starting $CONTAINER..."
        docker compose up -d --remove-orphans $CONTAINER
        STARTED_COUNT=$((STARTED_COUNT + 1))
    fi
done

if [ "$STARTED_COUNT" -eq 0 ]; then
    echo "All op-seq containers are already running"
    exit 0
fi

# Final check
sleep 2
for i in {1..3}; do
    CONTAINER="op-seq"
    [ "$i" != "1" ] && CONTAINER="${CONTAINER}${i}"

    if ! docker ps --format "{{.Names}}" | grep -q "^${CONTAINER}$"; then
        echo "Warning: $CONTAINER failed to start"
    fi
done
