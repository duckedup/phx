build:
    cargo build

build-release:
    cargo build --release

run *ARGS:
    cargo run -- {{ARGS}}

run-plugins *ARGS:
    cargo run -- --plugin-dir examples/plugins {{ARGS}}

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

# Build bundled WASM plugins and copy to bundled/
build-plugins:
    cargo build -p phoenix-plugin-conductor --target wasm32-wasip2 --release
    cp target/wasm32-wasip2/release/phoenix_plugin_conductor.wasm bundled/
    @echo "Bundled plugins updated"

# Initialize beads issue tracking for this project
bd-init:
    bd init --reinit-local --prefix PHX
    git config beads.role contributor
    chmod 700 .beads
