---
paths:
  - "src/**/*.rs"
---

# Rust Coding Rules

- No magic, just code
- Explicit is better than implicit
- Avoid unsafe code
- Things that need not change shouldn't: Immutability is the default to ensure safety and clarity.
- Always format the code with the following cmd: `just fmt`
- Always make sure clippy passes with no errors or warnings with the following cmd: `just clippy`
