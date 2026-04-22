# API Usage & Integration

How to wire arithma into an MCP client and call its 87 tools from code or an LLM.

## Integration

### Claude Code

```bash
claude mcp add arithma -- /absolute/path/to/target/release/arithma
```

### Claude Desktop (`mcp.json`)

```json
{
  "mcpServers": {
    "arithma": {
      "command": "/absolute/path/to/target/release/arithma"
    }
  }
}
```

### Cursor, Windsurf, OpenCode

Same stdio interface. Point the client's MCP config at the binary path.

### Verify the server responds

```bash
(printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}\n';
 printf '{"jsonrpc":"2.0","method":"notifications/initialized"}\n';
 printf '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}\n';
 sleep 0.3) | /absolute/path/to/arithma 2>/dev/null | jq .
```

`tools/list` must enumerate all 87 tools.

## Tool-calling convention

Every tool is invoked with the MCP `tools/call` method:

```json
{
  "jsonrpc": "2.0",
  "id": 123,
  "method": "tools/call",
  "params": {
    "name": "tool_name",
    "arguments": {
      "param1": "value1",
      "param2": 42
    }
  }
}
```

### Response wire format

The `result` of every tool call is a **single string** in arithma's line-oriented envelope (implemented in `src/mcp/message/builder.rs`).

| Shape | Layout |
|:---|:---|
| Scalar success | `TOOL: OK \| RESULT: value` |
| Multi-field success | `TOOL: OK \| KEY_1: v1 \| KEY_2: v2 \| …` |
| Tabular (block) | `TOOL: OK\n<fields>\nROW_1: k=v \| k=v\nROW_2: …` |
| Custom status | `TOOL: <STATUS> \| KEY: v \| …` (e.g. `SOLVE: NO_ROOT \| REASON: diverged`) |
| Error | `TOOL: ERROR\nREASON: [CODE] text\n[DETAIL: k=v]` |

Notes:

- Tool names render in `SCREAMING_SNAKE_CASE`: `ADD`, `SUBNET_CALCULATOR`, `EVALUATE_WITH_VARIABLES`.
- Keys are `SCREAMING_SNAKE_CASE`.
- Values containing newlines are escaped as `\n` (literal backslash + `n`).
- The MCP layer never raises a protocol-level error for a tool failure — always inspect the `result` string.

**Error codes**: `DOMAIN_ERROR`, `OUT_OF_RANGE`, `DIVISION_BY_ZERO`, `PARSE_ERROR`, `INVALID_INPUT`, `UNKNOWN_VARIABLE`, `UNKNOWN_FUNCTION`, `OVERFLOW`, `NOT_IMPLEMENTED`.

### Parsing the envelope

A minimal parser:

```javascript
function parse(result) {
  const lines = result.split('\n');
  const [tool, status] = lines[0].split(':').map(s => s.trim());

  if (status.startsWith('ERROR')) {
    const reason = lines[1]?.match(/\[(\w+)\]\s*(.*)/);
    const detail = lines[2]?.startsWith('DETAIL:') ? lines[2].slice(7).trim() : null;
    return { tool, ok: false, code: reason?.[1], message: reason?.[2], detail };
  }

  // success: status may be "OK" or a custom token, followed by " | KEY: v | …"
  const [head, ...fields] = result.split(' | ');
  const kv = Object.fromEntries(
    fields.map(f => {
      const i = f.indexOf(':');
      return [f.slice(0, i).trim(), f.slice(i + 1).trim()];
    })
  );
  return { tool, ok: true, status: head.split(':')[1].trim(), fields: kv };
}
```

## Parameter types

### Decimal numbers → strings

Always pass numeric values as strings to preserve arbitrary precision:

```json
{"value": "0.123456789012345678901234567890"}
```

A JSON number (`0.123456789012345678901234567890`) is silently truncated to `f64` by most JSON parsers.

### JSON objects → JSON-encoded strings

Some tools accept nested data as **a string that contains JSON**:

```json
{
  "name": "evaluateWithVariables",
  "arguments": {
    "expression": "2*x + y",
    "variables":  "{\"x\": 3.5, \"y\": 2}"
  }
}
```

### Arrays → CSV strings

```json
{"name": "sumArray", "arguments": {"numbers": "1,2,3,4,5"}}
```

### Empty strings → "not provided"

Tools like `ohmsLaw` accept any two of four parameters. Leave the others empty:

```json
{
  "name": "ohmsLaw",
  "arguments": {
    "voltage": "12",
    "current": "",
    "resistance": "4",
    "power": ""
  }
}
```

## Common use cases

Each block shows the `tools/call` arguments and the exact string returned in `result`.

### 1. Basic arithmetic

```json
{"name":"add","arguments":{"first":"0.1","second":"0.2"}}
```
```text
ADD: OK | RESULT: 0.3
```

### 2. Expression evaluation

```json
{"name":"evaluate","arguments":{"expression":"2 * sin(45) + 3^2"}}
```
```text
EVALUATE: OK | RESULT: 10.414213562373096
```

### 3. Variable substitution

