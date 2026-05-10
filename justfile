build: build-plugins
    @cargo build

build-release: build-plugins
    cargo build --release

run *ARGS:
    cargo run -- {{ARGS}}

run-plugins *ARGS:
    cargo run -- --plugin-dir examples/plugins {{ARGS}}

rpc:
    cargo run -- rpc

test: build-plugins
    cargo test --all-features

test-ignored:
    cargo test --all-features -- --ignored

fmt:
    cargo fmt

clippy: build-plugins
    cargo clippy --all-targets --all-features -- -D warnings

lint: fmt clippy

bench:
    @echo "bench: not yet implemented"

package:
    cargo build --release
    @echo "binary at target/release/phoenix"

clean:
    cargo clean

# Ensure wasm32-wasip2 target is installed
_ensure-wasm-target:
    @rustup target list --installed | grep -q wasm32-wasip2 || rustup target add wasm32-wasip2

# Build all bundled WASM plugins and copy to bundled/
build-plugins: _ensure-wasm-target
    #!/usr/bin/env bash
    set -euo pipefail
    for manifest in plugins/*/Cargo.toml; do
        name=$(grep '^name' "$manifest" | head -1 | sed 's/.*"\(.*\)".*/\1/')
        cargo build -p "$name" --target wasm32-wasip2 --release
        wasm="target/wasm32-wasip2/release/${name//-/_}.wasm"
        cp "$wasm" bundled/
    done

# Initialize beads issue tracking for this project
bd-init:
    bd init --reinit-local --prefix PHX
    git config beads.role contributor
    chmod 700 .beads
