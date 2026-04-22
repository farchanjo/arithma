# arithma Documentation

**The Ultimate LLM Calculator Engine** — 87 precision math tools exposed over [MCP](https://modelcontextprotocol.io).

## Navigation

| Document | When to read it |
|:---|:---|
| [Architecture](./ARCHITECTURE.md) | You want the big picture: modules, data flow, design decisions. |
| [Tools Catalog](./TOOLS.md) | You need a specific tool's inputs, outputs, and an example. |
| [Development Guide](./DEVELOPMENT.md) | You're building, testing, or contributing. |
| [API Usage](./API.md) | You're wiring arithma into an MCP client or LLM pipeline. |

## What is arithma?

A pure-Rust MCP server that delivers **87 expert-grade calculators** to any LLM-capable client. Built for precision and portability:

- **Arbitrary precision** — `BigDecimal` with DECIMAL128 semantics (34 digits, HALF_UP).
- **128-bit transcendentals** — correctly-rounded `sin`/`cos`/`tan`/`log` via `astro-float`.
- **Zero C deps** — single ~3 MB static binary, identical on Linux, macOS, and Windows.
- **Portable SIMD** — runtime dispatch via `wide` (SSE2 / AVX2 / AVX-512 / NEON).
- **Stateless & fast** — sub-second startup, millisecond-scale tool latency, safe to fan out.
- **Tested** — 434 tests (349 unit + 87 stdio integration), full suite runs in under a second.
- **LLM-friendly wire format** — `TOOL: OK | KEY: value | …` on success, `TOOL: ERROR\nREASON: [CODE] …` on failure. No JSON parsing required on the client side.

## I want to…

| Goal | Start here |
|:---|:---|
| Build from source | [Development › Building](./DEVELOPMENT.md#building) |
| Wire into Claude Code / Desktop | [API › Integration](./API.md#integration) |
| Look up a specific tool | [Tools Catalog](./TOOLS.md) |
| Understand the internals | [Architecture](./ARCHITECTURE.md) |
| Contribute a change | [Development › Contributing](./DEVELOPMENT.md#contributing) |

---

**Repository**: [github.com/farchanjo/arithma](https://github.com/farchanjo/arithma) · **License**: Apache-2.0
