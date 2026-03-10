set shell := ["bash", "-euo", "pipefail", "-c"]

default: verify

verify:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets --all-features -- -D warnings
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
	cargo test --workspace --all-targets --all-features
