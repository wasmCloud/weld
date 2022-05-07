# weld/codegen/Makefile

all: build

build clean update:
	cargo $@

release:
	cargo build --release

# Run lint check on all smithy models
lint validate:
	wash $@

test::
	cargo test --all-features --all-targets -- --nocapture

test::
	cargo clippy --all-features --all-targets
	rustup default nightly
	rustfmt --edition 2021 --check src/*.rs

.PHONY: all build release clean lint validate test update
