# Development Guide

Build, test, lint, and contribute to arithma.

## Prerequisites

- **Rust 1.94+** (pinned in [`rust-toolchain.toml`](../rust-toolchain.toml))
- **Cargo** (ships with Rust)
- **Python 3.9+** (only for the stdio integration tests)

## Building

```bash
git clone https://github.com/farchanjo/arithma.git
cd arithma
cargo build --release
# → ./target/release/arithma (~3 MB, fully static)
```

### Profiles

| Profile | Command | Notes |
|:---|:---|:---|
| Native | `cargo build --release` | Fastest; uses `target-cpu=native` via `.cargo/config.toml`. |
| Portable | `RUSTFLAGS="-C target-cpu=x86-64-v3" cargo build --profile release-portable` | Haswell+ / AVX2, redistributable. |
| Dev | `cargo build` | Debug symbols, incremental. |

### Extra slimming (optional)

```bash
RUSTFLAGS="-C lto=fat -C codegen-units=1 -C strip=symbols" cargo build --release
```

## Testing

| Scope | Command | Coverage |
|:---|:---|:---|
| Unit | `cargo test --lib` | 349 tests — parsers, units, helpers, each tool. |
| Stdio integration | `python3 scripts/test_stdio.py` | All 87 tools via full JSON-RPC round trips. |
| Full suite | `cargo test --all` | Unit + doctests. |
| Single tool module | `cargo test --lib unit_converter::tests` | |
| Single test | `cargo test --lib convert_length` | |

The full suite runs in under a second; there is no excuse for merging red tests.

## Lint & format

```bash
cargo fmt --check                               # format check
cargo fmt                                       # auto-fix
cargo clippy --all-targets --all-features -- -D warnings
```

`Cargo.toml` enables `deny` on the full `clippy::all`, `clippy::pedantic`, and `clippy::nursery` sets.

### Pre-commit sequence

```bash
cargo fmt                                       \
  && cargo clippy --all-targets -- -D warnings  \
  && cargo test --lib                           \
  && python3 scripts/test_stdio.py
```

Everything must pass. If a step fails, fix the root cause — do not bypass.

> **PMD / rulesets**: never modify linting rulesets to silence a failure. Fix the code instead.

## Project structure

```
.
├── Cargo.toml                     Dependencies, lint + release profiles
├── rust-toolchain.toml            Rust 1.94+ pin
├── clippy.toml                    Clippy tweaks
├── .cargo/config.toml             Cargo aliases, target-cpu=native
├── src/
│   ├── main.rs                    Binary entry, stdio transport
│   ├── lib.rs                     Library exports
│   ├── server.rs                  #[tool_router] — all 87 tools
│   ├── engine/
│   │   ├── expression.rs          Parser + f64 evaluator
│   │   ├── expression_exact.rs    Parser + 128-bit evaluator
│   │   ├── unit_registry.rs       21 categories, 118 units
│   │   └── bigdecimal_ext.rs      DECIMAL128 context, formatters
│   ├── mcp/                       MCP message helpers
│   └── tools/                     15 category modules
│       ├── basic.rs
│       ├── scientific.rs
│       ├── programmable.rs
│       ├── vector.rs
│       ├── financial.rs
│       ├── calculus.rs
│       ├── unit_converter.rs
│       ├── cooking.rs
│       ├── measure_reference.rs
│       ├── datetime.rs
│       ├── printing.rs
│       ├── graphing.rs
│       ├── network.rs
│       ├── analog_electronics.rs
│       └── digital_electronics.rs
├── scripts/test_stdio.py          Integration test (87 tools)
├── docs/                          INDEX · ARCHITECTURE · TOOLS · DEVELOPMENT · API
└── target/release/arithma         Final binary
```

## Code conventions

### Naming

| Kind | Style | Example |
|:---|:---|:---|
| Crates | `snake_case` | `arithma` (bin), `math_calc` (lib) |
| Modules | lowercase | `basic`, `unit_registry` |
| Functions | `snake_case` | `convert_units` |
| Types | `PascalCase` | `UnitCategory`, `UnitError` |
| Constants | `SCREAMING_SNAKE_CASE` | `FACTOR_SCALE` |

