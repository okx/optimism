
.PHONY: create-temp-dir
create-temp-dir:
	mkdir -p tmp

.PHONY: cleanup-temp-dir
cleanup-temp-dir:
	echo "Cleaning up temporary directory: tmp"; \
	rm -rf tmp

.PHONY: clean
clean:
	./clean.sh

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
	@if [ -n "$(BRANCH)" ]; then \
		echo "Running tests from branch: $(BRANCH)"; \
		cd ./tmp/op-geth && MallocNanoZone=0 go test -count=1 -failfast -race -v -p 1 -timeout 600s ./test/e2e/...; \
	else \
		echo "Running tests from default branch"; \
		cd ../op-geth && MallocNanoZone=0 go test -count=1 -failfast -race -v -p 1 -timeout 600s ./test/e2e/...; \
	fi

.PHONY: test-e2e
test-e2e: 
	@set -e; \
	cleanup() { \
		echo "Performing cleanup..."; \
		make cleanup-temp-dir; \
		make clean; \
	}; \
	trap cleanup EXIT ERR INT TERM; \
	if [ -n "$(BRANCH)" ]; then \
		echo "Running E2E tests with branch: $(BRANCH)"; \
		make create-temp-dir && \
		make clean && \
		make run BRANCH=$(BRANCH) && \
		sleep 3 && \
		make run-test BRANCH=$(BRANCH); \
		echo "E2E tests passed"; \
	else \
		echo "Running E2E tests with default branch"; \
		make clean && \
		make run && \
		sleep 3 && \
		make run-test; \
		echo "E2E tests passed"; \
	fi; \
