# Wrappers for the Docker backend self-test. SCHEMA stays 18.
# Default: FULL src-tauri cargo test + markdown report.
# See docs/docker-backend-self-test.md
.PHONY: test-docker test-docker-all test-docker-proxy test-docker-usage test-docker-pi-crud test-docker-lite

CARGO_TEST_THREADS ?= 1
SCRIPT := bash scripts/docker-self-test.sh

# Default = full suite (TEST_FILTER=all). Narrower filters are explicit.
test-docker:
	CARGO_TEST_THREADS="$(CARGO_TEST_THREADS)" $(SCRIPT)

test-docker-all:
	TEST_FILTER=all CARGO_TEST_THREADS="$(CARGO_TEST_THREADS)" $(SCRIPT)

test-docker-proxy:
	TEST_FILTER=proxy_projection_linux CARGO_TEST_THREADS="$(CARGO_TEST_THREADS)" $(SCRIPT)

test-docker-usage:
	TEST_FILTER=session_usage_scan CARGO_TEST_THREADS="$(CARGO_TEST_THREADS)" $(SCRIPT)

test-docker-pi-crud:
	TEST_FILTER=pi-crud CARGO_TEST_THREADS="$(CARGO_TEST_THREADS)" $(SCRIPT)

test-docker-lite:
	TEST_FILTER=lib-lite CARGO_TEST_THREADS="$(CARGO_TEST_THREADS)" $(SCRIPT)
