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

check: lint build test

bench:
    @echo "bench: not yet implemented"

package:
    cargo build --release
    @echo "binary at target/release/phx"

publish:
    cargo publish -p phx

clean:
    cargo clean

# Build native plugins and install to .phx/plugins/
# Scans both plugins/ and examples/plugins/ for Cargo and manifest-only plugins.
build-plugins:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    for dir in plugins examples/plugins; do
        [ -d "$dir" ] || continue
        # Build Cargo-based plugins
        for manifest in "$dir"/*/Cargo.toml; do
            name=$(grep '^name' "$manifest" | head -1 | sed 's/.*"\(.*\)".*/\1/')
            cargo build -p "$name" --release
            short_name="${name#phx-plugin-}"
            ./target/release/"$name" install .phx/plugins/"$short_name"
        done
        # Copy manifest-only plugins (no Cargo.toml, just manifest.json)
        for plugin_dir in "$dir"/*/; do
            [ -f "$plugin_dir/manifest.json" ] && [ ! -f "$plugin_dir/Cargo.toml" ] || continue
            name=$(basename "$plugin_dir")
            mkdir -p .phx/plugins/"$name"
            cp "$plugin_dir"* .phx/plugins/"$name"/
        done
    done

# Initialize beads issue tracking for this project
bd-init:
    bd init --reinit-local --prefix PHX
    git config beads.role contributor
    chmod 700 .beads
