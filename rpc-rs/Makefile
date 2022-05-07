# weld/rpc-rs/Makefile

all: build

build clean update:
	cargo $@

release:
	cargo build --release

# Run lint check on all smithy models
lint validate:
	wash $@

test::
	@-docker rm -f wasmbus_rpc_test_19222 2>/dev/null
	@docker run -d --rm --name wasmbus_rpc_test_19222 -p 127.0.0.1:19222:4222 nats:2.8
	NATS_URL=nats://127.0.0.1:19222 cargo test --all-features --all-targets -- --nocapture
	@docker rm -f wasmbus_rpc_test_19222

rust-check::
	cargo clippy --all-features --all-targets
	rustfmt --edition 2021 --check src/*.rs

.PHONY: all build release clean lint validate test update rust-check
