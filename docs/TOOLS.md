# Tools Catalog — 87 tools, 15 categories

Every tool is a stateless function whose response is a compact, line-oriented envelope built by `src/mcp/message/builder.rs`. Numeric parameters are passed as **strings** to preserve arbitrary precision.

> **Total**: 87 tools — 7 + 7 + 4 + 4 + 6 + 4 + 2 + 3 + 4 + 5 + 1 + 3 + 13 + 14 + 10.

## Response format

| Shape | Layout | Example |
|:---|:---|:---|
| Scalar success | `TOOL: OK \| RESULT: value` | `ADD: OK \| RESULT: 0.3` |
| Multi-field success | `TOOL: OK \| KEY: v \| KEY: v \| …` | `OHMS_LAW: OK \| VOLTAGE: 12 \| CURRENT: 3 \| RESISTANCE: 4 \| POWER: 36` |
| Tabular success (block) | `TOOL: OK\n<fields>\nROW_N: k=v \| k=v` | Used by `amortizationSchedule`, `plotFunction`, `vlsmSubnets`. |
| Error | `TOOL: ERROR\nREASON: [CODE] text\n[DETAIL: k=v]` | `DIVIDE: ERROR\nREASON: [DIVISION_BY_ZERO] cannot divide by zero` |

Tool names in responses use `SCREAMING_SNAKE_CASE` (e.g. `SUBNET_CALCULATOR`, `EVALUATE_WITH_VARIABLES`). Values containing newlines are escaped as `\n`.

**Error codes**: `DOMAIN_ERROR`, `OUT_OF_RANGE`, `DIVISION_BY_ZERO`, `PARSE_ERROR`, `INVALID_INPUT`, `UNKNOWN_VARIABLE`, `UNKNOWN_FUNCTION`, `OVERFLOW`, `NOT_IMPLEMENTED`.

## Category index