```json
{"name":"evaluateWithVariables",
 "arguments":{
   "expression":"principal * (1 + rate/100)^years",
   "variables":"{\"principal\":1000,\"rate\":5,\"years\":10}"}}
```
```text
EVALUATE_WITH_VARIABLES: OK | RESULT: 1628.8946267774418
```

### 4. Unit conversion

```json
{"name":"convert",
 "arguments":{"value":"5","fromUnit":"km","toUnit":"mi","category":"LENGTH"}}
```
```text
CONVERT: OK | RESULT: 3.1068559611866698480871709218165910
```

### 5. Financial — compound interest

```json
{"name":"compoundInterest",
 "arguments":{"principal":"10000","annualRate":"5","years":"20","compoundsPerYear":12}}
```
```text
COMPOUND_INTEREST: OK | RESULT: 27126.400573618468815...
```

### 6. Financial — amortization (block layout)

```json
{"name":"amortizationSchedule",
 "arguments":{"principal":"1000","annualRate":"5","years":"1"}}
```
```text
AMORTIZATION_SCHEDULE: OK
MONTHLY_PAYMENT: 85.61
TOTAL_INTEREST: 27.30
TOTAL_PAID: 1027.32
MONTHS: 12
ROW_1: month=1 | payment=85.61 | principal=81.44 | interest=4.17 | balance=918.56
ROW_2: month=2 | payment=85.61 | principal=81.78 | interest=3.83 | balance=836.78
...
ROW_12: month=12 | payment=85.61 | principal=85.25 | interest=0.36 | balance=0.00
```

### 7. Networking — subnet

```json
{"name":"subnetCalculator","arguments":{"address":"192.168.1.0","cidr":24}}
```
```text
SUBNET_CALCULATOR: OK | NETWORK: 192.168.1.0 | BROADCAST: 192.168.1.255 | MASK: 255.255.255.0 | WILDCARD: 0.0.0.255 | FIRST_HOST: 192.168.1.1 | LAST_HOST: 192.168.1.254 | USABLE_HOSTS: 254 | IP_CLASS: C
```

### 8. Electronics — Ohm's law

```json
{"name":"ohmsLaw",
 "arguments":{"voltage":"12","current":"","resistance":"4","power":""}}
```
```text
OHMS_LAW: OK | VOLTAGE: 12 | CURRENT: 3 | RESISTANCE: 4 | POWER: 36
```

### 9. Introspection

```json
{"name":"listCategories","arguments":{}}
```
```text
LIST_CATEGORIES: OK | COUNT: 21 | VALUES: DATA_STORAGE,LENGTH,MASS,VOLUME,TEMPERATURE,TIME,SPEED,AREA,ENERGY,FORCE,PRESSURE,POWER,DENSITY,FREQUENCY,ANGLE,DATA_RATE,RESISTANCE,CAPACITANCE,INDUCTANCE,VOLTAGE,CURRENT
```

## Errors

Failures come back as a two- or three-line block. Examples:

```text
DIVIDE: ERROR
REASON: [DIVISION_BY_ZERO] cannot divide by zero
```

```text
CONVERT: ERROR
REASON: [INVALID_INPUT] unit is not a recognized unit
DETAIL: unit=foo
```

```text
FACTORIAL: ERROR
REASON: [OUT_OF_RANGE] factorial is defined for integers 0..=20
DETAIL: received=25
```

| Code | Typical cause |
|:---|:---|
| `DOMAIN_ERROR` | `sqrt(-1)`, `log(0)`, `tan(90°)`. |
| `OUT_OF_RANGE` | `factorial(25)`, exponent overflow. |
| `DIVISION_BY_ZERO` | Divisor is zero. |
| `PARSE_ERROR` | Malformed expression or number. |
| `INVALID_INPUT` | Unknown unit/category, wrong mode, bad CSV. |
| `UNKNOWN_VARIABLE` | Expression references a variable not in the map. |
| `UNKNOWN_FUNCTION` | Expression calls a function not in the built-in list. |
| `OVERFLOW` | Result exceeds the target numeric type. |
| `NOT_IMPLEMENTED` | Operation intentionally unsupported for this input shape. |

## Performance notes

- Tool calls are **stateless**; clients may fan out concurrently without coordination.
- Use `evaluateExact` only when ~34-digit precision matters — it is noticeably slower than `evaluate`.
- Pre-compute shared intermediates on the client side when looping over large inputs, rather than re-issuing identical calls.

## Precision summary

| Tool family | Precision | Notes |
|:---|:---|:---|
| Basic arithmetic | Exact (`BigDecimal`) | Division scaled to 20 digits. |
| Scientific | Exact at notable angles | `astro-float` elsewhere. |
| `evaluate` | ~15–17 digits | `f64` fast path. |
| `evaluateExact` | ~34 digits | 128-bit `astro-float`. |
| Financial | DECIMAL128 | 34 digits, HALF_UP. |
| Unit conversion | DECIMAL128 | 34-digit factors. |
| Date / Time | IANA standard | Embedded tz database. |

---

See [TOOLS.md](./TOOLS.md) for the complete tool reference.
