.PHONY: help build test fmt fmt-check lint check clean sim summary snapshots snapshots-accept

# Default sim parameters — override on the CLI, e.g. `make sim KILLS=10000 LEVEL=60`
LEVEL ?= 30
KILLS ?= 1000
SEED  ?= 1

help:
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}'

build: ## cargo build --workspace
	cargo build --workspace

test: ## cargo test --workspace
	cargo test --workspace

fmt: ## cargo fmt
	cargo fmt --all

fmt-check: ## cargo fmt --check
	cargo fmt --all -- --check

lint: ## cargo clippy with warnings as errors
	cargo clippy --workspace --all-targets -- -D warnings

check: fmt-check lint test ## fmt-check + lint + test

clean: ## cargo clean
	cargo clean

sim: ## run loot sim, CSV to stdout (vars: LEVEL, KILLS, SEED)
	cargo run -q -p head2box-sim -- --monster-level $(LEVEL) --kills $(KILLS) --seed $(SEED)

summary: ## run loot sim with --summary (vars: LEVEL, KILLS, SEED)
	cargo run -q -p head2box-sim -- --monster-level $(LEVEL) --kills $(KILLS) --seed $(SEED) --summary

snapshots: ## review pending insta snapshots
	cargo insta review

snapshots-accept: ## accept all pending insta snapshots
	cargo insta accept
