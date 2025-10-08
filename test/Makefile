-include .env
export

.PHONY: stop
stop:
	echo "Performing cleanup..."
	@rm -rf tmp
	@./clean.sh
	echo "Cleanup completed"

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

.PHONY: run-realtime-test
run-realtime-test:
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
	echo "Running realtime E2E tests..."; \
	MallocNanoZone=0 go test -count=1 -failfast -race -v -p 1 -timeout 1200s ./test/e2e/... ./realtime/test/...

.PHONY: test-e2e
test-e2e: stop
	@trap 'make stop' EXIT ERR INT TERM;
	@echo "OP_GETH_BRANCH: $(OP_GETH_BRANCH)"
	if [ -n "$(OP_GETH_BRANCH)" ]; then \
		echo "Running E2E tests with branch: $(OP_GETH_BRANCH)"; \
		make run && sleep 10 && make run-test; \
	else \
		echo "Running E2E tests with default branch"; \
		make run && sleep 10 && make run-test; \
	fi && echo "✅ E2E tests passed ✅"

.PHONY: test-realtime-e2e
test-realtime-e2e: stop
	@trap 'make stop' EXIT ERR INT TERM;
	@echo "OP_GETH_BRANCH: $(OP_GETH_BRANCH)"
	if [ -n "$(OP_GETH_BRANCH)" ]; then \
		echo "Running E2E tests with branch: $(OP_GETH_BRANCH)"; \
		make run && sleep 10 && make run-realtime-test; \
	else \
		echo "Running E2E tests with default branch"; \
		make run && sleep 10 && make run-realtime-test; \
	fi && echo "✅ E2E tests passed ✅"

