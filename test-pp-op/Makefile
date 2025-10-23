-include .env
export

ifeq ($(ENV),local)
    COMPOSE_FILE := docker-compose-local.yml
	include local.env
else
    COMPOSE_FILE := docker-compose.yml
	include testnet.env
endif

DOCKER_COMPOSE := $(shell docker compose version >/dev/null 2>&1 && echo "docker compose" || echo "docker-compose") -f ${COMPOSE_FILE}

RUN_DOCKER_MOCK_L1_NETWORK := $(DOCKER_COMPOSE) up -d xlayer-mock-l1-network

PAUSE := $(DOCKER_COMPOSE) down --remove-orphans; \
	sleep 3; \
	rm -Rf ./l1-geth/consensus/beacondata; \
	rm -Rf ./l1-geth/consensus/validatordata; \
	rm -Rf ./l1-geth/consensus/genesis.ssz; \
	rm -Rf ./l1-geth/execution/geth; \
    rm -Rf ./config-op/artifact.json; \
	rm -Rf ./config-op/31337-deploy.json; \
    rm -Rf ./config-op/genesis.json; \
	rm -Rf ./config-op/rollup.json; \
	rm -Rf ./config-op/state_dump.json; \
	rm -Rf ./l1-geth/execution/genesis.json; \
	rm -Rf ./config-op/genesis-op-raw.json; \
	rm -Rf ./config-op/superchain.json; \
	rm -Rf ./config-op/implementations.json; \


DEPLOY_ERC20_L1 := cd contracts && forge create OKBToken.sol:StandardERC20 --private-key $(SEQUENCER_PRIVATE_KEY) --rpc-url $(L1_RPC_URL) --legacy --broadcast --constructor-args "OKBToken" "OKB" 1000000000
INIT_ACCOUNT_1 := cast send -f $(SEQUENCER_ADDRESS)  --private-key $(SEQUENCER_PRIVATE_KEY) --value 1000ether $(DEPLOYER_ADDRESS) --legacy --rpc-url $(L1_RPC_URL)
INIT_ACCOUNT_2 := cast send -f $(SEQUENCER_ADDRESS)  --private-key $(SEQUENCER_PRIVATE_KEY) --value 1000ether $(AGGREGATOR_ADDRESS) --legacy --rpc-url $(L1_RPC_URL)
INIT_ACCOUNT_3 := cast send -f $(SEQUENCER_ADDRESS)  --private-key $(SEQUENCER_PRIVATE_KEY) --value 1000ether $(RICH_ADDRESS) --legacy --rpc-url $(L1_RPC_URL)
INIT_ACCOUNT_4 := cast send -f $(SEQUENCER_ADDRESS)  --private-key $(SEQUENCER_PRIVATE_KEY) --value 1000ether $(CHALLENGER_ADDRESS) --legacy --rpc-url $(L1_RPC_URL)

INIT_BRIDGE_ACCOUNT := cast send -f $(DEPLOYER_ADDRESS)  --private-key $(DEPLOYER_PRIVATE_KEY) --value 0.01ether $(SEQUENCER_ADDRESS) --legacy --rpc-url $(L2_ERIGON_RPC_URL)

.PHONY: set-env
set-env:
	@if [ "$(ENV)" = "local" ]; then \
		if [ -f local.env ]; then \
			cp local.env .env; \
			echo "Using local.env -> .env"; \
		else \
			echo "local.env not found"; \
			exit 1; \
		fi; \
	fi

.PHONY: show-env
show-env:
	@echo "Current ENV: $(ENV)"
	@echo "Using compose file: $(COMPOSE_FILE)"

.PHONY: run_erigon
run_erigon: set-env show-env
	@echo "🚀 Starting L1 PoS environment automatic dependency management..."
	$(DOCKER_COMPOSE) up -d l1-validator
	@echo "🎉 L1 PoS environment started!"
	sleep 30
	$(DEPLOY_ERC20_L1)
	$(INIT_ACCOUNT_1)
	$(INIT_ACCOUNT_2)
	$(INIT_ACCOUNT_3)
	$(INIT_ACCOUNT_4)
	sleep 5
	./init-erigon.sh
	sleep 5
	$(DOCKER_COMPOSE) up -d xlayer-bridge-db
	$(DOCKER_COMPOSE) up -d xlayer-pool-db
	sleep 5
	$(DOCKER_COMPOSE) up -d xlayer-agglayer-prover
	$(DOCKER_COMPOSE) up -d xlayer-agglayer
	sleep 5

	sleep 3
	$(DOCKER_COMPOSE) up -d xlayer-approve
	$(DOCKER_COMPOSE) up -d xlayer-seq
	$(DOCKER_COMPOSE) up -d xlayer-pool-manager

	sleep 5
	$(DOCKER_COMPOSE) up -d xlayer-rpc

	sleep 10
	$(DOCKER_COMPOSE) up -d xlayer-bridge-service
	$(DOCKER_COMPOSE) up -d xlayer-bridge-ui
	$(INIT_BRIDGE_ACCOUNT)

	$(DOCKER_COMPOSE) up -d xlayer-agg-sender

