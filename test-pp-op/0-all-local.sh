#!/bin/bash
set -e

# ./build_images.sh --all # build all images. add --force if want to force rebuild
make clean
cp local.env .env
./1-start-erigon.sh
./2-deploy-op-contracts.sh
./3-stop-erigon.sh
./4-migrate-op.sh
./5-start-op.sh
./6-build-op-program.sh
./7-setup-fraud-proof.sh
