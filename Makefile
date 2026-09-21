# Wrappers for the Docker backend self-test. SCHEMA stays 18.
# See docs/docker-backend-self-test.md
.PHONY: test-docker test-docker-all

TEST_FILTER ?=
CARGO_TEST_THREADS ?= 1
COMPOSE := docker compose -f docker-compose.test.yml

test-docker:
	$(COMPOSE) run --build --rm \
		-e TEST_FILTER="$(TEST_FILTER)" \
		-e CARGO_TEST_THREADS="$(CARGO_TEST_THREADS)" \
		backend-self-test

test-docker-all:
	$(COMPOSE) run --build --rm \
		-e TEST_FILTER=all \
		-e CARGO_TEST_THREADS="$(CARGO_TEST_THREADS)" \
		backend-self-test
