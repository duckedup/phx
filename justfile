build:
    cargo build

build-release:
    cargo build --release

run *ARGS:
    cargo run -- {{ARGS}}

rpc:
    cargo run -- rpc

test:
    cargo test --all-features

test-ignored:
    cargo test --all-features -- --ignored

fmt:
    cargo fmt

clippy:
    cargo clippy --all-targets --all-features -- -D warnings

lint: fmt clippy

bench:
    @echo "bench: not yet implemented"

package:
    cargo build --release
    @echo "binary at target/release/phoenix"

clean:
    cargo clean