### Tool function signature

Every tool returns a `String` built through the shared response builder in `src/mcp/message/builder.rs`:

```rust
use crate::mcp::message::{Response, ErrorCode, error_with_detail};

pub fn tool_name(param1: String, param2: i32) -> String {
    match compute(&param1, param2) {
        Ok(result) => Response::ok("TOOL_NAME").result(result.to_string()).build(),
        // → "TOOL_NAME: OK | RESULT: <value>"

        Err(ComputeError::BadUnit(u)) => error_with_detail(
            "TOOL_NAME",
            ErrorCode::InvalidInput,
            "unit is not a recognized unit",
            &format!("unit={u}"),
        ),
        // → "TOOL_NAME: ERROR\nREASON: [INVALID_INPUT] unit is not a recognized unit\nDETAIL: unit=<u>"
    }
}
```

**Rules of the envelope:**

- Tool and key names are `SCREAMING_SNAKE_CASE`.
- Scalar success: `.result(value)` → `TOOL: OK | RESULT: value`. Prefer this over `.field("RESULT", value)`.
- Multi-field success: chain `.field(key, value)` calls.
- Tabular payloads: opt in with `.block()` and emit repeated keys like `ROW_1`, `ROW_2`, ….
- Errors: use one of the canonical `ErrorCode` variants. Add a `DETAIL` line when it helps the caller diagnose (unit name, received value, etc.).

### Rules of thumb

- **Never panic.** Return `Result<T, E>` internally and route the failure through `mcp::message::error` / `error_with_detail` — never hand-roll an error string.
- **Methods under 30 lines.** Extract helpers when they grow.
- **No dead code, no duplication.** Clippy enforces most of this.
- **Document the WHY, not the WHAT.** If a comment restates the code, delete it.
- **Cache lookup tables with `LazyLock`** — never rebuild on each call.
- **Don't mix precision contexts.** Stay inside DECIMAL128, or stay inside `f64`. Choose per path.
- **No f64/f32 in financial or unit paths.** Use `BigDecimal`.

### Adding a dependency

Before adding anything:

1. Pure Rust only (no C FFI) — keeps the binary portable and the build trivial.
2. License-compatible with Apache-2.0.
3. Test compilation on macOS, Linux, and Windows.
4. If it changes architecture, update [`ARCHITECTURE.md`](./ARCHITECTURE.md).

```bash
cargo add <crate> --vers ^X.Y.Z
```

## Debugging

### Structured logging

The server uses `tracing` and logs to **stderr** (never stdout — stdio transport requires a clean stdout).

```bash
RUST_LOG=debug           ./target/release/arithma
RUST_LOG=math_calc=trace ./target/release/arithma
```

### Manual tool invocation

```bash
(echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}';
 echo '{"jsonrpc":"2.0","method":"notifications/initialized"}';
 echo '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"add","arguments":{"first":"0.1","second":"0.2"}}}';
 sleep 0.1) | ./target/release/arithma
```

## Contributing

### Workflow

1. Fork the repo.
2. Branch off `main`: `git checkout -b fix/short-description`.
3. Make the change; run the pre-commit sequence above.
4. Commit using the [Angular format](https://github.com/angular/angular/blob/main/CONTRIBUTING.md#commit): `<type>(<scope>): <subject>`.
5. Push and open a PR.

### Commit template

```
<type>(<scope>): <subject>

<body — why, not what>

<footer — issue refs, breaking changes>
```

**Types**: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `perf`, `style`.

### Checklist for reviewers

- Formatted (`cargo fmt`).
- Lint-clean (`cargo clippy -- -D warnings`).
- All tests pass (unit + stdio integration).
- No new panics; errors flow through `mcp::message::error*` with a canonical `ErrorCode`.
- Docs updated when behavior or interfaces change.
- en-US only.

---

See [ARCHITECTURE.md](./ARCHITECTURE.md) for system design and [TOOLS.md](./TOOLS.md) for per-tool specs.
