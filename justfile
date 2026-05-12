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

# Build native plugins and install to .phoenix/plugins/
# Usage: just build-plugins [folder]
# Examples:
#   just build-plugins              # builds from plugins/
#   just build-plugins examples/plugins  # builds from examples/plugins/
build-plugins dir="plugins":
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    for manifest in {{dir}}/*/Cargo.toml; do
        name=$(grep '^name' "$manifest" | head -1 | sed 's/.*"\(.*\)".*/\1/')
        cargo build -p "$name" --release
        short_name="${name#phoenix-plugin-}"
        ./target/release/"$name" install .phoenix/plugins/"$short_name"
    done

# Initialize beads issue tracking for this project
bd-init:
    bd init --reinit-local --prefix PHX
    git config beads.role contributor
    chmod 700 .beads
