.PHONY: help build test fmt fmt-check lint check clean dbg arena arena-empty sim summary snapshots snapshots-accept web serve web-serve wasm-target

# Default sim parameters — override on the CLI, e.g. `make sim KILLS=10000 LEVEL=60`
LEVEL ?= 30
KILLS ?= 1000
SEED  ?= 1

# Web (wasm) build.
GAME_CRATE := head2box-game
WASM       := target/wasm32-unknown-unknown/release/h2b-game.wasm
PORT       ?= 8000

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

# Native debug-overlay runs (mirror the cargo aliases in .cargo/config.toml).
dbg: ## run the game with the debug overlay, BSP dungeon (= cargo dbg)
	cargo run -p $(GAME_CRATE) --features debug

arena: ## run in the pillar test arena, auto-spawn off (= cargo arena)
	cargo run -p $(GAME_CRATE) --features debug -- arena

arena-empty: ## run in the empty arena, no pillars (= cargo arena-empty)
	cargo run -p $(GAME_CRATE) --features debug -- arena-empty

sim: ## run loot sim, CSV to stdout (vars: LEVEL, KILLS, SEED)
	cargo run -q -p head2box-sim -- --monster-level $(LEVEL) --kills $(KILLS) --seed $(SEED)

summary: ## run loot sim with --summary (vars: LEVEL, KILLS, SEED)
	cargo run -q -p head2box-sim -- --monster-level $(LEVEL) --kills $(KILLS) --seed $(SEED) --summary

snapshots: ## review pending insta snapshots
	cargo insta review

snapshots-accept: ## accept all pending insta snapshots
	cargo insta accept

web: wasm-target ## build the browser (wasm) bundle → web/head2box.wasm
	cargo build -p $(GAME_CRATE) --target wasm32-unknown-unknown --release
	cp $(WASM) web/head2box.wasm
	@echo "built web/head2box.wasm ($$(du -h web/head2box.wasm | cut -f1))"

serve: ## serve web/ on $(PORT) without rebuilding (vars: PORT)
	@echo "serving web/ → http://localhost:$(PORT)"
	cd web && python3 -m http.server $(PORT)

web-serve: web serve ## build the wasm bundle, then serve it

# Install the wasm target on first use; a no-op once present.
wasm-target:
	@rustup target list --installed | grep -q wasm32-unknown-unknown \
		|| rustup target add wasm32-unknown-unknown
