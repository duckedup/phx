# phoenix

A lightweight, fast, minimalistic agent harness.

## Contributing

### Prerequisites

- [Rust](https://rustup.rs/) (edition 2024, stable toolchain)
- [just](https://github.com/casey/just) — command runner
- The `wasm32-wasip2` target (installed automatically by `just build`)

### Setup

```bash
git clone https://codeberg.org/reddirtbytes/phoenix.git
cd phoenix
just build
```

### Common Commands

```bash
just build          # Build everything (plugins + binary)
just test           # Run all tests
just lint           # Format + clippy
just run            # Run phoenix
just run-plugins    # Run with example plugins loaded
```

### Workflow

1. Fork the repo and create a feature branch.
2. Make your changes.
3. Run `just lint` and `just test` to verify.
4. Submit a pull request.

## License

[MIT](LICENSE)
