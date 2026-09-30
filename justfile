# seeds — work items as facts in a quipu graph (experiment)

default:
    @just --list

# Build the sd binary
build:
    cargo build

# Run the tests (unit, core, CLI end-to-end, JSON schema)
test:
    cargo test

# Format the code
fmt:
    cargo fmt

# Lint with warnings as errors (clippy.toml also keeps the core wasm-clean)
clippy:
    cargo clippy --all-targets -- -D warnings

# Build the mdBook (requires mdbook)
book:
    mdbook build docs/book

# Serve the book locally with live reload
book-serve:
    mdbook serve docs/book --open

# Build the core for wasm32 (no default features: no CLI, no SHACL), lint it,
# and build the create -> dep -> ready demo as a linked module to report its size
wasm:
    cargo build --target wasm32-unknown-unknown --lib --no-default-features --release
    cargo clippy --target wasm32-unknown-unknown --no-default-features --all-targets -- -D warnings
    cargo build --target wasm32-unknown-unknown --no-default-features --release --example ready_demo
    @ls -l "$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)/wasm32-unknown-unknown/release/examples/ready_demo.wasm" | awk '{print "ready_demo.wasm: " $5 " bytes"}'

# Run the core test suite on wasm32 under node (needs wasm-bindgen-test-runner
# at the wasm-bindgen version in Cargo.lock; see .cargo/config.toml)
wasm-test:
    cargo test --target wasm32-unknown-unknown --no-default-features --test core

# Everything the native CI job runs
check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test
    cargo test --no-default-features
    bash scripts/ci/crates-publish-guard.sh --selftest
    mdbook build docs/book

# The local CI equivalent: the native checks plus the wasm job
ci: check wasm wasm-test

# The storage-mode tests against a real quipu server:
#   SEEDS_TEST_QUIPU_SERVER=/path/to/quipu-server just remote-test
remote-test:
    test -x "${SEEDS_TEST_QUIPU_SERVER:?set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary}"
    cargo test --test modes -- --nocapture
