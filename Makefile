.PHONY: build test fmt clean

build:
	cargo build --target wasm32v1-none --release -p healthtaki-payments-escrow

test:
	cargo test

fmt:
	cargo fmt --all

clean:
	cargo clean
