.PHONY: run
run:
	./0-all.sh

.PHONY: test-e2e
test-e2e: 
	./clean.sh
	make run
	sleep 3
	MallocNanoZone=0 go test -count=1 -failfast -race -v -p 1 -timeout 600s ./e2e/...
	echo "E2E tests passed"
