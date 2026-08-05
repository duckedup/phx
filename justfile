build:
    @cargo build

build-release:
    cargo build --release

run *ARGS:
    cargo run -- {{ARGS}}

test:
    cargo test --all-features

test-ignored:
    cargo test --all-features -- --ignored

fmt:
    cargo fmt

clippy:
    cargo clippy --all-targets --all-features -- -D warnings

lint: fmt clippy

check: lint build test lockfile

lockfile:
    cargo check --locked

package:
    cargo build --release
    @echo "binary at target/release/phx"

clean:
    cargo clean
