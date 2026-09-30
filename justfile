# seeds — work items as facts in a quipu graph (experiment)

default:
    @just --list

# Build the seeds binary
build:
    cargo build

# Run the unit tests
test:
    cargo test

# Format the code
fmt:
    cargo fmt

# Lint with warnings as errors
clippy:
    cargo clippy --all-targets -- -D warnings

# Build the mdBook (requires mdbook)
book:
    mdbook build docs/book

# Serve the book locally with live reload
book-serve:
    mdbook serve docs/book --open

# Everything CI runs
check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test
    mdbook build docs/book

# Alias for check: the local CI equivalent
ci: check