.PHONY: mainnet
mainnet:
	@echo "🚀 Starting XLayer Fake Mainnet ..."
	$(DOCKER_COMPOSE) up -d l1-validator
	sleep 20
	$(INIT_ACCOUNT_1)
	$(INIT_ACCOUNT_2)
	$(INIT_ACCOUNT_3)
	$(INIT_ACCOUNT_4)
	$(DOCKER_COMPOSE) up -d xlayer-mainnet-seq
	sleep 5
	$(DOCKER_COMPOSE) up -d xlayer-rpc

.PHONY: build-docker
build-docker: ## build images for all
	./build_images.sh --all

.PHONY: build-force
build-force: ## Force rebuild all images
	./build_images.sh --all --force

.PHONY: build-op-geth
build-op-geth: ## Build op-geth image only
	./build_images.sh --op-geth

.PHONY: build-op-stack
build-op-stack: ## Build op-stack images only
	./build_images.sh --op-stack

.PHONY: build-cdk-erigon
build-cdk-erigon:
	./build_images.sh --cdk-erigon

.PHONY: pause
pause: ## Stops all services
	$(PAUSE)

.PHONY: stop
stop: pause
	rm -rf data

.PHONY: clean
clean: stop
	rm -rf data
	rm -rf config-op/genesis.json
	rm -rf config-op/genesis.json.gz
	rm -rf config-op/implementations.json
	rm -rf config-op/intent.toml
	rm -rf config-op/rollup.json
	rm -rf config-op/state.json
	rm -rf config-op/superchain.json
	rm -rf config-op/genesis-op-before-number.json
	rm -rf config-op/genesis-op-after-number.json
	rm -rf config-op/merged.genesis.gz.json
	rm -rf config-op/196-*
	rm -rf config-op/195-*
	rm -rf config-op/1952-*
	rm -rf config/genesis.json
	rm -rf l1-geth/consensus/beacondata/
	rm -rf l1-geth/consensus/genesis.ssz
	rm -rf l1-geth/consensus/validatordata/
	rm -rf l1-geth/execution/genesis.json
	rm -rf l1-geth/execution/geth/
	rm -rf merged.genesis.json
	rm -rf migrate.log
	rm -rf .env

.PHONY: run
run: stop
	@mkdir -p tmp
	@echo "OP_GETH_BRANCH: $(OP_GETH_BRANCH)"
	@if [ -n "$(OP_GETH_BRANCH)" ]; then \
		echo "Initializing with branch: $(OP_GETH_BRANCH)"; \
		./init.sh $(OP_GETH_BRANCH); \
	else \
		echo "Initializing with default branch"; \
		./init.sh; \
	fi
	./0-all.sh

.PHONY: run-test
run-test:
	@echo "OP_GETH_LOCAL_DIRECTORY: $$OP_GETH_LOCAL_DIRECTORY"
	@echo "OP_GETH_BRANCH: $(OP_GETH_BRANCH)"
	@if [ -n "$$OP_GETH_LOCAL_DIRECTORY" ]; then \
		echo "Using custom local directory: $$OP_GETH_LOCAL_DIRECTORY"; \
		cd "$$OP_GETH_LOCAL_DIRECTORY"; \
	else \
		echo "Using default submodule: ../op-geth"; \
		cd ../op-geth; \
	fi && \
	if [ -n "$(OP_GETH_BRANCH)" ]; then \
		echo "Switching to branch: $(OP_GETH_BRANCH)"; \
		git fetch origin && git checkout "$(OP_GETH_BRANCH)" && git pull origin "$(OP_GETH_BRANCH)"; \
	else \
		echo "Using current branch"; \
	fi && \
	echo "Running E2E tests..."; \
	MallocNanoZone=0 go test -count=1 -failfast -race -v -p 1 -timeout 600s ./test/e2e/...

.PHONY: test-e2e
test-e2e: stop
	@trap 'make stop' EXIT ERR INT TERM;
	@echo "OP_GETH_BRANCH: $(OP_GETH_BRANCH)"
	if [ -n "$(OP_GETH_BRANCH)" ]; then \
		echo "Running E2E tests with branch: $(OP_GETH_BRANCH)"; \
		make run && sleep 3 && make run-test; \
	else \
		echo "Running E2E tests with default branch"; \
		make run && sleep 3 && make run-test; \
	fi && echo "✅ E2E tests passed ✅"

