
.PHONY: create-temp-dir
create-temp-dir:
	@mkdir -p tmp

.PHONY: cleanup-temp-dir
cleanup-temp-dir:
	@echo "Removing temporary directory: tmp"
	@rm -rf tmp

.PHONY: clean
clean:
	@./clean.sh

.PHONY: cleanup
cleanup:
	echo "Performing cleanup..."
	@make cleanup-temp-dir >/dev/null 2>&1
	@make clean >/dev/null 2>&1 
	echo "Cleanup completed"

.PHONY: run
run:
	@if [ -n "$(BRANCH)" ]; then \
		echo "Initializing with branch: $(BRANCH)"; \
		./init.sh $(BRANCH); \
	else \
		echo "Initializing with default branch"; \
		./init.sh; \
	fi
	./0-all.sh

.PHONY: run-test
run-test:
	if [ -n "$(BRANCH)" ]; then \
		echo "Running tests from branch: $(BRANCH)"; \
		cd ./tmp/op-geth && MallocNanoZone=0 go test -count=1 -failfast -race -v -p 1 -timeout 600s ./test/e2e/...; \
	elif [ -n "$(OP_GETH_LOCAL_DIRECTORY)" ]; then \
		echo "Running tests from custom local directory: $(OP_GETH_LOCAL_DIRECTORY)"; \
		cd $(OP_GETH_LOCAL_DIRECTORY) && MallocNanoZone=0 go test -count=1 -failfast -race -v -p 1 -timeout 600s ./test/e2e/...; \
	else \
		echo "Running tests from default submodule"; \
		cd ../op-geth && MallocNanoZone=0 go test -count=1 -failfast -race -v -p 1 -timeout 600s ./test/e2e/...; \
	fi

.PHONY: test-e2e
test-e2e: clean create-temp-dir
	@trap 'make cleanup' EXIT ERR INT TERM; \
	if [ -n "$(BRANCH)" ]; then \
		echo "Running E2E tests with branch: $(BRANCH)"; \
		make run BRANCH=$(BRANCH) && sleep 3 && make run-test BRANCH=$(BRANCH); \
	else \
		echo "Running E2E tests with default branch"; \
		make run && sleep 3 && make run-test; \
	fi && echo "✅ E2E tests passed ✅"
