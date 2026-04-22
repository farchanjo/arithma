# arithma Project Guide

Project-specific instructions for working on arithma — The Ultimate LLM Calculator Engine.

---

## 📋 Language & Style

> [!IMPORTANT]
> **ENFORCE: All code, comments, commits, docs, and output MUST be in en-US English**  
> This includes variable names, function documentation, commit messages, code comments, and all written communication.

- **Code artifacts**: en-US only (no other languages in code)
- **Comments**: Clear, concise, technical English
- **Commit messages**: [Angular format](https://github.com/angular/angular/blob/main/CONTRIBUTING.md#commit) in en-US
- **Documentation**: Markdown in en-US, structured for clarity
- **Console output**: Terse, direct responses

---

## 🏗️ Architecture & Design

**arithma** is a pure-Rust MCP server with **87 expert calculator tools** across 15 categories.

### Core Principles

1. **Precision over Speed** — BigDecimal, DECIMAL128 semantics, exact transcendentals
2. **Stateless** — Each tool call is independent, no state across invocations
3. **Zero C Dependencies** — Pure Rust, portable single binary (~3 MB)
4. **Portable SIMD** — Auto-dispatch SSE2/AVX2/AVX-512/NEON at runtime
5. **Tested** — 434 unit + integration tests, all green

### Main Modules

```
src/
├── main.rs           → Binary entry, stdio transport (tokio)
├── server.rs         → MCP tool router (#[tool_router] macro)
├── engine/           → Math computation (expression, units, BigDecimal)
└── tools/            → 15 tool categories (87 total tools)
```

**No other modules.** Keep code organized and focused.

### Key Dependencies

- **rmcp 1.5** — Official Rust MCP SDK
- **tokio** — Async runtime (multi-threaded)
- **bigdecimal** + **num-bigint** — Arbitrary precision
- **astro-float** — 128-bit transcendentals
- **jiff** — IANA timezone support (no libicu)
- **wide** — Portable SIMD

---

## 📚 Documentation

**Location**: `docs/` folder  
**5 files** — complete reference aligned with README

| Document | Purpose | Link |
|:---|:---|:---|
| **INDEX.md** | Quick links, common tasks | [docs/INDEX.md](./docs/INDEX.md) |
| **ARCHITECTURE.md** | System design, module flow, decisions | [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) |
| **TOOLS.md** | Complete 87-tool reference by category | [docs/TOOLS.md](./docs/TOOLS.md) |
| **DEVELOPMENT.md** | Building, testing, contributing workflow | [docs/DEVELOPMENT.md](./docs/DEVELOPMENT.md) |
| **API.md** | MCP integration, tool calling, examples | [docs/API.md](./docs/API.md) |

Start with [docs/INDEX.md](./docs/INDEX.md) for navigation.

---

## 🔧 Development Workflow

### Building

```bash
cargo build --release              # Native CPU, fastest
RUSTFLAGS="-C target-cpu=x86-64-v3" cargo build --profile release-portable  # Portable
```

Binary: `./target/release/arithma`

### Testing

```bash
cargo test --lib                   # Unit tests (349)
python3 scripts/test_stdio.py     # Integration tests (87 tools)
cargo test --all                   # Full suite
```

All tests must pass before committing.

### Code Quality

```bash
cargo fmt --check                  # Format check
cargo clippy --all-targets -- -D warnings  # Lint (deny mode)
cargo test --lib                   # Unit tests
python3 scripts/test_stdio.py     # Integration tests
```

Run all four before `git commit`.

### Commit Format

```
<type>(<scope>): <subject>

<body (optional)>

Co-Authored-By: Claude Haiku 4.5 <noreply@anthropic.com>
```

**Types**: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `perf`  
**Scope**: Tool name or module (e.g., `unit_converter`, `core`)  
**Subject**: Imperative, lowercase, no period

**Example**:
```
fix(unit_converter): handle temperature precision in Celsius pivot

Use DECIMAL128 context for Celsius intermediate to prevent rounding
errors in Fahrenheit↔Kelvin conversions.

Fixes #42
```

---

## 🎯 Code Conventions

### Naming

- **Functions**: snake_case (`convert_units`, `evaluate_expression`)
- **Types**: PascalCase (`UnitCategory`, `UnitError`)
- **Constants**: SCREAMING_SNAKE_CASE (`FACTOR_SCALE`, `DECIMAL128_PRECISION`)
- **Modules**: lowercase (`engine`, `tools`, `basic`)

### Tool Function Signature

All tools return `String` (MCP requirement, errors embedded):

```rust
pub fn tool_name(param1: String, param2: i32) -> String {
    match compute(param1, param2) {
        Ok(result) => format!("{}", result),
        Err(e) => format!("Error: {}", e),
    }
}
```

### Error Handling

- **Never panic** — Return `Result` internally, convert to `"Error: ..."` String for MCP boundary
- **No exceptions cross MCP** — All errors embedded as string results
- **Actionable messages** — Errors should help the LLM or user fix the input

### Module Documentation

Every module has a doc comment explaining purpose and invariants:

```rust
//! Unit conversion registry — 21 categories, 118 units.
//!
//! Arithmetic uses DECIMAL128 semantics (34 significant digits)
//! with HALF_UP rounding. Temperature uses formula routing through
//! Celsius. Gas mark uses fixed lookup table.
```

### Comments

- **No unnecessary comments** — Code should be self-explanatory
- **WHY over WHAT** — Only explain non-obvious logic or constraints
- **No stale comments** — Update or delete if code changes

---

## 📏 Design Rules

### DO ✅

- Use BigDecimal for precision-critical paths (finance, unit conversion, basic arithmetic)
- Cache lookup tables in `LazyLock` (built once, reused)
- Test edge cases (0, negative, extreme values, precision boundaries)
- Document public functions with doc comments
- Keep functions < 30 lines (readability threshold)
- Use DECIMAL128 context for all multiplication/division in units

### DON'T ❌

- Use f64/f32 for financial or unit calculations (use BigDecimal)
- Allocate memory in tight loops (pre-allocate or cache)
- Mix precision contexts (DECIMAL128 vs. f64 in same calculation)
- Add features beyond the 87 tools without discussion
- Commit code that doesn't compile or fails tests
- Use other languages in code/comments (en-US only)

---

## 🧪 Testing Strategy

### Unit Tests (349 total)

- **Location**: Inline in source files, `#[cfg(test)]` modules
- **Coverage**: Each public function has ≥1 test
- **Edge cases**: 0, negative, boundaries, precision limits
- **Run**: `cargo test --lib`

### Integration Tests (87 tools)

- **Location**: `scripts/test_stdio.py`
- **Method**: Full JSON-RPC round trips via stdio
- **Coverage**: Every tool invoked with valid inputs
- **Run**: `python3 scripts/test_stdio.py`

### Before Shipping

```bash
cargo test --lib && \
  python3 scripts/test_stdio.py && \
  cargo clippy --all-targets -- -D warnings && \
  cargo fmt --check
```

If any step fails, fix before committing.

---

## 🚀 Release & Deployment

### Binary Distribution

- **Target**: `release-portable` (x86-64-v3, Haswell+)
- **Size**: ~3 MB (fully static, no runtime deps)
- **Platforms**: Linux, macOS, Windows (cross-compile with `cargo-cross`)

### Versioning

Use [Semantic Versioning](https://semver.org/):
- **MAJOR**: Breaking API changes (tool removed, parameter changed)
- **MINOR**: New tools or backward-compatible features
- **PATCH**: Bug fixes

Update `Cargo.toml` and tag releases on GitHub.

---

## 🔄 MCP Integration

**Clients**: Claude Code, Claude Desktop, Cursor, Windsurf, OpenCode  
**Protocol**: JSON-RPC 2.0 over stdio  
**Tools**: 87 total (see [Tools Catalog](./docs/TOOLS.md))  
**Parameters**: Decimal numbers as strings (preserve precision)

See [API Usage](./docs/API.md) for integration examples.

---

## 🛠️ Common Tasks

| Task | Command | Notes |
|:---|:---|:---|
| Build | `cargo build --release` | Binary in `target/release/arithma` |
| Test | `cargo test --lib && python3 scripts/test_stdio.py` | All tests must pass |
| Format | `cargo fmt` | Required before commit |
| Lint | `cargo clippy --all-targets -- -D warnings` | Deny mode |
| Add tool | Edit `src/tools/mod.rs` + add function | Register in server.rs #[tool_router] |
| Add unit | Edit `src/engine/unit_registry.rs` | Update ALL_UNITS, UnitCategory |
| Debug | `RUST_LOG=debug cargo run --release` | Logs to stderr |

---

## 📖 Useful References

- [README.md](./README.md) — Project overview, badges, quick start
- [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) — Module design, data flow
- [docs/TOOLS.md](./docs/TOOLS.md) — Complete tool reference
- [docs/DEVELOPMENT.md](./docs/DEVELOPMENT.md) — Building, testing, contributing
- [docs/API.md](./docs/API.md) — MCP integration examples
- [Cargo.toml](./Cargo.toml) — Dependencies, build profiles
- [rust-toolchain.toml](./rust-toolchain.toml) — Rust version requirement

---

## 📞 Questions?

- **Architecture**: See [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md)
- **Specific tool**: See [docs/TOOLS.md](./docs/TOOLS.md)
- **How to integrate**: See [docs/API.md](./docs/API.md)
- **Contributing**: See [docs/DEVELOPMENT.md](./docs/DEVELOPMENT.md)

---

**Last Updated**: 2026-04-22  
**Language**: en-US (enforced)  
**Repository**: [github.com/farchanjo/arithma](https://github.com/farchanjo/arithma)
