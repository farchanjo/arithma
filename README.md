# math-calc-mcp

[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![MCP](https://img.shields.io/badge/MCP-stdio-green.svg)](https://modelcontextprotocol.io)

**Pure-Rust [Model Context Protocol](https://modelcontextprotocol.io) server exposing 85 math, engineering, and conversion tools over stdio.** A full-featured port of the Spring AI [math-calculator](https://github.com/farchanjo/math-calculator) from Java 25 to Rust — same behavior, same precision, single static binary, cross-platform.

## Highlights

- **100% Rust, zero C dependencies** — one `~3 MB` statically-linked binary for Linux, macOS, and Windows
- **Arbitrary-precision arithmetic** via [`bigdecimal`](https://crates.io/crates/bigdecimal) + [`num-bigint`](https://crates.io/crates/num-bigint), matching Java `BigDecimal`/`BigInteger` semantics (DECIMAL128, HALF_UP)
- **Correctly-rounded transcendentals** via [`astro-float`](https://crates.io/crates/astro-float) (drop-in replacement for Java `StrictMath`)
- **IANA-aware datetime** via [`jiff`](https://crates.io/crates/jiff) (no `libicu`, no C deps)
- **Portable SIMD** via [`wide`](https://crates.io/crates/wide) — auto-dispatches SSE2/AVX2/AVX-512/NEON at runtime
- **CPU-specific optimization** (`target-cpu=native`) with a portable `release-portable` profile for distribution
- **349 unit tests** + **85-tool stdio integration suite** — all green, sub-second

## Table of contents

- [Install](#install)
- [Build from source](#build-from-source)
- [Wire into an MCP client](#wire-into-an-mcp-client)
- [Tool matrix (85 tools)](#tool-matrix-85-tools)
- [Examples](#examples)
- [Architecture](#architecture)
- [Precision & numerical parity with Java](#precision--numerical-parity-with-java)
- [Development](#development)
- [License](#license)

## Install

Binary releases will be published via GitHub Releases. Until then, build from source:

```bash
git clone https://github.com/farchanjo/math-calc-mcp.git
cd math-calc-mcp
cargo build --release
# Binary lives at: ./target/release/math-calc-mcp
```

Minimum Rust: **1.94** (pinned via `rust-toolchain.toml`).

## Build from source

```bash
# Native CPU — fastest on the machine that compiles it (default release profile
# uses RUSTFLAGS="-C target-cpu=native" via .cargo/config.toml)
cargo build --release

# Portable build — targets x86-64-v3 (Haswell+; AVX2 available)
RUSTFLAGS="-C target-cpu=x86-64-v3" cargo build --profile release-portable

# Run the stdio server (sends JSON-RPC over stdin/stdout)
cargo run --release --bin math-calc-mcp
# Or directly:
./target/release/math-calc-mcp
```

## Wire into an MCP client

### Claude Code

```bash
claude mcp add math-calc -- /absolute/path/to/target/release/math-calc-mcp
```

### Claude Desktop / generic `mcp.json`

```json
{
  "mcpServers": {
    "math-calc": {
      "command": "/absolute/path/to/target/release/math-calc-mcp"
    }
  }
}
```

### Cursor / Windsurf / OpenCode

All accept the same stdio command. Point them at the absolute path of the binary.

### Verify the handshake

```bash
(printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}\n';
 printf '{"jsonrpc":"2.0","method":"notifications/initialized"}\n';
 printf '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}\n';
 sleep 0.3) | ./target/release/math-calc-mcp 2>/dev/null | head -c 500
```

You should see an `initialize` response followed by a `tools/list` response containing all 85 tools.

## Tool matrix (85 tools)

| Category | Count | Tools |
|---|---:|---|
| **Basic** (BigDecimal) | 7 | `add`, `subtract`, `multiply`, `divide`, `power`, `modulo`, `abs` |
| **Scientific** (StrictMath + exact lookup) | 7 | `sqrt`, `log`, `log10`, `factorial`, `sin`, `cos`, `tan` |
| **Programmable** (expression engine) | 2 | `evaluate`, `evaluateWithVariables` |
| **Vector** (SIMD) | 4 | `sumArray`, `dotProduct`, `scaleArray`, `magnitudeArray` |
| **Financial** (BigDecimal) | 6 | `compoundInterest`, `loanPayment`, `presentValue`, `futureValueAnnuity`, `returnOnInvestment`, `amortizationSchedule` |
| **Calculus** (numerical) | 4 | `derivative`, `nthDerivative`, `definiteIntegral`, `tangentLine` |
| **Unit converter** (21 categories, 118 units) | 2 | `convert`, `convertAutoDetect` |
| **Cooking** (gas mark + aliases) | 3 | `convertCookingVolume`, `convertCookingWeight`, `convertOvenTemperature` |
| **Measure reference** | 4 | `listCategories`, `listUnits`, `getConversionFactor`, `explainConversion` |
| **DateTime** (IANA timezones) | 5 | `convertTimezone`, `formatDateTime`, `currentDateTime`, `listTimezones`, `dateTimeDifference` |
| **Printing** (tape calculator) | 1 | `calculateWithTape` |
| **Graphing** (plot + roots) | 3 | `plotFunction`, `solveEquation`, `findRoots` |
| **Network** (IPv4/IPv6) | 13 | `subnetCalculator`, `ipToBinary`, `binaryToIp`, `ipToDecimal`, `decimalToIp`, `ipInSubnet`, `vlsmSubnets`, `summarizeSubnets`, `expandIpv6`, `compressIpv6`, `transferTime`, `throughput`, `tcpThroughput` |
| **Analog electronics** | 14 | `ohmsLaw`, `resistorCombination`, `capacitorCombination`, `inductorCombination`, `voltageDivider`, `currentDivider`, `rcTimeConstant`, `rlTimeConstant`, `rlcResonance`, `impedance`, `decibelConvert`, `filterCutoff`, `ledResistor`, `wheatstoneBridge` |
| **Digital electronics** | 10 | `convertBase`, `twosComplement`, `grayCode`, `bitwiseOp`, `adcResolution`, `dacOutput`, `timer555Astable`, `timer555Monostable`, `frequencyPeriod`, `nyquistRate` |

## Examples

All tool calls use the standard MCP `tools/call` JSON-RPC method. Examples of the `arguments` payload and the returned text:

### Arbitrary-precision arithmetic (no f64 drift)

```json
{"name": "add",    "arguments": {"first": "0.1", "second": "0.2"}}           // "0.3"
{"name": "divide", "arguments": {"first": "10", "second": "3"}}              // "3.33333333333333333333"
```

### Exact trig at notable angles

```json
{"name": "sin", "arguments": {"degrees": 30}}                                // "0.5"
{"name": "cos", "arguments": {"degrees": 60}}                                // "0.5"
{"name": "tan", "arguments": {"degrees": 45}}                                // "1.0"
```

### Expression evaluation with variables

```json
{"name": "evaluate",              "arguments": {"expression": "2+3*4"}}                       // "14.0"
{"name": "evaluateWithVariables", "arguments": {"expression": "2*x+y", "variables": "{\"x\":3,\"y\":1}"}} // "7.0"
```

### High-precision unit conversion

```json
{"name": "convert", "arguments": {"value": "1", "fromUnit": "km", "toUnit": "mi", "category": "LENGTH"}}
// "0.6213711922373339696174341843633182"
```

### Financial

```json
{"name": "compoundInterest", "arguments": {"principal": "1000", "annualRate": "5", "years": "10", "compoundsPerYear": 12}}
// "1647.009497690283034841743827660086"
```

### Networking

```json
{"name": "subnetCalculator", "arguments": {"address": "192.168.1.0", "cidr": 24}}
// {"network":"192.168.1.0","broadcast":"192.168.1.255","mask":"255.255.255.0",
//  "wildcard":"0.0.0.255","firstHost":"192.168.1.1","lastHost":"192.168.1.254",
//  "usableHosts":254,"ipClass":"C"}
```

### Calculus

```json
{"name": "definiteIntegral", "arguments": {"expression": "x^2", "variable": "x", "lower": 0, "upper": 1}}
// "0.3333333333500001"
```

## Architecture

```
math-calc-mcp (stdio binary, ~3 MB)
├── rmcp 1.5             ← MCP protocol + tool router
├── tokio (multi-thread) ← async runtime
└── math_calc (library)
    ├── engine/
    │   ├── expression.rs     ← recursive-descent parser (f64, 9 built-in functions)
    │   ├── unit_registry.rs  ← 21 categories, 118 units, gas mark, temperature
    │   └── bigdecimal_ext.rs ← constants matching Java DECIMAL128 + HALF_UP
    ├── tools/                ← one module per Java *Tool class (15 modules)
    └── server.rs             ← #[tool_router] block registering every MCP tool
```

**Dependencies** (all pure Rust, no C FFI):

| Crate | Use |
|---|---|
| `rmcp` | Official Rust MCP SDK |
| `tokio` | Async runtime (stdio I/O, multi-threaded) |
| `bigdecimal` + `num-bigint` | Arbitrary-precision arithmetic — matches Java `BigDecimal`/`BigInteger` |
| `astro-float` | Arbitrary-precision float with correct rounding — replaces `StrictMath` |
| `jiff` | Datetime with embedded IANA tz data — replaces `java.time` |
| `wide` | Portable SIMD (SSE2/AVX2/AVX-512/NEON auto-dispatch) |
| `serde` / `serde_json` | JSON I/O |
| `schemars` | JSON Schema generation for tool parameters |
| `tracing` + `tracing-subscriber` | Structured logging to stderr (stdio stays clean) |

## Precision & numerical parity with Java

This port preserves behavioral parity with the Java implementation, including error-message strings:

- **Basic arithmetic**: `BigDecimal` with `toPlainString()` output — exact results; division uses scale 20 + HALF_UP.
- **Scientific**: exact values at notable angles (`sin(30°) = 0.5`, etc.) via lookup tables; fallback computation is bit-equivalent to Java `StrictMath` on tier-1 targets.
- **Factorial**: 0..=20 range, exact `u64`.
- **Unit conversion**: conversion factors stored with 34-digit precision (DECIMAL128). Temperature uses formula routing through Celsius. Gas mark uses a fixed lookup.
- **Financial**: DECIMAL128 precision context; powers are always integer in the source, so no astro-float dependency is pulled in.
- **Electronics**: `impedance`, `rlcResonance`, `decibelConvert`, `atan2` — use `astro-float` at 128-bit precision.
- **Error strings**: verbatim matches for Java `IllegalArgumentException` messages (e.g. `"Gas mark must be 1-10. Received: 11"`).

## Development

```bash
# Format check
cargo fmt --check

# Lint (lib + bin + tests, treating warnings as errors)
cargo clippy --all-targets --all-features -- -D warnings

# Unit tests (349 tests)
cargo test --lib

# Release build
cargo build --release

# End-to-end stdio integration test (85 tools)
python3 scripts/test_stdio.py
```

### Project layout

```
.
├── .cargo/config.toml      ← target-cpu=native; cargo aliases
├── clippy.toml             ← clippy complexity thresholds
├── rust-toolchain.toml     ← pinned to stable
├── Cargo.toml              ← dependencies + release/release-portable profiles
├── src/
│   ├── main.rs             ← binary entry point (stdio transport)
│   ├── lib.rs              ← library re-exports
│   ├── server.rs           ← MCP tool registration (one #[tool_router] block)
│   ├── engine/             ← expression evaluator + unit registry + helpers
│   └── tools/              ← 15 tool modules, one per Java *Tool class
├── scripts/
│   └── test_stdio.py       ← Python stdio e2e test — 85 tools in ~0.5s
└── target/release/math-calc-mcp   ← static binary (~3 MB)
```

## License

Licensed under the [Apache License, Version 2.0](LICENSE).

Original Java project: [farchanjo/math-calculator](https://github.com/farchanjo/math-calculator) (also Apache-2.0).
