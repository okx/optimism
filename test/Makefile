.PHONY: run
run:
	./clean.sh
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
	cd ../op-geth && MallocNanoZone=0 go test -count=1 -failfast -race -v -p 1 -timeout 600s ./test/e2e/...

.PHONY: test-e2e
test-e2e: 
	@if [ -n "$(BRANCH)" ]; then \
		make run BRANCH=$(BRANCH); \
	else \
		make run; \
	fi
	sleep 3
	make run-test
	echo "E2E tests passed"

