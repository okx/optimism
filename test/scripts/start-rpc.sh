#!/bin/bash

set -e

source .env

set -x

docker compose up -d op-rpc