| # | Category | Count | Jump |
|:-:|:---|:-:|:---|
| 1 | Basic math | 7 | [↓](#basic-math-7) |
| 2 | Scientific | 7 | [↓](#scientific-7) |
| 3 | Expression engine | 4 | [↓](#expression-engine-4) |
| 4 | Vectors & arrays | 4 | [↓](#vectors--arrays-4) |
| 5 | Finance | 6 | [↓](#finance-6) |
| 6 | Calculus | 4 | [↓](#calculus-4) |
| 7 | Unit conversion | 2 | [↓](#unit-conversion-2) |
| 8 | Cooking | 3 | [↓](#cooking-3) |
| 9 | Measure reference | 4 | [↓](#measure-reference-4) |
| 10 | Date & time | 5 | [↓](#date--time-5) |
| 11 | Tape calculator | 1 | [↓](#tape-calculator-1) |
| 12 | Graphing & roots | 3 | [↓](#graphing--roots-3) |
| 13 | Networking | 13 | [↓](#networking-13) |
| 14 | Analog electronics | 14 | [↓](#analog-electronics-14) |
| 15 | Digital electronics | 10 | [↓](#digital-electronics-10) |

---

## Basic math (7)

Arbitrary-precision arithmetic via `BigDecimal`. All return `TOOL: OK | RESULT: <value>`.

| Tool | Inputs | Example response |
|:---|:---|:---|
| `add` | `first`, `second` | `ADD: OK \| RESULT: 0.3` |
| `subtract` | `first`, `second` | `SUBTRACT: OK \| RESULT: 0.1` |
| `multiply` | `first`, `second` | `MULTIPLY: OK \| RESULT: 6` |
| `divide` | `first`, `second` | `DIVIDE: OK \| RESULT: 3.33333333333333333333` |
| `power` | `base`, `exponent` | `POWER: OK \| RESULT: 1024` |
| `modulo` | `first`, `second` | `MODULO: OK \| RESULT: 1` |
| `abs` | `value` | `ABS: OK \| RESULT: 42` |

## Scientific (7)

Trigonometry and transcendentals — exact at notable angles, 128-bit elsewhere.

| Tool | Inputs | Example response |
|:---|:---|:---|
| `sin` | `degrees` | `SIN: OK \| RESULT: 0.5` |
| `cos` | `degrees` | `COS: OK \| RESULT: 0.5` |
| `tan` | `degrees` | `TAN: OK \| RESULT: 1` |
| `sqrt` | `number` | `SQRT: OK \| RESULT: 1.4142135623730951` |
| `log` | `number` | `LOG: OK \| RESULT: 1.0` |
| `log10` | `number` | `LOG10: OK \| RESULT: 3.0` |
| `factorial` | `num` (0–20) | `FACTORIAL: OK \| RESULT: 120` |

## Expression engine (4)

Parse-and-evaluate any expression with full operator support. `variables` is a **string containing JSON**.

| Tool | Inputs | Response shape |
|:---|:---|:---|
| `evaluate` | `expression` | `EVALUATE: OK \| RESULT: <f64>` |
| `evaluateWithVariables` | `expression`, `variables` | `EVALUATE_WITH_VARIABLES: OK \| RESULT: <f64>` |
| `evaluateExact` | `expression` | `EVALUATE_EXACT: OK \| RESULT: <128-bit>` |
| `evaluateExactWithVariables` | `expression`, `variables` | `EVALUATE_EXACT_WITH_VARIABLES: OK \| RESULT: <128-bit>` |

**Operators**: `+ - * / ^ %` and parentheses.
**Built-ins**: `sin`, `cos`, `tan`, `log`, `log10`, `sqrt`, `abs`, `ceil`, `floor`.

Common errors: `UNKNOWN_VARIABLE` (e.g. `DETAIL: name=foo`), `UNKNOWN_FUNCTION`, `DIVISION_BY_ZERO`, `PARSE_ERROR`.

## Vectors & arrays (4)

SIMD-accelerated via `wide`. Arrays are comma-separated strings.

| Tool | Inputs | Example response |
|:---|:---|:---|
| `sumArray` | CSV numbers | `SUM_ARRAY: OK \| RESULT: 15` |
| `dotProduct` | two CSV arrays | `DOT_PRODUCT: OK \| RESULT: 32` |
| `scaleArray` | CSV array, scalar | `SCALE_ARRAY: OK \| RESULT: 2,4,6` |
| `magnitudeArray` | CSV array | `MAGNITUDE_ARRAY: OK \| RESULT: 5` |

## Finance (6)

Time-value-of-money calculations with DECIMAL128 precision.

| Tool | Inputs | Response |
|:---|:---|:---|
| `compoundInterest` | principal, annualRate (%), years, compoundsPerYear | `COMPOUND_INTEREST: OK \| RESULT: <fv>` |
| `loanPayment` | principal, annualRate (%), years | `LOAN_PAYMENT: OK \| RESULT: <monthly>` |
| `presentValue` | futureValue, rate (%), years | `PRESENT_VALUE: OK \| RESULT: <pv>` |
| `futureValueAnnuity` | payment, rate (%), years | `FUTURE_VALUE_ANNUITY: OK \| RESULT: <fv>` |
| `returnOnInvestment` | gain, cost | `RETURN_ON_INVESTMENT: OK \| RESULT: <roi%>` |
| `amortizationSchedule` | principal, annualRate (%), years | Block layout with `MONTHLY_PAYMENT`, `TOTAL_INTEREST`, `TOTAL_PAID`, `MONTHS`, then `ROW_N: month=… \| payment=… \| principal=… \| interest=… \| balance=…` |

## Calculus (4)

Numerical calculus on parsed expressions.

| Tool | Inputs | Output | Method |
|:---|:---|:---|:---|
| `derivative` | expression, variable, point | `DERIVATIVE: OK \| RESULT: <f'(x)>` | Five-point central difference |
| `nthDerivative` | expression, variable, point, order (1–10) | `NTH_DERIVATIVE: OK \| RESULT: …` | Repeated finite differences |
| `definiteIntegral` | expression, variable, lower, upper | `DEFINITE_INTEGRAL: OK \| RESULT: …` | Composite Simpson's (10 000 intervals) |
| `tangentLine` | expression, variable, point | `TANGENT_LINE: OK \| SLOPE: … \| Y_INTERCEPT: … \| EQUATION: …` | Derivative + point-slope |

## Unit conversion (2)

21 categories, 118 units.

| Tool | Inputs | Response |
|:---|:---|:---|
| `convert` | value, fromUnit, toUnit, category | `CONVERT: OK \| RESULT: <34-digit value>` |
| `convertAutoDetect` | value, fromUnit, toUnit | `CONVERT_AUTO_DETECT: OK \| RESULT: …` |

**Categories**: `DATA_STORAGE`, `LENGTH`, `MASS`, `VOLUME`, `TEMPERATURE`, `TIME`, `SPEED`, `AREA`, `ENERGY`, `FORCE`, `PRESSURE`, `POWER`, `DENSITY`, `FREQUENCY`, `ANGLE`, `DATA_RATE`, `RESISTANCE`, `CAPACITANCE`, `INDUCTANCE`, `VOLTAGE`, `CURRENT`.

Unknown units raise `INVALID_INPUT` with `DETAIL: unit=<name>`.

## Cooking (3)

Kitchen-scale conversions.

| Tool | Units | Example response |
|:---|:---|:---|
| `convertCookingVolume` | `cup`, `tbsp`, `tsp`, `ml`, `l` | `CONVERT_COOKING_VOLUME: OK \| RESULT: 236.588…` |
| `convertCookingWeight` | `g`, `kg`, `oz`, `lb` | `CONVERT_COOKING_WEIGHT: OK \| RESULT: 453.592…` |
| `convertOvenTemperature` | `c`, `f`, `gasmark` | `CONVERT_OVEN_TEMPERATURE: OK \| RESULT: 176.67` |

## Measure reference (4)

Introspection helpers for the unit system.

| Tool | Inputs | Response |
|:---|:---|:---|
| `listCategories` | — | `LIST_CATEGORIES: OK \| COUNT: 21 \| VALUES: DATA_STORAGE,LENGTH,…,CURRENT` |
| `listUnits` | category | `LIST_UNITS: OK \| CATEGORY: LENGTH \| COUNT: N \| VALUES: m,km,mi,…` |
| `getConversionFactor` | fromUnit, toUnit | `GET_CONVERSION_FACTOR: OK \| RESULT: <factor>` |
| `explainConversion` | fromUnit, toUnit | `EXPLAIN_CONVERSION: OK \| RESULT: "<human readable>"` |

## Date & time (5)

IANA-aware; no `libicu`.

| Tool | Inputs | Response |
|:---|:---|:---|
| `convertTimezone` | datetime, fromTimezone, toTimezone | `CONVERT_TIMEZONE: OK \| RESULT: <ISO-8601 zoned>` |
| `formatDateTime` | datetime, inputFormat, outputFormat, timezone | `FORMAT_DATE_TIME: OK \| RESULT: <formatted>` |
| `currentDateTime` | timezone, format | `CURRENT_DATE_TIME: OK \| RESULT: <now>` |
| `listTimezones` | region prefix (`"America"`, `"Europe"`, `"all"`) | `LIST_TIMEZONES: OK \| COUNT: N \| VALUES: <IANA,ids,…>` |
| `dateTimeDifference` | datetime1, datetime2, timezone | `DATE_TIME_DIFFERENCE: OK \| YEARS: … \| MONTHS: … \| DAYS: … \| HOURS: … \| MINUTES: … \| SECONDS: … \| TOTAL_SECONDS: …` |

**Format keywords**: `iso`, `iso-offset`, `iso-local`, `epoch`, `epochmillis`, `rfc1123` — or any strftime pattern.

## Tape calculator (1)

| Tool | Inputs | Response |
|:---|:---|:---|
| `calculateWithTape` | JSON array of `{op, value}` | Block layout. Each tape line is one `ROW_N: op=… \| value=… \| running=…` row, ending with a `TOTAL` field. |

**Ops**: `+`, `-`, `*`, `/`, `=` (total), `C` (clear), `T` (subtotal).

## Graphing & roots (3)

| Tool | Inputs | Response |
|:---|:---|:---|
| `plotFunction` | expression, variable, min, max, steps | Block layout: `COUNT: N` + `ROW_N: x=… \| y=…`. |
| `solveEquation` | expression, variable, initialGuess | `SOLVE_EQUATION: OK \| RESULT: <root>` (or `NO_ROOT` status on failure). |
| `findRoots` | expression, variable, min, max | Block layout: `COUNT: N` + `ROW_N: x=…`. |

## Networking (13)

IPv4/IPv6, CIDR, VLSM, throughput.

| Tool | Response |
|:---|:---|
| `subnetCalculator` | `SUBNET_CALCULATOR: OK \| NETWORK: … \| BROADCAST: … \| MASK: … \| WILDCARD: … \| FIRST_HOST: … \| LAST_HOST: … \| USABLE_HOSTS: … \| IP_CLASS: …` |
| `ipToBinary` | `IP_TO_BINARY: OK \| RESULT: <dotted/colon binary>` |
| `binaryToIp` | `BINARY_TO_IP: OK \| RESULT: <ip>` |
| `ipToDecimal` | `IP_TO_DECIMAL: OK \| RESULT: <unsigned>` |
| `decimalToIp` | `DECIMAL_TO_IP: OK \| RESULT: <ip>` |
| `ipInSubnet` | `IP_IN_SUBNET: OK \| RESULT: true \| false` |
| `vlsmSubnets` | Block layout — one `ROW_N: network=… \| cidr=… \| first=… \| last=… \| hosts=…` per allocation. |
| `summarizeSubnets` | `SUMMARIZE_SUBNETS: OK \| RESULT: <supernet/cidr>` |
| `expandIpv6` | `EXPAND_IPV6: OK \| RESULT: <8-group>` |
| `compressIpv6` | `COMPRESS_IPV6: OK \| RESULT: <shortest>` |
| `transferTime` | `TRANSFER_TIME: OK \| SECONDS: … \| MINUTES: … \| HOURS: …` |
| `throughput` | `THROUGHPUT: OK \| RESULT: <rate> \| UNIT: <Mbps \| Gbps \| …>` |
| `tcpThroughput` | `TCP_THROUGHPUT: OK \| RESULT: <Mbps>` |

## Analog electronics (14)

Circuit analysis.

| Tool | Response / formula |
|:---|:---|
| `ohmsLaw` | `OHMS_LAW: OK \| VOLTAGE: … \| CURRENT: … \| RESISTANCE: … \| POWER: …` — supply any 2, get all 4. |
| `resistorCombination` | `RESISTOR_COMBINATION: OK \| RESULT: <Ω>` — series sum or `1/Σ(1/Rᵢ)`. |
| `capacitorCombination` | Dual of resistors. |
| `inductorCombination` | Dual of resistors. |
| `voltageDivider` | `VOLTAGE_DIVIDER: OK \| RESULT: <Vout>` — `Vin · R2 / (R1+R2)`. |
| `currentDivider` | `CURRENT_DIVIDER: OK \| I1: … \| I2: …`. |
| `rcTimeConstant` | `RC_TIME_CONSTANT: OK \| TAU: … \| CUTOFF: …`. |
| `rlTimeConstant` | `RL_TIME_CONSTANT: OK \| TAU: … \| CUTOFF: …`. |
| `rlcResonance` | `RLC_RESONANCE: OK \| FREQUENCY: … \| Q_FACTOR: … \| BANDWIDTH: …`. |
| `impedance` | `IMPEDANCE: OK \| MAGNITUDE: … \| PHASE_DEG: …`. |
| `decibelConvert` | `DECIBEL_CONVERT: OK \| RESULT: …` — modes `powerToDb`, `voltageToDb`, `dbToPower`, `dbToVoltage`. |
| `filterCutoff` | `FILTER_CUTOFF: OK \| RESULT: <Hz>`. |
| `ledResistor` | `LED_RESISTOR: OK \| RESULT: <Ω>` — `R = (Vs − Vf) / If`. |
| `wheatstoneBridge` | `WHEATSTONE_BRIDGE: OK \| RESULT: <R4>` — `R4 = R3·R2 / R1`. |

## Digital electronics (10)

Bit-level operations, ADC/DAC, timers.

| Tool | Response |
|:---|:---|
| `convertBase` | `CONVERT_BASE: OK \| RESULT: <uppercase digits>` — bases 2–36. |
| `twosComplement` | `TWOS_COMPLEMENT: OK \| RESULT: …` — `bits ∈ [1, 64]`. |
| `grayCode` | `GRAY_CODE: OK \| RESULT: …` — `toGray` / `fromGray`. |
| `bitwiseOp` | `BITWISE_OP: OK \| DECIMAL: … \| BINARY: …` — `AND`, `OR`, `XOR`, `NOT`, `SHL`, `SHR`. |
| `adcResolution` | `ADC_RESOLUTION: OK \| LSB: … \| STEPS: …`. |
| `dacOutput` | `DAC_OUTPUT: OK \| RESULT: <V>`. |
| `timer555Astable` | `TIMER_555_ASTABLE: OK \| FREQUENCY: … \| DUTY_CYCLE: … \| PERIOD: …`. |
| `timer555Monostable` | `TIMER_555_MONOSTABLE: OK \| RESULT: <pulse width>` — `PW = 1.1·R·C`. |
| `frequencyPeriod` | `FREQUENCY_PERIOD: OK \| RESULT: …` — `freqToPeriod` / `periodToFreq`. |
| `nyquistRate` | `NYQUIST_RATE: OK \| RESULT: <min sample rate>`. |

---

## Summary

- **87 tools** across **15 categories**.
- Every response is a single string in arithma's line-oriented envelope: `TOOL: OK | …` on success, `TOOL: ERROR\nREASON: [CODE] …` on failure.
- Arbitrary precision where it matters (arithmetic, finance, unit conversion).
- Stateless — safe to fan out concurrent calls.

See [API.md](./API.md) for wire-level JSON-RPC examples.
