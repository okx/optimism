-include .env
export

TEST_DIR := $(shell cd ../test && pwd)

.PHONY: stop
stop:
	@echo "Performing cleanup..."
	@rm -rf tmp
	@cd ../test && ./clean.sh
	@docker compose -f $(CURDIR)/docker-compose-apollo.yml down
	@echo "Cleanup completed"

.PHONY: run  
run:
	@ln -sf ../test/data data 2>/dev/null || true
	@echo "OP_GETH_BRANCH: $(OP_GETH_BRANCH)"
	@if [ -n "$(OP_GETH_BRANCH)" ]; then \
		echo "Initializing with branch: $(OP_GETH_BRANCH)"; \
		cd ../test && ./init.sh $(OP_GETH_BRANCH); \
	else \
		echo "Initializing with default branch"; \
		cd ../test && ./init.sh; \
	fi
	cd ../test && COMPOSE_FILE=$(CURDIR)/docker-compose-apollo.yml APOLLO_ENABLED=true ./0-all.sh

.PHONY: apollo
apollo: stop
	@trap 'make stop' EXIT ERR INT TERM;
	@echo "Starting with Apollo enabled..."
	docker compose -f $(CURDIR)/docker-compose-apollo.yml up -d mysql
	sleep 10
	docker compose -f $(CURDIR)/docker-compose-apollo.yml up -d apollo-configservice
	docker compose -f $(CURDIR)/docker-compose-apollo.yml up -d apollo-adminservice
	sleep 5
	@echo "Starting Apollo portal..."
	docker compose -f $(CURDIR)/docker-compose-apollo.yml up -d apollo-portal
	@APOLLO_ENABLED=true $(MAKE) run