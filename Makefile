# Number of fake bulbs `make mock` starts.
N ?= 1

.PHONY: help build test clippy fmt doc mock gui discover

help:           ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-10s\033[0m %s\n", $$1, $$2}'

build:          ## Build the workspace
	cargo build

test:           ## Run unit tests (no device/network needed)
	cargo test

clippy:         ## Lint all targets
	cargo clippy --all-targets

fmt:            ## Format the workspace
	cargo fmt

doc:            ## Build and open the API docs
	cargo doc --open

mock:           ## Run N fake bulbs for dev, e.g. make mock N=3
	cargo run -p yeelight-mock -- $(N)

gui:            ## Run the desktop GUI
	cargo run -p yeelight-gui

discover:       ## Run the discover example (finds bulbs, including mocks)
	cargo run -p yeelight-core --example discover
