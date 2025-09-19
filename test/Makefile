.PHONY: stop
stop:
	echo "Performing cleanup..."
	@rm -rf tmp
	@./clean.sh
	echo "Cleanup completed"

.PHONY: run
run: stop
	@mkdir -p tmp
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
	@source .env && \
	if [ -n "$(OP_GETH_BRANCH)" ]; then \
		echo "Running tests from branch: $(OP_GETH_BRANCH)"; \
		cd ./tmp/op-geth && MallocNanoZone=0 go test -count=1 -failfast -race -v -p 1 -timeout 600s ./test/e2e/...; \
	elif [ -n "$$OP_GETH_LOCAL_DIRECTORY" ]; then \
		echo "Running tests from custom local directory: $$OP_GETH_LOCAL_DIRECTORY"; \
		cd "$$OP_GETH_LOCAL_DIRECTORY" && MallocNanoZone=0 go test -count=1 -failfast -race -v -p 1 -timeout 600s ./test/e2e/...; \
	else \
		echo "Running tests from default submodule"; \
		cd ../op-geth && MallocNanoZone=0 go test -count=1 -failfast -race -v -p 1 -timeout 600s ./test/e2e/...; \
	fi

.PHONY: test-e2e
test-e2e: stop
	@trap 'make stop' EXIT ERR INT TERM; \
	if [ -n "$(OP_GETH_BRANCH)" ]; then \
		echo "Running E2E tests with branch: $(OP_GETH_BRANCH)"; \
		make run OP_GETH_BRANCH=$(OP_GETH_BRANCH) && sleep 3 && make run-test OP_GETH_BRANCH=$(OP_GETH_BRANCH); \
	else \
		echo "Running E2E tests with default branch"; \
		make run && sleep 3 && make run-test; \
	fi && echo "✅ E2E tests passed ✅"

