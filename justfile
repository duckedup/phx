zig_version := `cat .zigversion`

build:
    zig build

build-release:
    zig build -Doptimize=ReleaseSafe

run *ARGS:
    zig build run -- {{ARGS}}

rpc:
    zig build run -- rpc

test:
    zig build test

fmt:
    zig fmt src/ build.zig

lint: fmt
    @echo "lint: ok"

bench:
    @echo "bench: not yet implemented"

package:
    zig build -Doptimize=ReleaseSafe
    @echo "binary at zig-out/bin/phoenix"

clean:
    rm -rf zig-out .zig-cache
