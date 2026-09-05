#!/usr/bin/env python3
"""
End-to-end stdio integration test for the math-calc-mcp binary.

Spawns the MCP server as a subprocess, handshakes, enumerates tools via
`tools/list`, and exercises every tool at least once via JSON-RPC over stdio.

The server returns a canonical envelope for every tool call:

    Inline success:  TOOL_NAME: OK | KEY: value | KEY: value
    Block success:   TOOL_NAME: OK\\nKEY: value\\nROW_1: k=v | k2=v2\\n...
    Error:           TOOL_NAME: ERROR\\nREASON: [CODE] lowercase reason\\nDETAIL: ...

Usage:
    cargo build --release --bin math-calc-mcp
    python3 scripts/test_stdio.py
"""

from __future__ import annotations

import json
import os
import queue
import subprocess
import sys
import threading
import time
from subprocess import PIPE

# Test labels carry non-ASCII glyphs (φ, ±, °). The Windows console defaults
# to cp1252, where print() raises UnicodeEncodeError and kills the run
# mid-suite. Force UTF-8 on the report streams regardless of console codepage.
for _stream in (sys.stdout, sys.stderr):
    if hasattr(_stream, "reconfigure"):
        _stream.reconfigure(encoding="utf-8", errors="replace")

BINARY = os.environ.get(
    "MATH_CALC_MCP",
    os.path.join(os.path.dirname(__file__), "..", "target", "release", "arithma"),
)

# --------------------------------------------------------------------------- #
#  MCP client over stdio
# --------------------------------------------------------------------------- #


class McpClient:
    def __init__(self) -> None:
        env = os.environ.copy()
        env["RUST_LOG"] = "error"
        self.proc = subprocess.Popen(
            [BINARY],
            stdin=PIPE,
            stdout=PIPE,
            stderr=PIPE,
            env=env,
        )
        self.req_id = 0
        self.response_queue: "queue.Queue[dict]" = queue.Queue()

        self._stdout_thread = threading.Thread(target=self._read_stdout, daemon=True)
        self._stdout_thread.start()
        self._stderr_thread = threading.Thread(target=self._drain_stderr, daemon=True)
        self._stderr_thread.start()

        time.sleep(0.3)
        if self.proc.poll() is not None:
            raise RuntimeError(f"Process exited with code {self.proc.returncode}")

    def _read_stdout(self) -> None:
        while True:
            line = self.proc.stdout.readline()
            if not line:
                break
            try:
                data = json.loads(line)
                self.response_queue.put(data)
            except json.JSONDecodeError:
                pass

    def _drain_stderr(self) -> None:
        while True:
            line = self.proc.stderr.readline()
            if not line:
                break

    def send(self, method: str, params=None, is_notification: bool = False):
        self.req_id += 1
        req = {"jsonrpc": "2.0", "method": method, "params": params or {}}
        if not is_notification:
            req["id"] = self.req_id
        line = json.dumps(req) + "\n"
        self.proc.stdin.write(line.encode())
        self.proc.stdin.flush()
        if is_notification:
            return None
        try:
            return self.response_queue.get(timeout=30)
        except queue.Empty:
            return {"error": "Timeout waiting for response"}

    def initialize(self) -> None:
        self.send(
            "initialize",
            {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "math-calc-test", "version": "0.1.0"},
            },
        )
        self.send("notifications/initialized", is_notification=True)

    def list_tools(self):
        resp = self.send("tools/list", {})
        return [t["name"] for t in resp.get("result", {}).get("tools", [])]

    def call(self, name: str, arguments: dict) -> str:
        """Return the envelope text verbatim. The server ALWAYS returns the
        canonical envelope format — no JSON decoding is performed."""
        resp = self.send("tools/call", {"name": name, "arguments": arguments})
        if resp is None:
            return "CLIENT: ERROR\nREASON: [TRANSPORT] no response"
        if "error" in resp and isinstance(resp["error"], dict):
            msg = resp["error"].get("message", str(resp["error"]))
            return f"CLIENT: ERROR\nREASON: [TRANSPORT] {msg}"
        result = resp.get("result", {})
        text = (result.get("content") or [{}])[0].get("text", "")
        return text if isinstance(text, str) else str(text)

    def close(self) -> None:
        try:
            self.proc.stdin.close()
        except Exception:
            pass
        try:
            self.proc.terminate()
            self.proc.wait(timeout=5)
        except Exception:
            self.proc.kill()


# --------------------------------------------------------------------------- #
#  Envelope parsing helpers
# --------------------------------------------------------------------------- #


def parse_envelope(text):
    """Return (tool, status, fields_dict, error_code) tuple.

    For OK responses, fields_dict maps KEY -> value.
    For ERROR responses, error_code is the bracketed token; fields_dict
    includes 'REASON' (full reason without brackets) and optionally 'DETAIL'.
    """
    if not isinstance(text, str):
        return ("", "", {}, None)

    lines = text.split("\n")
    if not lines:
        return ("", "", {}, None)

    header = lines[0]
    fields: dict[str, str] = {}
    error_code: str | None = None

    # Header can be inline-success ("TOOL: OK | K: v | ...") or pure
    # status-only ("TOOL: OK" / "TOOL: ERROR"). Split header on " | " to
    # harvest any inline fields.
    parts = header.split(" | ")
    first = parts[0]
    if ":" not in first:
        return ("", "", {}, None)
    tool, _, status_raw = first.partition(":")
    tool = tool.strip()
    status = status_raw.strip()

    for segment in parts[1:]:
        if ":" in segment:
            k, _, v = segment.partition(":")
            fields[k.strip()] = v.strip()

    # Remaining lines are block fields: either "KEY: value" or continuations.
    for line in lines[1:]:
        if ":" not in line:
            continue
        k, _, v = line.partition(":")
        key = k.strip()
        value = v.strip()

        if status == "ERROR" and key == "REASON":
            # REASON: [CODE] lowercase reason text
            if value.startswith("[") and "]" in value:
                end = value.index("]")
                error_code = value[1:end]
                fields[key] = value[end + 1:].strip()
            else:
                fields[key] = value
        else:
            fields[key] = value

    return (tool, status, fields, error_code)


def envelope_ok(text, tool):
    t, s, _, _ = parse_envelope(text)
    return t == tool and s == "OK"


def envelope_field(text, key):
    _, _, f, _ = parse_envelope(text)
    return f.get(key)


def envelope_result(text, tool):
    return envelope_field(text, "RESULT") if envelope_ok(text, tool) else None


def envelope_error(text, tool, code=None):
    t, s, _, c = parse_envelope(text)
    ok = t == tool and s == "ERROR"
    return ok and (code is None or c == code)


# --------------------------------------------------------------------------- #
#  Test harness
# --------------------------------------------------------------------------- #


class TestRunner:
    def __init__(self, client: McpClient) -> None:
        self.client = client
        self.results: list[tuple[str, str, str, bool, str]] = []  # (category, tool, call_desc, passed, detail)
        self.current_category = ""

    def category(self, name: str, expected_count: int) -> None:
        print(f"\n=== {name.upper()} ({expected_count} tools) ===")
        self.current_category = name

    def record(self, tool: str, call_desc: str, passed: bool, detail: str) -> None:
        self.results.append((self.current_category, tool, call_desc, passed, detail))
        status = "PASS" if passed else "FAIL"
        print(f"  {status} {tool}({call_desc}) -> {detail}")

    # --- helpers --- #

    @staticmethod
    def close(actual, expected, tol=1e-6) -> bool:
        try:
            return abs(float(actual) - float(expected)) <= tol
        except (TypeError, ValueError):
            return False

    def check(self, tool: str, call_desc: str, result, predicate, detail_render=None) -> bool:
        passed = False
        try:
            passed = bool(predicate(result))
        except Exception as exc:  # noqa: BLE001
            detail = f"exception: {exc}; result={result!r}"
            self.record(tool, call_desc, False, detail)
            return False
        if detail_render:
            detail = detail_render(result)
        else:
            detail = repr(result) if not isinstance(result, str) else result
            if len(detail) > 80:
                detail = detail[:77] + "..."
        self.record(tool, call_desc, passed, detail)
        return passed


# --------------------------------------------------------------------------- #
#  Category test implementations
# --------------------------------------------------------------------------- #


def test_basic(r: TestRunner) -> None:
    r.category("basic", 7)
    c = r.client.call
    r.check("add", "0.1, 0.2", c("add", {"first": "0.1", "second": "0.2"}),
            lambda v: envelope_result(v, "ADD") == "0.3")
    r.check("subtract", "10, 3", c("subtract", {"first": "10", "second": "3"}),
            lambda v: envelope_result(v, "SUBTRACT") == "7")
    r.check("multiply", "3, 4", c("multiply", {"first": "3", "second": "4"}),
            lambda v: envelope_result(v, "MULTIPLY") == "12")
    r.check("divide", "10, 3", c("divide", {"first": "10", "second": "3"}),
            lambda v: envelope_result(v, "DIVIDE") == "3.33333333333333333333")
    r.check("power", "2^10", c("power", {"base": "2", "exponent": "10"}),
            lambda v: envelope_result(v, "POWER") == "1024")
    # Negative exponents delegate to the exact evaluator — `2^-3 = 0.125`.
    r.check("power", "2^-3 via exact fallback", c("power", {"base": "2", "exponent": "-3"}),
            lambda v: envelope_result(v, "POWER") == "0.125")
    # Fractional exponents likewise use the 128-bit exact evaluator.
    r.check("power", "2^0.5 via exact fallback", c("power", {"base": "2", "exponent": "0.5"}),
            lambda v: envelope_result(v, "POWER").startswith("1.41421356237"))
    r.check("modulo", "10 %% 3", c("modulo", {"first": "10", "second": "3"}),
            lambda v: envelope_result(v, "MODULO") == "1")
    r.check("abs", "-5", c("abs", {"value": "-5"}),
            lambda v: envelope_result(v, "ABS") == "5")


def test_scientific(r: TestRunner) -> None:
    r.category("scientific", 7)
    c = r.client.call
    r.check("sqrt", "16", c("sqrt", {"number": 16.0}),
            lambda v: TestRunner.close(envelope_result(v, "SQRT"), 4.0))
    r.check("log", "e", c("log", {"number": 2.718281828459045}),
            lambda v: TestRunner.close(envelope_result(v, "LOG"), 1.0, 1e-6))
    r.check("log10", "100", c("log10", {"number": 100.0}),
            lambda v: TestRunner.close(envelope_result(v, "LOG10"), 2.0, 1e-6))
    r.check("factorial", "5", c("factorial", {"num": 5}),
            lambda v: envelope_result(v, "FACTORIAL") == "120")
    r.check("sin", "30deg", c("sin", {"degrees": 30.0}),
            lambda v: TestRunner.close(envelope_result(v, "SIN"), 0.5, 1e-9))
    r.check("cos", "60deg", c("cos", {"degrees": 60.0}),
            lambda v: TestRunner.close(envelope_result(v, "COS"), 0.5, 1e-9))
    r.check("tan", "45deg", c("tan", {"degrees": 45.0}),
            lambda v: TestRunner.close(envelope_result(v, "TAN"), 1.0, 1e-9))


def test_programmable(r: TestRunner) -> None:
    r.category("programmable", 6)
    c = r.client.call
    r.check("evaluate", "2+3*4", c("evaluate", {"expression": "2+3*4"}),
            lambda v: TestRunner.close(envelope_result(v, "EVALUATE"), 14.0, 1e-9))
    r.check("evaluateWithVariables", "2*x+y", c(
        "evaluateWithVariables",
        {"expression": "2*x + y", "variables": '{"x":3,"y":1}'},
    ), lambda v: TestRunner.close(envelope_result(v, "EVALUATE_WITH_VARIABLES"), 7.0, 1e-9))
    r.check("evaluateExact", "0.1+0.2",
            c("evaluateExact", {"expression": "0.1 + 0.2"}),
            lambda v: envelope_result(v, "EVALUATE_EXACT") == "0.3")
    # Uses `my_pi` rather than `pi` because constant names are reserved
    # (regression for evaluateWithVariables pi-override bug).
    r.check("evaluateExactWithVariables", "my_pi*2",
            c("evaluateExactWithVariables",
              {"expression": "my_pi * 2",
               "variables": '{"my_pi":"3.1415926535897932384626433"}'}),
            lambda v: envelope_result(v, "EVALUATE_EXACT_WITH_VARIABLES")
                      == "6.2831853071795864769252866")
    # Regression: evaluateWithVariables must reject variable names that shadow
    # engine constants (pi/e/tau/phi).
    r.check("evaluateWithVariables", "rejects pi override",
            c("evaluateWithVariables",
              {"expression": "pi * 2", "variables": '{"pi": 100}'}),
            lambda v: envelope_error(v, "EVALUATE_WITH_VARIABLES", "INVALID_INPUT"))
    # Regression: exact evaluator must surface DOMAIN_ERROR instead of silently
    # returning 0 when a transcendental leaves its real-valued domain.
    r.check("evaluateExact", "sqrt(-2) -> DOMAIN_ERROR",
            c("evaluateExact", {"expression": "sqrt(-2)"}),
            lambda v: envelope_error(v, "EVALUATE_EXACT", "DOMAIN_ERROR")
                      and envelope_field(v, "DETAIL") == "op=sqrt, value=-2")
    r.check("evaluateExact", "log(0) -> DOMAIN_ERROR",
            c("evaluateExact", {"expression": "log(0)"}),
            lambda v: envelope_error(v, "EVALUATE_EXACT", "DOMAIN_ERROR")
                      and envelope_field(v, "DETAIL") == "op=log, value=0")


def test_vector(r: TestRunner) -> None:
    r.category("vector", 4)
    c = r.client.call
    r.check("sumArray", "1..5", c("sumArray", {"numbers": "1,2,3,4,5"}),
            lambda v: TestRunner.close(envelope_result(v, "SUM_ARRAY"), 15.0, 1e-9))
    r.check("dotProduct", "[1,2,3].[4,5,6]", c(
        "dotProduct", {"first": "1,2,3", "second": "4,5,6"}),
            lambda v: TestRunner.close(envelope_result(v, "DOT_PRODUCT"), 32.0, 1e-9))
    r.check("scaleArray", "[1,2,3]*2", c(
        "scaleArray", {"numbers": "1,2,3", "scalar": "2"}),
            lambda v: envelope_ok(v, "SCALE_ARRAY")
            and [float(x) for x in (envelope_field(v, "RESULT") or "").split(",")]
                == [2.0, 4.0, 6.0])
    r.check("magnitudeArray", "[3,4]", c("magnitudeArray", {"numbers": "3,4"}),
            lambda v: TestRunner.close(envelope_result(v, "MAGNITUDE_ARRAY"), 5.0, 1e-9))


def test_financial(r: TestRunner) -> None:
    r.category("financial", 9)
    c = r.client.call
    r.check("compoundInterest", "1000@5%/10y/12", c("compoundInterest", {
        "principal": "1000", "annualRate": "5", "years": "10", "compoundsPerYear": 12
    }), lambda v: TestRunner.close(envelope_result(v, "COMPOUND_INTEREST"), 1647.009497, 1.0))
    r.check("loanPayment", "100k@5%/30y", c("loanPayment", {
        "principal": "100000", "annualRate": "5", "years": "30"
    }), lambda v: TestRunner.close(envelope_result(v, "LOAN_PAYMENT"), 536.82, 2.0))
    r.check("presentValue", "fv=1000@5%/10y", c("presentValue", {
        "futureValue": "1000", "annualRate": "5", "years": "10"
    }), lambda v: TestRunner.close(envelope_result(v, "PRESENT_VALUE"), 613.91, 2.0))
    r.check("futureValueAnnuity", "100@5%/10y", c("futureValueAnnuity", {
        "payment": "100", "annualRate": "5", "years": "10"
    }), lambda v: TestRunner.close(envelope_result(v, "FUTURE_VALUE_ANNUITY"), 1257.79, 3.0))
    # ROI = (gain - cost) / cost * 100. gain=1200, cost=1000 -> 20%.
    r.check("returnOnInvestment", "gain=1200/cost=1000", c("returnOnInvestment", {
        "gain": "1200", "cost": "1000"
    }), lambda v: TestRunner.close(envelope_result(v, "RETURN_ON_INVESTMENT"), 20.0, 1e-6))

    schedule = c("amortizationSchedule", {
        "principal": "10000", "annualRate": "5", "years": "1"
    })
    r.check("amortizationSchedule", "10k@5%/1y", schedule,
            lambda v: envelope_ok(v, "AMORTIZATION_SCHEDULE")
            and envelope_field(v, "ROW_1") is not None,
            detail_render=lambda v: f"header_ok={envelope_ok(v, 'AMORTIZATION_SCHEDULE')}, "
                                    f"row_1={envelope_field(v, 'ROW_1')!s:.60}")

    # Regression: the schedule must refuse terms that would exceed 480 months
    # (40-year cap) — a 50y run at 100% previously produced ~52 KB of output
    # that exceeded the MCP client token limit.
    r.check("amortizationSchedule", "50y exceeds 480-month cap",
            c("amortizationSchedule", {"principal": "1000",
                                       "annualRate": "5", "years": "50"}),
            lambda v: envelope_error(v, "AMORTIZATION_SCHEDULE", "OUT_OF_RANGE")
            and "max=480" in v)

    # Regression: financial error envelopes must now use lowercase reason text
    # and camelCase DETAIL keys (previously "Principal" / "annual rate=-5").
    r.check("compoundInterest", "negative principal -> lowercase + detail",
            c("compoundInterest", {"principal": "-100", "annualRate": "5",
                                   "years": "1", "compoundsPerYear": 1}),
            lambda v: envelope_error(v, "COMPOUND_INTEREST", "INVALID_INPUT")
                      and "principal must be greater than zero"
                          in (envelope_field(v, "REASON") or "")
                      and envelope_field(v, "DETAIL") == "principal=-100")
    r.check("compoundInterest", "negative rate -> annualRate in DETAIL",
            c("compoundInterest", {"principal": "1000", "annualRate": "-5",
                                   "years": "1", "compoundsPerYear": 12}),
            lambda v: envelope_error(v, "COMPOUND_INTEREST", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "annualRate=-5")
    r.check("compoundInterest", "zero compounds -> compoundsPerYear in DETAIL",
            c("compoundInterest", {"principal": "1000", "annualRate": "5",
                                   "years": "1", "compoundsPerYear": 0}),
            lambda v: envelope_error(v, "COMPOUND_INTEREST", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "compoundsPerYear=0")


def test_calculus(r: TestRunner) -> None:
    r.category("calculus", 5)
    c = r.client.call
    r.check("derivative", "x^2 at 3", c("derivative", {
        "expression": "x^2", "variable": "x", "point": 3.0
    }), lambda v: TestRunner.close(envelope_result(v, "DERIVATIVE"), 6.0, 1e-4))
    # Regression: calculus error envelopes must match the normalized format
    # used by programmable (lowercase reason + DETAIL line). The tool layer
    # rewrites UnknownVariable when the expression uses a name other than the
    # declared variable — surfacing the real cause instead of the raw
    # evaluator's `name=<other>` suffix.
    r.check("derivative", "unknown var -> normalized envelope",
            c("derivative", {"expression": "y + 1", "variable": "x", "point": 0.0}),
            lambda v: envelope_error(v, "DERIVATIVE", "UNKNOWN_VARIABLE")
                      and envelope_field(v, "DETAIL") == "found=y, declared=x")
    r.check("nthDerivative", "x^3 n=2 at 2", c("nthDerivative", {
        "expression": "x^3", "variable": "x", "point": 2.0, "order": 2
    }), lambda v: TestRunner.close(envelope_result(v, "NTH_DERIVATIVE"), 12.0, 1e-2))
    r.check("definiteIntegral", "x^2 [0,1]", c("definiteIntegral", {
        "expression": "x^2", "variable": "x", "lower": 0.0, "upper": 1.0
    }), lambda v: TestRunner.close(envelope_result(v, "DEFINITE_INTEGRAL"), 1.0 / 3.0, 1e-5))
    tangent = c("tangentLine", {"expression": "x^2", "variable": "x", "point": 3.0})
    r.check("tangentLine", "x^2 at 3", tangent,
            lambda v: envelope_ok(v, "TANGENT_LINE")
            and TestRunner.close(envelope_field(v, "SLOPE"), 6.0, 1e-3)
            and TestRunner.close(envelope_field(v, "INTERCEPT"), -9.0, 1e-3),
            detail_render=lambda v: f"slope={envelope_field(v, 'SLOPE')}, "
                                    f"intercept={envelope_field(v, 'INTERCEPT')}")

    # Regression: derivative/nthDerivative/tangentLine used to quietly return
    # huge spurious values at singularities because central differences only
    # sample point±h. e.g. d/dx(1/x) at x=0 used to yield ~1.25e12.
    r.check("derivative", "1/x at 0 -> DOMAIN_ERROR",
            c("derivative", {"expression": "1/x", "variable": "x", "point": 0.0}),
            lambda v: envelope_error(v, "DERIVATIVE", "DOMAIN_ERROR")
                      and "function is not defined" in (envelope_field(v, "REASON") or ""))
    r.check("nthDerivative", "1/x at 0 order=2 -> DOMAIN_ERROR",
            c("nthDerivative", {"expression": "1/x", "variable": "x",
                                "point": 0.0, "order": 2}),
            lambda v: envelope_error(v, "NTH_DERIVATIVE", "DOMAIN_ERROR"))
    r.check("tangentLine", "1/x at 0 -> DOMAIN_ERROR",
            c("tangentLine", {"expression": "1/x", "variable": "x", "point": 0.0}),
            lambda v: envelope_error(v, "TANGENT_LINE", "DOMAIN_ERROR"))
    # Sanity: derivative near (but not at) the singularity must still work.
    r.check("derivative", "1/x at 1 -> -1",
            c("derivative", {"expression": "1/x", "variable": "x", "point": 1.0}),
            lambda v: envelope_ok(v, "DERIVATIVE")
                      and TestRunner.close(envelope_result(v, "DERIVATIVE"), -1.0, 1e-4))


def test_unit_converter(r: TestRunner) -> None:
    r.category("unit converter", 2)
    c = r.client.call
    r.check("convert", "1km->mi", c("convert", {
        "value": "1", "fromUnit": "km", "toUnit": "mi", "category": "LENGTH"
    }), lambda v: envelope_ok(v, "CONVERT")
       and (envelope_field(v, "RESULT") or "").startswith("0.6213711922"))
    r.check("convertAutoDetect", "100c->f", c("convertAutoDetect", {
        "value": "100", "fromUnit": "c", "toUnit": "f"
    }), lambda v: TestRunner.close(envelope_result(v, "CONVERT_AUTO_DETECT"), 212.0, 1e-6))

    # Regression: `B` is the SI symbol for byte. Users writing `KB` / `MB`
    # / `GB` expect a standalone `B` to resolve to byte; a missing alias
    # surprised callers with `unit is not a recognized unit`.
    r.check("convertAutoDetect", "KB -> B alias to byte",
            c("convertAutoDetect", {"value": "1", "fromUnit": "KB", "toUnit": "B"}),
            lambda v: TestRunner.close(envelope_result(v, "CONVERT_AUTO_DETECT"), 1000.0, 1e-6))

    # Regression: physical quantities (length, mass, volume, area, density,
    # time, data storage/rate, frequency, R/L/C) must reject negatives.
    # Signed categories (temperature, voltage, current, speed, energy, force,
    # power, pressure, angle) must still allow negative values.
    r.check("convert", "negative length -> INVALID_INPUT",
            c("convert", {"value": "-100", "fromUnit": "km",
                          "toUnit": "mi", "category": "LENGTH"}),
            lambda v: envelope_error(v, "CONVERT", "INVALID_INPUT")
                      and "value must not be negative" in (envelope_field(v, "REASON") or "")
                      and envelope_field(v, "DETAIL") == "value=-100, category=LENGTH")
    r.check("convert", "negative mass -> INVALID_INPUT",
            c("convert", {"value": "-5", "fromUnit": "kg",
                          "toUnit": "g", "category": "MASS"}),
            lambda v: envelope_error(v, "CONVERT", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "value=-5, category=MASS")
    r.check("convert", "negative temperature -> OK",
            c("convert", {"value": "-40", "fromUnit": "c",
                          "toUnit": "f", "category": "TEMPERATURE"}),
            lambda v: envelope_ok(v, "CONVERT")
                      and envelope_field(v, "RESULT") == "-40")
    r.check("convert", "negative voltage -> OK",
            c("convert", {"value": "-5", "fromUnit": "vlt",
                          "toUnit": "mvlt", "category": "VOLTAGE"}),
            lambda v: envelope_ok(v, "CONVERT"))
    r.check("convertAutoDetect", "negative length -> INVALID_INPUT",
            c("convertAutoDetect", {"value": "-10", "fromUnit": "km",
                                    "toUnit": "mi"}),
            lambda v: envelope_error(v, "CONVERT_AUTO_DETECT", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "value=-10, category=LENGTH")
    r.check("convertAutoDetect", "negative celsius -> OK",
            c("convertAutoDetect", {"value": "-10", "fromUnit": "c",
                                    "toUnit": "f"}),
            lambda v: envelope_ok(v, "CONVERT_AUTO_DETECT"))


def test_cooking(r: TestRunner) -> None:
    r.category("cooking", 3)
    c = r.client.call
    vol = c("convertCookingVolume", {"value": "1", "fromUnit": "uscup", "toUnit": "tbsp"})
    r.check("convertCookingVolume", "1 uscup -> tbsp", vol,
            lambda v: TestRunner.close(envelope_result(v, "CONVERT_COOKING_VOLUME"), 16.0, 0.5))
    r.check("convertCookingWeight", "1 lb -> oz", c("convertCookingWeight", {
        "value": "1", "fromUnit": "lb", "toUnit": "oz"
    }), lambda v: TestRunner.close(envelope_result(v, "CONVERT_COOKING_WEIGHT"), 16.0, 1e-6))
    r.check("convertOvenTemperature", "gasmark 4 -> c", c("convertOvenTemperature", {
        "value": "4", "fromUnit": "gasmark", "toUnit": "c"
    }), lambda v: TestRunner.close(envelope_result(v, "CONVERT_OVEN_TEMPERATURE"), 180.0, 1e-6))

    # Regression: cooking measurements are strictly non-negative. Negative
    # values used to silently round-trip through the unit registry (e.g.
    # -1 cup → -236.59 ml).
    r.check("convertCookingVolume", "negative value -> INVALID_INPUT",
            c("convertCookingVolume", {"value": "-1",
                                       "fromUnit": "cup",
                                       "toUnit": "ml"}),
            lambda v: envelope_error(v, "CONVERT_COOKING_VOLUME", "INVALID_INPUT")
                      and "value must not be negative" in (envelope_field(v, "REASON") or "")
                      and envelope_field(v, "DETAIL") == "value=-1")
    r.check("convertCookingWeight", "negative value -> INVALID_INPUT",
            c("convertCookingWeight", {"value": "-5",
                                       "fromUnit": "kg",
                                       "toUnit": "g"}),
            lambda v: envelope_error(v, "CONVERT_COOKING_WEIGHT", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "value=-5")
    # Oven temperature must remain permissive: -10°C → 14°F is valid.
    r.check("convertOvenTemperature", "negative celsius allowed",
            c("convertOvenTemperature", {"value": "-10",
                                         "fromUnit": "c",
                                         "toUnit": "f"}),
            lambda v: envelope_ok(v, "CONVERT_OVEN_TEMPERATURE")
                      and envelope_field(v, "RESULT") == "14")


def test_measure_reference(r: TestRunner) -> None:
    r.category("measure reference", 5)
    c = r.client.call
    cats = c("listCategories", {})
    r.check("listCategories", "", cats,
            lambda v: envelope_ok(v, "LIST_CATEGORIES")
            and (envelope_field(v, "COUNT") is not None
                 or envelope_field(v, "RESULT") is not None),
            detail_render=lambda v: f"count={envelope_field(v, 'COUNT')}, "
                                    f"result={envelope_field(v, 'RESULT')!s:.60}")

    # Tool-module categories (23) — distinct from the 21 measurement-unit
    # categories above. This tool was added so callers can discover the
    # taxonomy programmatically instead of parsing the server-info string.
    tool_cats = c("listToolCategories", {})
    r.check("listToolCategories", "", tool_cats,
            lambda v: envelope_ok(v, "LIST_TOOL_CATEGORIES")
            and envelope_field(v, "COUNT") == "23"
            and "Basic" in (envelope_field(v, "VALUES") or ""))

    units = c("listUnits", {"category": "LENGTH"})
    r.check("listUnits", "LENGTH", units,
            lambda v: envelope_ok(v, "LIST_UNITS")
            and "m" in (envelope_field(v, "VALUES") or ""),
            detail_render=lambda v: f"values={envelope_field(v, 'VALUES')!s:.60}")

    r.check("getConversionFactor", "km->m", c("getConversionFactor", {
        "fromUnit": "km", "toUnit": "m"
    }), lambda v: TestRunner.close(envelope_result(v, "GET_CONVERSION_FACTOR"), 1000.0, 1e-6))

    r.check("explainConversion", "c->f", c("explainConversion", {
        "fromUnit": "c", "toUnit": "f"
    }), lambda v: envelope_ok(v, "EXPLAIN_CONVERSION")
       and "F = C * 9/5 + 32" in (envelope_field(v, "RESULT")
                                  or envelope_field(v, "FORMULA")
                                  or ""))


def test_datetime(r: TestRunner) -> None:
    r.category("datetime", 5)
    c = r.client.call
    tz = c("convertTimezone", {
        "datetime": "2026-03-03T12:00:00",
        "fromTimezone": "UTC",
        "toTimezone": "Asia/Tokyo",
    })
    r.check("convertTimezone", "UTC->Tokyo", tz,
            lambda v: envelope_ok(v, "CONVERT_TIMEZONE")
            and "21:00:00" in (envelope_field(v, "DATETIME") or ""))

    r.check("formatDateTime", "epoch->iso", c("formatDateTime", {
        "datetime": "1709424000",
        "inputFormat": "epoch",
        "outputFormat": "iso-offset",
        "timezone": "UTC",
    }), lambda v: envelope_ok(v, "FORMAT_DATETIME")
       and "2024-03-03" in (envelope_field(v, "RESULT") or ""))

    # Regression: output format without any `%` strftime token used to be
    # echoed as literal text (e.g. outputFormat="invalid_format" returned
    # RESULT: invalid_format). Must now surface INVALID_INPUT.
    r.check("formatDateTime", "unknown keyword -> INVALID_INPUT",
            c("formatDateTime", {"datetime": "2026-04-22T10:30:00Z",
                                 "inputFormat": "iso",
                                 "outputFormat": "invalid_format",
                                 "timezone": "UTC"}),
            lambda v: envelope_error(v, "FORMAT_DATETIME", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "format=invalid_format")
    r.check("formatDateTime", "empty output -> INVALID_INPUT",
            c("formatDateTime", {"datetime": "2026-04-22T10:30:00Z",
                                 "inputFormat": "iso",
                                 "outputFormat": "",
                                 "timezone": "UTC"}),
            lambda v: envelope_error(v, "FORMAT_DATETIME", "INVALID_INPUT"))
    # Sanity: strftime with `%` tokens must still work
    r.check("formatDateTime", "strftime %Y-%m-%d works",
            c("formatDateTime", {"datetime": "2026-04-22T10:30:00Z",
                                 "inputFormat": "iso",
                                 "outputFormat": "%Y-%m-%d",
                                 "timezone": "UTC"}),
            lambda v: envelope_ok(v, "FORMAT_DATETIME")
                      and envelope_field(v, "RESULT") == "2026-04-22")

    now = c("currentDateTime", {"timezone": "UTC", "format": "iso"})
    r.check("currentDateTime", "UTC iso", now,
            lambda v: envelope_ok(v, "CURRENT_DATE_TIME")
            and "T" in (envelope_field(v, "RESULT") or ""))

    tzs = c("listTimezones", {"region": "Europe"})
    r.check("listTimezones", "Europe", tzs,
            lambda v: envelope_ok(v, "LIST_TIMEZONES")
            and "Europe/Paris" in (envelope_field(v, "VALUES") or ""),
            detail_render=lambda v: f"values={envelope_field(v, 'VALUES')!s:.60}")

    diff = c("dateTimeDifference", {
        "datetime1": "2026-01-01T00:00:00",
        "datetime2": "2026-03-03T15:30:00",
        "timezone": "UTC",
    })
    r.check("dateTimeDifference", "", diff,
            lambda v: envelope_ok(v, "DATETIME_DIFFERENCE")
            and float(envelope_field(v, "TOTAL_SECONDS") or 0) > 0,
            detail_render=lambda v: f"totalSeconds={envelope_field(v, 'TOTAL_SECONDS')}")


def test_printing(r: TestRunner) -> None:
    r.category("printing", 1)
    c = r.client.call
    tape = c("calculateWithTape", {
        "operations": '[{"op":"+","value":"100"},{"op":"-","value":"30"},{"op":"=","value":null}]'
    })
    r.check("calculateWithTape", "100-30", tape,
            lambda v: envelope_ok(v, "CALCULATE_WITH_TAPE") and "70" in v)


def test_graphing(r: TestRunner) -> None:
    r.category("graphing", 3)
    c = r.client.call
    pts = c("plotFunction", {
        "expression": "x^2", "variable": "x", "min": -2.0, "max": 2.0, "steps": 4
    })
    r.check("plotFunction", "x^2 [-2,2]", pts,
            lambda v: envelope_ok(v, "PLOT_FUNCTION")
            and envelope_field(v, "ROW_1") is not None,
            detail_render=lambda v: f"row_1={envelope_field(v, 'ROW_1')!s:.60}")

    root = c("solveEquation", {
        "expression": "x^2 - 4", "variable": "x", "initialGuess": 3.0
    })
    r.check("solveEquation", "x^2-4 near 3", root,
            lambda v: TestRunner.close(envelope_result(v, "SOLVE_EQUATION"), 2.0, 1e-4))

    roots = c("findRoots", {
        "expression": "x^2 - 4", "variable": "x", "min": -5.0, "max": 5.0
    })
    def roots_ok(v):
        if not envelope_ok(v, "FIND_ROOTS"):
            return False
        raw = envelope_field(v, "VALUES")
        if not raw:
            return False
        try:
            vals = sorted(float(x.strip()) for x in raw.split(",") if x.strip())
        except ValueError:
            return False
        if len(vals) < 2:
            return False
        return TestRunner.close(vals[0], -2.0, 0.1) and TestRunner.close(vals[-1], 2.0, 0.1)
    r.check("findRoots", "x^2-4 [-5,5]", roots, roots_ok,
            detail_render=lambda v: f"values={envelope_field(v, 'VALUES')}")


def test_network(r: TestRunner) -> None:
    r.category("network", 22)
    c = r.client.call

    subnet = c("subnetCalculator", {"address": "192.168.1.0", "cidr": 24})
    r.check("subnetCalculator", "192.168.1.0/24", subnet,
            lambda v: envelope_ok(v, "SUBNET_CALCULATOR")
            and envelope_field(v, "NETWORK") == "192.168.1.0"
            and int(envelope_field(v, "USABLE_HOSTS") or 0) == 254
            and envelope_field(v, "IP_CLASS") == "C",
            detail_render=lambda v: f"network={envelope_field(v, 'NETWORK')}, "
                                    f"usableHosts={envelope_field(v, 'USABLE_HOSTS')}, "
                                    f"ipClass={envelope_field(v, 'IP_CLASS')}")

    r.check("ipToBinary", "192.168.1.1", c("ipToBinary", {"address": "192.168.1.1"}),
            lambda v: envelope_result(v, "IP_TO_BINARY")
            == "11000000.10101000.00000001.00000001")

    r.check("binaryToIp", "192.168.1.1", c("binaryToIp", {
        "binary": "11000000.10101000.00000001.00000001"
    }), lambda v: envelope_result(v, "BINARY_TO_IP") == "192.168.1.1")

    r.check("ipToDecimal", "192.168.1.1", c("ipToDecimal", {"address": "192.168.1.1"}),
            lambda v: envelope_result(v, "IP_TO_DECIMAL") == "3232235777")

    r.check("decimalToIp", "3232235777", c("decimalToIp", {
        "decimal": "3232235777", "version": 4
    }), lambda v: envelope_result(v, "DECIMAL_TO_IP") == "192.168.1.1")

    r.check("ipInSubnet", "100 in /24", c("ipInSubnet", {
        "address": "192.168.1.100", "network": "192.168.1.0", "cidr": 24
    }), lambda v: envelope_ok(v, "IP_IN_SUBNET")
       and envelope_field(v, "IN_SUBNET") in ("true", "True"))

    vlsm = c("vlsmSubnets", {
        "networkCidr": "192.168.1.0/24",
        "hostCounts": "[50,25,10]",
    })
    r.check("vlsmSubnets", "3 subnets", vlsm,
            lambda v: envelope_ok(v, "VLSM_SUBNETS")
            and envelope_field(v, "ROW_1") is not None,
            detail_render=lambda v: f"row_1={envelope_field(v, 'ROW_1')!s:.60}")

    # Regression: VLSM must reject invalid host-count arrays (empty, zero,
    # negative) instead of silently producing nonsensical allocations.
    r.check("vlsmSubnets", "empty hostCounts -> INVALID_INPUT",
            c("vlsmSubnets", {"networkCidr": "192.168.1.0/24", "hostCounts": "[]"}),
            lambda v: envelope_error(v, "VLSM_SUBNETS", "INVALID_INPUT")
                      and "must not be empty" in (envelope_field(v, "REASON") or ""))
    r.check("vlsmSubnets", "zero hostCount -> INVALID_INPUT",
            c("vlsmSubnets", {"networkCidr": "192.168.1.0/24", "hostCounts": "[0]"}),
            lambda v: envelope_error(v, "VLSM_SUBNETS", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "hosts=0")
    r.check("vlsmSubnets", "negative hostCount -> INVALID_INPUT",
            c("vlsmSubnets", {"networkCidr": "192.168.1.0/24", "hostCounts": "[-10]"}),
            lambda v: envelope_error(v, "VLSM_SUBNETS", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "hosts=-10")

    summary = c("summarizeSubnets", {
        "subnets": '["192.168.0.0/25","192.168.0.128/25"]'
    })
    r.check("summarizeSubnets", "two /25s", summary,
            lambda v: envelope_result(v, "SUMMARIZE_SUBNETS") == "192.168.0.0/24")

    r.check("expandIpv6", "::1", c("expandIpv6", {"address": "::1"}),
            lambda v: envelope_result(v, "EXPAND_IPV6")
            == "0000:0000:0000:0000:0000:0000:0000:0001")

    # RFC 4291 §2.5.5.2 IPv4-mapped form: the embedded v4 tail must stay
    # dotted so the original address is readable (`c0a8:0101` obscures it).
    r.check("expandIpv6", "v4-mapped keeps dotted quad",
            c("expandIpv6", {"address": "::ffff:192.168.1.1"}),
            lambda v: envelope_result(v, "EXPAND_IPV6")
            == "0000:0000:0000:0000:0000:ffff:192.168.1.1")

    r.check("compressIpv6", "2001:db8::1", c("compressIpv6", {
        "address": "2001:0db8:0000:0000:0000:0000:0000:0001"
    }), lambda v: envelope_result(v, "COMPRESS_IPV6") == "2001:db8::1")

    tt = c("transferTime", {
        "fileSize": "1", "fileSizeUnit": "gb",
        "bandwidth": "100", "bandwidthUnit": "mbps",
    })
    r.check("transferTime", "1GB/100Mbps", tt,
            lambda v: envelope_ok(v, "TRANSFER_TIME")
            and envelope_field(v, "SECONDS") is not None,
            detail_render=lambda v: f"seconds={envelope_field(v, 'SECONDS')}")

    thr = c("throughput", {
        "dataSize": "100", "dataSizeUnit": "mb",
        "time": "10", "timeUnit": "s", "outputUnit": "mbps",
    })
    r.check("throughput", "100MB/10s->mbps", thr,
            lambda v: envelope_ok(v, "THROUGHPUT")
            and float(envelope_field(v, "RATE") or 0) > 0)

    tcp = c("tcpThroughput", {
        "bandwidthMbps": "1000", "rttMs": "100", "windowSizeKb": "64"
    })
    r.check("tcpThroughput", "1Gbps/100ms/64kB", tcp,
            lambda v: envelope_ok(v, "TCP_THROUGHPUT")
            and float(envelope_field(v, "RATE_MBPS") or 0) > 0)

    # Regression: transferTime must reject negative/zero physical inputs
    r.check("transferTime", "negative fileSize -> INVALID_INPUT",
            c("transferTime", {"fileSize": "-1", "fileSizeUnit": "gb",
                               "bandwidth": "100", "bandwidthUnit": "mbps"}),
            lambda v: envelope_error(v, "TRANSFER_TIME", "INVALID_INPUT")
                      and "file size must not be negative" in (envelope_field(v, "REASON") or ""))
    r.check("transferTime", "negative bandwidth -> INVALID_INPUT",
            c("transferTime", {"fileSize": "1", "fileSizeUnit": "gb",
                               "bandwidth": "-100", "bandwidthUnit": "mbps"}),
            lambda v: envelope_error(v, "TRANSFER_TIME", "INVALID_INPUT")
                      and "bandwidth must be positive" in (envelope_field(v, "REASON") or ""))
    r.check("transferTime", "zero bandwidth -> INVALID_INPUT",
            c("transferTime", {"fileSize": "1", "fileSizeUnit": "gb",
                               "bandwidth": "0", "bandwidthUnit": "mbps"}),
            lambda v: envelope_error(v, "TRANSFER_TIME", "INVALID_INPUT")
                      and "bandwidth must be positive" in (envelope_field(v, "REASON") or ""))

    # Regression: throughput must reject negative/zero physical inputs
    r.check("throughput", "negative dataSize -> INVALID_INPUT",
            c("throughput", {"dataSize": "-500", "dataSizeUnit": "mb",
                             "time": "10", "timeUnit": "s", "outputUnit": "mbps"}),
            lambda v: envelope_error(v, "THROUGHPUT", "INVALID_INPUT")
                      and "data size must not be negative" in (envelope_field(v, "REASON") or ""))
    r.check("throughput", "negative time -> INVALID_INPUT",
            c("throughput", {"dataSize": "500", "dataSizeUnit": "mb",
                             "time": "-10", "timeUnit": "s", "outputUnit": "mbps"}),
            lambda v: envelope_error(v, "THROUGHPUT", "INVALID_INPUT")
                      and "time must be positive" in (envelope_field(v, "REASON") or ""))
    r.check("throughput", "zero time -> INVALID_INPUT",
            c("throughput", {"dataSize": "500", "dataSizeUnit": "mb",
                             "time": "0", "timeUnit": "s", "outputUnit": "mbps"}),
            lambda v: envelope_error(v, "THROUGHPUT", "INVALID_INPUT")
                      and "time must be positive" in (envelope_field(v, "REASON") or ""))


def test_analog(r: TestRunner) -> None:
    r.category("analog electronics", 26)
    c = r.client.call

    # Regression: physical positivity invariants on R/C must be enforced
    # instead of quietly returning a negative time constant.
    r.check("rcTimeConstant", "negative R -> INVALID_INPUT",
            c("rcTimeConstant", {"resistance": "-1000", "capacitance": "0.000001"}),
            lambda v: envelope_error(v, "RC_TIME_CONSTANT", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "resistance=-1000")
    r.check("filterCutoff", "negative reactive -> INVALID_INPUT",
            c("filterCutoff", {"resistance": "1000", "reactive": "-0.000001",
                               "filterType": "lowpass"}),
            lambda v: envelope_error(v, "FILTER_CUTOFF", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "capacitance=-0.000001")

    ohms = c("ohmsLaw", {"voltage": "12", "current": "2", "resistance": "", "power": ""})
    r.check("ohmsLaw", "V=12 I=2", ohms,
            lambda v: envelope_ok(v, "OHMS_LAW")
            and TestRunner.close(envelope_field(v, "RESISTANCE"), 6.0, 1e-6),
            detail_render=lambda v: f"R={envelope_field(v, 'RESISTANCE')}, "
                                    f"P={envelope_field(v, 'POWER')}")

    r.check("resistorCombination", "series 10,20,30", c("resistorCombination", {
        "values": "10,20,30", "mode": "series"
    }), lambda v: TestRunner.close(envelope_result(v, "RESISTOR_COMBINATION"), 60.0, 1e-6))

    r.check("capacitorCombination", "parallel 10,20", c("capacitorCombination", {
        "values": "10,20", "mode": "parallel"
    }), lambda v: TestRunner.close(envelope_result(v, "CAPACITOR_COMBINATION"), 30.0, 1e-6))

    r.check("inductorCombination", "series 5,10", c("inductorCombination", {
        "values": "5,10", "mode": "series"
    }), lambda v: TestRunner.close(envelope_result(v, "INDUCTOR_COMBINATION"), 15.0, 1e-6))

    r.check("voltageDivider", "10, 1k, 1k", c("voltageDivider", {
        "vin": "10", "r1": "1000", "r2": "1000"
    }), lambda v: envelope_ok(v, "VOLTAGE_DIVIDER")
       and TestRunner.close(envelope_field(v, "VOUT"), 5.0, 1e-6))

    cdiv = c("currentDivider", {"totalCurrent": "2", "r1": "1000", "r2": "1000"})
    r.check("currentDivider", "2A split", cdiv,
            lambda v: envelope_ok(v, "CURRENT_DIVIDER")
            and envelope_field(v, "I1") is not None
            and envelope_field(v, "I2") is not None,
            detail_render=lambda v: f"i1={envelope_field(v, 'I1')}, "
                                    f"i2={envelope_field(v, 'I2')}")

    rc = c("rcTimeConstant", {"resistance": "1000", "capacitance": "0.000001"})
    r.check("rcTimeConstant", "1k, 1uF", rc,
            lambda v: envelope_ok(v, "RC_TIME_CONSTANT")
            and TestRunner.close(envelope_field(v, "TAU"), 0.001, 1e-9),
            detail_render=lambda v: f"tau={envelope_field(v, 'TAU')}")

    rl = c("rlTimeConstant", {"resistance": "10", "inductance": "0.001"})
    r.check("rlTimeConstant", "10, 1mH", rl,
            lambda v: envelope_ok(v, "RL_TIME_CONSTANT")
            and envelope_field(v, "TAU") is not None,
            detail_render=lambda v: f"tau={envelope_field(v, 'TAU')}")

    rlc = c("rlcResonance", {"r": "10", "l": "0.001", "c": "0.000001"})
    r.check("rlcResonance", "", rlc,
            lambda v: envelope_ok(v, "RLC_RESONANCE")
            and envelope_field(v, "RESONANT_FREQUENCY") is not None,
            detail_render=lambda v: f"fr={envelope_field(v, 'RESONANT_FREQUENCY')}")

    imp = c("impedance", {
        "r": "100", "l": "0.001", "c": "0.000001", "frequency": "1000"
    })
    r.check("impedance", "RLC @ 1kHz", imp,
            lambda v: envelope_ok(v, "IMPEDANCE")
            and envelope_field(v, "MAGNITUDE") is not None
            and envelope_field(v, "PHASE_DEG") is not None,
            detail_render=lambda v: f"|Z|={envelope_field(v, 'MAGNITUDE')}, "
                                    f"ph={envelope_field(v, 'PHASE_DEG')}")

    db = c("decibelConvert", {"value": "100", "mode": "powerToDb"})
    r.check("decibelConvert", "100 powerToDb", db,
            lambda v: TestRunner.close(envelope_result(v, "DECIBEL_CONVERT"), 20.0, 1e-6))

    fc = c("filterCutoff", {
        "resistance": "1000", "reactive": "0.000001", "filterType": "lowpass"
    })
    r.check("filterCutoff", "RC low-pass", fc,
            lambda v: envelope_ok(v, "FILTER_CUTOFF")
            and envelope_field(v, "CUTOFF_HZ") is not None,
            detail_render=lambda v: f"fc={envelope_field(v, 'CUTOFF_HZ')}")

    # NOTE: LedResistorParams does NOT carry #[serde(rename_all = "camelCase")],
    # so the forward-current field stays as the Rust name `i_f`.
    led = c("ledResistor", {"vs": "5", "vf": "2", "i_f": "0.02"})
    r.check("ledResistor", "5V/2V/20mA", led,
            lambda v: envelope_ok(v, "LED_RESISTOR")
            and TestRunner.close(envelope_field(v, "RESISTANCE"), 150.0, 0.5))

    wh = c("wheatstoneBridge", {"r1": "100", "r2": "200", "r3": "300"})
    r.check("wheatstoneBridge", "R1=100 R2=200 R3=300", wh,
            lambda v: TestRunner.close(envelope_result(v, "WHEATSTONE_BRIDGE"), 600.0, 1e-4))

    # Regression: voltageDivider must reject non-positive resistances. Zero
    # was previously accepted (returning Vout=Vin for R1=0), which is the
    # ideal-short corner case — reject it so callers spot bad inputs.
    r.check("voltageDivider", "negative r1 -> INVALID_INPUT",
            c("voltageDivider", {"vin": "10", "r1": "-100", "r2": "50"}),
            lambda v: envelope_error(v, "VOLTAGE_DIVIDER", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "r1=-100")
    r.check("voltageDivider", "negative r2 -> INVALID_INPUT",
            c("voltageDivider", {"vin": "10", "r1": "100", "r2": "-50"}),
            lambda v: envelope_error(v, "VOLTAGE_DIVIDER", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "r2=-50")
    r.check("voltageDivider", "zero r1 -> INVALID_INPUT",
            c("voltageDivider", {"vin": "5", "r1": "0", "r2": "1000"}),
            lambda v: envelope_error(v, "VOLTAGE_DIVIDER", "INVALID_INPUT")
                      and "r1 must be positive" in (envelope_field(v, "REASON") or ""))
    r.check("voltageDivider", "zero r2 -> INVALID_INPUT",
            c("voltageDivider", {"vin": "5", "r1": "1000", "r2": "0"}),
            lambda v: envelope_error(v, "VOLTAGE_DIVIDER", "INVALID_INPUT")
                      and "r2 must be positive" in (envelope_field(v, "REASON") or ""))

    # Regression: currentDivider must reject non-positive resistances. R=0
    # used to pass silently (I1=Itotal, I2=0), masking upstream bad data.
    r.check("currentDivider", "negative r1 -> INVALID_INPUT",
            c("currentDivider", {"totalCurrent": "5", "r1": "-100", "r2": "50"}),
            lambda v: envelope_error(v, "CURRENT_DIVIDER", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "r1=-100")
    r.check("currentDivider", "negative r2 -> INVALID_INPUT",
            c("currentDivider", {"totalCurrent": "5", "r1": "100", "r2": "-50"}),
            lambda v: envelope_error(v, "CURRENT_DIVIDER", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "r2=-50")
    r.check("currentDivider", "zero r1 -> INVALID_INPUT",
            c("currentDivider", {"totalCurrent": "1", "r1": "0", "r2": "1000"}),
            lambda v: envelope_error(v, "CURRENT_DIVIDER", "INVALID_INPUT")
                      and "r1 must be positive" in (envelope_field(v, "REASON") or ""))
    r.check("currentDivider", "zero r2 -> INVALID_INPUT",
            c("currentDivider", {"totalCurrent": "1", "r1": "1000", "r2": "0"}),
            lambda v: envelope_error(v, "CURRENT_DIVIDER", "INVALID_INPUT")
                      and "r2 must be positive" in (envelope_field(v, "REASON") or ""))

    # Regression: ohmsLaw must reject negative V/I/R/P. Previously V=-5, I=1
    # produced R=-5, P=-5 (non-physical).
    r.check("ohmsLaw", "negative voltage (V,I) -> INVALID_INPUT",
            c("ohmsLaw", {"voltage": "-5", "current": "1",
                          "resistance": "", "power": ""}),
            lambda v: envelope_error(v, "OHMS_LAW", "INVALID_INPUT")
                      and "voltage must not be negative" in (envelope_field(v, "REASON") or ""))
    r.check("ohmsLaw", "negative current (I,R) -> INVALID_INPUT",
            c("ohmsLaw", {"voltage": "", "current": "-2",
                          "resistance": "10", "power": ""}),
            lambda v: envelope_error(v, "OHMS_LAW", "INVALID_INPUT")
                      and "current must not be negative" in (envelope_field(v, "REASON") or ""))
    r.check("ohmsLaw", "negative resistance (V,R) -> INVALID_INPUT",
            c("ohmsLaw", {"voltage": "5", "current": "",
                          "resistance": "-10", "power": ""}),
            lambda v: envelope_error(v, "OHMS_LAW", "INVALID_INPUT")
                      and "resistance must not be negative" in (envelope_field(v, "REASON") or ""))
    r.check("ohmsLaw", "negative power (R,P) -> INVALID_INPUT",
            c("ohmsLaw", {"voltage": "", "current": "",
                          "resistance": "10", "power": "-100"}),
            lambda v: envelope_error(v, "OHMS_LAW", "INVALID_INPUT")
                      and "power must not be negative" in (envelope_field(v, "REASON") or ""))

    # Regression: resistorCombination series mode used to silently accept
    # negative values (e.g. [-100,200,300] → 400). Must now reject.
    r.check("resistorCombination", "series negative -> INVALID_INPUT",
            c("resistorCombination", {"values": "-100,200,300", "mode": "series"}),
            lambda v: envelope_error(v, "RESISTOR_COMBINATION", "INVALID_INPUT")
                      and "must not be negative" in (envelope_field(v, "REASON") or ""))
    r.check("inductorCombination", "series negative -> INVALID_INPUT",
            c("inductorCombination", {"values": "-0.001,0.002", "mode": "series"}),
            lambda v: envelope_error(v, "INDUCTOR_COMBINATION", "INVALID_INPUT")
                      and "must not be negative" in (envelope_field(v, "REASON") or ""))
    r.check("capacitorCombination", "parallel negative -> INVALID_INPUT",
            c("capacitorCombination", {"values": "-1e-6,2e-6", "mode": "parallel"}),
            lambda v: envelope_error(v, "CAPACITOR_COMBINATION", "INVALID_INPUT")
                      and "must not be negative" in (envelope_field(v, "REASON") or ""))

    # Regression: ledResistor used to accept negative forward/supply voltage.
    # Vf=-1.5 V previously produced R=325Ω silently.
    r.check("ledResistor", "negative vf -> INVALID_INPUT",
            c("ledResistor", {"vs": "5", "vf": "-1.5", "i_f": "0.02"}),
            lambda v: envelope_error(v, "LED_RESISTOR", "INVALID_INPUT")
                      and "forward voltage must not be negative" in (envelope_field(v, "REASON") or ""))
    r.check("ledResistor", "negative vs -> INVALID_INPUT",
            c("ledResistor", {"vs": "-5", "vf": "2", "i_f": "0.02"}),
            lambda v: envelope_error(v, "LED_RESISTOR", "INVALID_INPUT")
                      and "supply voltage must not be negative" in (envelope_field(v, "REASON") or ""))

    # Regression: rlcResonance must reject non-positive R, L, C
    r.check("rlcResonance", "negative r -> INVALID_INPUT",
            c("rlcResonance", {"r": "-10", "l": "0.001", "c": "0.000001"}),
            lambda v: envelope_error(v, "RLC_RESONANCE", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "resistance=-10")
    r.check("rlcResonance", "zero inductance -> INVALID_INPUT",
            c("rlcResonance", {"r": "10", "l": "0", "c": "0.000001"}),
            lambda v: envelope_error(v, "RLC_RESONANCE", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "inductance=0")
    r.check("rlcResonance", "negative capacitance -> INVALID_INPUT",
            c("rlcResonance", {"r": "10", "l": "0.001", "c": "-0.000001"}),
            lambda v: envelope_error(v, "RLC_RESONANCE", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "capacitance=-0.000001")

    # Regression: wheatstoneBridge must reject negative resistances
    r.check("wheatstoneBridge", "negative r1 -> INVALID_INPUT",
            c("wheatstoneBridge", {"r1": "-100", "r2": "200", "r3": "300"}),
            lambda v: envelope_error(v, "WHEATSTONE_BRIDGE", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "r1=-100")
    r.check("wheatstoneBridge", "negative r2 -> INVALID_INPUT",
            c("wheatstoneBridge", {"r1": "100", "r2": "-200", "r3": "300"}),
            lambda v: envelope_error(v, "WHEATSTONE_BRIDGE", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "r2=-200")
    r.check("wheatstoneBridge", "negative r3 -> INVALID_INPUT",
            c("wheatstoneBridge", {"r1": "100", "r2": "200", "r3": "-300"}),
            lambda v: envelope_error(v, "WHEATSTONE_BRIDGE", "INVALID_INPUT")
                      and envelope_field(v, "DETAIL") == "r3=-300")

    # Regression: voltageDivider/currentDivider must reject negative driving
    # signals. Bipolar supplies are modeled by the caller as absolute values
    # with explicit sign handling; mixing negative Vin here used to silently
    # return Vout=-Vin/2 (and likewise for current), masking bad inputs.
    r.check("voltageDivider", "negative vin -> INVALID_INPUT",
            c("voltageDivider", {"vin": "-12", "r1": "1000", "r2": "1000"}),
            lambda v: envelope_error(v, "VOLTAGE_DIVIDER", "INVALID_INPUT")
                      and "vin must not be negative" in (envelope_field(v, "REASON") or "")
                      and envelope_field(v, "DETAIL") == "vin=-12")
    r.check("voltageDivider", "zero vin still allowed",
            c("voltageDivider", {"vin": "0", "r1": "1000", "r2": "1000"}),
            lambda v: envelope_ok(v, "VOLTAGE_DIVIDER")
                      and envelope_field(v, "VOUT") == "0")
    r.check("currentDivider", "negative totalCurrent -> INVALID_INPUT",
            c("currentDivider", {"totalCurrent": "-1", "r1": "1000", "r2": "1000"}),
            lambda v: envelope_error(v, "CURRENT_DIVIDER", "INVALID_INPUT")
                      and "totalCurrent must not be negative" in (envelope_field(v, "REASON") or "")
                      and envelope_field(v, "DETAIL") == "totalCurrent=-1")
    r.check("currentDivider", "zero totalCurrent still allowed",
            c("currentDivider", {"totalCurrent": "0", "r1": "1000", "r2": "1000"}),
            lambda v: envelope_ok(v, "CURRENT_DIVIDER"))


def test_digital(r: TestRunner) -> None:
    r.category("digital electronics", 11)
    c = r.client.call

    r.check("convertBase", "255 dec->hex", c("convertBase", {
        "value": "255", "fromBase": 10, "toBase": 16
    }), lambda v: envelope_result(v, "CONVERT_BASE") == "FF")

    r.check("twosComplement", "-5 8-bit", c("twosComplement", {
        "value": "-5", "bits": 8, "direction": "toTwos"
    }), lambda v: envelope_result(v, "TWOS_COMPLEMENT") == "11111011")
    # Regression: values outside the signed range must surface OUT_OF_RANGE
    # instead of silently wrapping via bitmask (used to return 00000000).
    r.check("twosComplement", "1024 8-bit -> OUT_OF_RANGE",
            c("twosComplement", {"value": "1024", "bits": 8, "direction": "toTwos"}),
            lambda v: envelope_error(v, "TWOS_COMPLEMENT", "OUT_OF_RANGE")
                      and envelope_field(v, "DETAIL") == "value=1024, min=-128, max=127")

    r.check("grayCode", "1010 toGray", c("grayCode", {
        "value": "1010", "direction": "toGray"
    }), lambda v: envelope_result(v, "GRAY_CODE") == "1111")

    bw = c("bitwiseOp", {"a": "12", "b": "10", "operation": "AND"})
    r.check("bitwiseOp", "12 AND 10", bw,
            lambda v: envelope_result(v, "BITWISE_OP") == "8",
            detail_render=lambda v: f"result={envelope_field(v, 'RESULT')}")

    adc = c("adcResolution", {"bits": 10, "vref": "5"})
    r.check("adcResolution", "10-bit @5V", adc,
            lambda v: envelope_ok(v, "ADC_RESOLUTION")
            and envelope_field(v, "LSB") is not None
            and envelope_field(v, "STEP_COUNT") is not None,
            detail_render=lambda v: f"lsb={envelope_field(v, 'LSB')}, "
                                    f"stepCount={envelope_field(v, 'STEP_COUNT')}")

    dac = c("dacOutput", {"bits": 10, "vref": "5", "code": 512})
    r.check("dacOutput", "10-bit code=512", dac,
            lambda v: envelope_ok(v, "DAC_OUTPUT")
            and TestRunner.close(envelope_field(v, "VOUT"), 2.5, 0.01))

    ast = c("timer555Astable", {"r1": "1000", "r2": "1000", "c": "0.000001"})
    r.check("timer555Astable", "R1=R2=1k C=1uF", ast,
            lambda v: envelope_ok(v, "TIMER_555_ASTABLE")
            and envelope_field(v, "FREQUENCY") is not None,
            detail_render=lambda v: f"f={envelope_field(v, 'FREQUENCY')}")

    # Timer555MonostableParams uses fields `r` and `c`, NOT resistance/capacitance.
    mono = c("timer555Monostable", {"r": "1000", "c": "0.000001"})
    r.check("timer555Monostable", "R=1k C=1uF", mono,
            lambda v: envelope_ok(v, "TIMER_555_MONOSTABLE")
            and envelope_field(v, "PULSE_WIDTH") is not None,
            detail_render=lambda v: f"pulseWidth={envelope_field(v, 'PULSE_WIDTH')}")

    fp = c("frequencyPeriod", {"value": "1000", "mode": "freqToPeriod"})
    r.check("frequencyPeriod", "1000 freqToPeriod", fp,
            lambda v: TestRunner.close(envelope_result(v, "FREQUENCY_PERIOD"), 0.001, 1e-9))

    # NyquistParams uses `bandwidth_hz` which camelCases to `bandwidthHz`.
    nyq = c("nyquistRate", {"bandwidthHz": "20000"})
    r.check("nyquistRate", "20kHz", nyq,
            lambda v: TestRunner.close(envelope_result(v, "NYQUIST_RATE"), 40000.0, 1e-6),
            detail_render=lambda v: f"result={envelope_field(v, 'RESULT')}")


# --------------------------------------------------------------------------- #
#  Statistics
# --------------------------------------------------------------------------- #


def test_statistics(r: TestRunner) -> None:
    r.category("statistics", 16)
    c = r.client.call

    r.check("mean", "1..5", c("mean", {"values": "1,2,3,4,5"}),
            lambda v: TestRunner.close(envelope_result(v, "MEAN"), 3.0))
    r.check("median", "1..4", c("median", {"values": "1,2,3,4"}),
            lambda v: TestRunner.close(envelope_result(v, "MEDIAN"), 2.5))
    r.check("mode", "1,2,2,3", c("mode", {"values": "1,2,2,3"}),
            lambda v: envelope_ok(v, "MODE") and envelope_field(v, "MODES") == "2.0")

    r.check("variance", "sample 1..5", c("variance", {"values": "1,2,3,4,5", "population": False}),
            lambda v: TestRunner.close(envelope_result(v, "VARIANCE"), 2.5))
    r.check("stdDev", "sample 1..5", c("stdDev", {"values": "1,2,3,4,5", "population": False}),
            lambda v: TestRunner.close(envelope_result(v, "STDDEV"), 1.5811388, 1e-5))

    r.check("percentile", "p50", c("percentile", {"values": "1,2,3,4,5", "p": "50"}),
            lambda v: TestRunner.close(envelope_result(v, "PERCENTILE"), 3.0))
    r.check("quartile", "q1", c("quartile", {"values": "1,2,3,4,5", "q": 1}),
            lambda v: envelope_ok(v, "QUARTILE") and TestRunner.close(envelope_field(v, "VALUE"), 2.0))
    r.check("iqr", "1..9", c("iqr", {"values": "1,2,3,4,5,6,7,8,9"}),
            lambda v: envelope_ok(v, "IQR") and TestRunner.close(envelope_field(v, "IQR"), 4.0))

    r.check("correlation", "perfect +", c("correlation", {"xValues": "1,2,3,4,5", "yValues": "2,4,6,8,10"}),
            lambda v: TestRunner.close(envelope_result(v, "CORRELATION"), 1.0))
    r.check("covariance", "sample", c("covariance", {"xValues": "1,2,3,4,5", "yValues": "2,4,6,8,10", "population": False}),
            lambda v: TestRunner.close(envelope_result(v, "COVARIANCE"), 5.0))

    r.check("linearRegression", "y=2x+1", c("linearRegression", {"xValues": "0,1,2,3,4", "yValues": "1,3,5,7,9"}),
            lambda v: envelope_ok(v, "LINEAR_REGRESSION")
            and TestRunner.close(envelope_field(v, "SLOPE"), 2.0)
            and TestRunner.close(envelope_field(v, "INTERCEPT"), 1.0))

    r.check("normalPdf", "f(0;0,1)", c("normalPdf", {"x": "0", "mean": "0", "stdDev": "1"}),
            lambda v: TestRunner.close(envelope_result(v, "NORMAL_PDF"), 0.3989422804, 1e-4))
    r.check("normalCdf", "F(1;0,1)", c("normalCdf", {"x": "1", "mean": "0", "stdDev": "1"}),
            lambda v: TestRunner.close(envelope_result(v, "NORMAL_CDF"), 0.8413, 1e-3))

    r.check("tTestOneSample", "t=?", c("tTestOneSample", {"values": "1,2,3,4,5", "hypothesizedMean": "2.5"}),
            lambda v: envelope_ok(v, "T_TEST") and TestRunner.close(envelope_field(v, "T"), 0.7071, 1e-3))
    r.check("binomialPmf", "B(10,5,0.5)", c("binomialPmf", {"n": 10, "k": 5, "p": "0.5"}),
            lambda v: TestRunner.close(envelope_result(v, "BINOMIAL_PMF"), 0.2461, 1e-3))
    r.check("confidenceInterval", "95%", c("confidenceInterval",
                                          {"values": "1,2,3,4,5", "confidenceLevel": "0.95"}),
            lambda v: envelope_ok(v, "CONFIDENCE_INTERVAL")
            and TestRunner.close(envelope_field(v, "MEAN"), 3.0))


# --------------------------------------------------------------------------- #
#  Combinatorics
# --------------------------------------------------------------------------- #


def test_combinatorics(r: TestRunner) -> None:
    r.category("combinatorics", 7)
    c = r.client.call

    r.check("combination", "C(10,3)", c("combination", {"n": 10, "k": 3}),
            lambda v: envelope_result(v, "COMBINATION") == "120")
    r.check("permutation", "P(5,2)", c("permutation", {"n": 5, "k": 2}),
            lambda v: envelope_result(v, "PERMUTATION") == "20")
    r.check("fibonacci", "fib(20)", c("fibonacci", {"n": 20}),
            lambda v: envelope_result(v, "FIBONACCI") == "6765")
    r.check("isPrime", "13", c("isPrime", {"n": 13}),
            lambda v: envelope_ok(v, "IS_PRIME") and envelope_field(v, "IS_PRIME") == "true")
    r.check("nextPrime", "7", c("nextPrime", {"n": 7}),
            lambda v: envelope_result(v, "NEXT_PRIME") == "11")
    r.check("primeFactors", "12", c("primeFactors", {"n": 12}),
            lambda v: envelope_ok(v, "PRIME_FACTORS") and envelope_field(v, "FACTORS") == "2,2,3")
    # The empty product represents the prime factorization of 1 — COUNT 0, no
    # FACTORS, matching the convention used by eulerTotient(1).
    r.check("primeFactors", "1 is empty product", c("primeFactors", {"n": 1}),
            lambda v: envelope_ok(v, "PRIME_FACTORS") and envelope_field(v, "COUNT") == "0")
    r.check("eulerTotient", "φ(10)", c("eulerTotient", {"n": 10}),
            lambda v: envelope_result(v, "EULER_TOTIENT") == "4")


# --------------------------------------------------------------------------- #
#  Geometry
# --------------------------------------------------------------------------- #


def test_geometry(r: TestRunner) -> None:
    r.category("geometry", 12)
    c = r.client.call
    import math

    r.check("circleArea", "r=1", c("circleArea", {"radius": "1"}),
            lambda v: TestRunner.close(envelope_result(v, "CIRCLE_AREA"), math.pi, 1e-9))
    r.check("circlePerimeter", "r=1", c("circlePerimeter", {"radius": "1"}),
            lambda v: TestRunner.close(envelope_result(v, "CIRCLE_PERIMETER"), 2 * math.pi, 1e-9))
    r.check("sphereVolume", "r=1", c("sphereVolume", {"radius": "1"}),
            lambda v: TestRunner.close(envelope_result(v, "SPHERE_VOLUME"), 4/3 * math.pi, 1e-9))
    r.check("sphereArea", "r=1", c("sphereArea", {"radius": "1"}),
            lambda v: TestRunner.close(envelope_result(v, "SPHERE_AREA"), 4 * math.pi, 1e-9))

    r.check("triangleArea", "3-4-5", c("triangleArea", {"sides": "3,4,5"}),
            lambda v: TestRunner.close(envelope_result(v, "TRIANGLE_AREA"), 6.0, 1e-9))
    r.check("polygonArea", "unit square", c("polygonArea", {"coordinates": "0,0,1,0,1,1,0,1"}),
            lambda v: envelope_ok(v, "POLYGON_AREA") and TestRunner.close(envelope_field(v, "AREA"), 1.0))

    r.check("coneVolume", "r=1,h=3", c("coneVolume", {"radius": "1", "height": "3"}),
            lambda v: TestRunner.close(envelope_result(v, "CONE_VOLUME"), math.pi, 1e-9))
    r.check("cylinderVolume", "r=1,h=1", c("cylinderVolume", {"radius": "1", "height": "1"}),
            lambda v: TestRunner.close(envelope_result(v, "CYLINDER_VOLUME"), math.pi, 1e-9))

    r.check("distance2D", "0,0→3,4", c("distance2D", {"p1": "0,0", "p2": "3,4"}),
            lambda v: TestRunner.close(envelope_result(v, "DISTANCE_2D"), 5.0))
    r.check("distance3D", "origin→(1,1,1)", c("distance3D", {"p1": "0,0,0", "p2": "1,1,1"}),
            lambda v: TestRunner.close(envelope_result(v, "DISTANCE_3D"), math.sqrt(3), 1e-9))

    r.check("regularPolygon", "square s=2", c("regularPolygon", {"sides": 4, "sideLength": "2"}),
            lambda v: envelope_ok(v, "REGULAR_POLYGON")
            and TestRunner.close(envelope_field(v, "AREA"), 4.0))
    r.check("pointToLineDistance", "0,0→x=1", c("pointToLineDistance",
                                               {"point": "0,0", "lineP1": "1,0", "lineP2": "1,5"}),
            lambda v: TestRunner.close(envelope_result(v, "POINT_TO_LINE_DISTANCE"), 1.0))


# --------------------------------------------------------------------------- #
#  Complex numbers
# --------------------------------------------------------------------------- #


def test_complex(r: TestRunner) -> None:
    r.category("complex", 10)
    c = r.client.call

    r.check("complexAdd", "(1+2i)+(3+4i)", c("complexAdd", {"a": "1,2", "b": "3,4"}),
            lambda v: envelope_ok(v, "COMPLEX_ADD")
            and TestRunner.close(envelope_field(v, "REAL"), 4.0)
            and TestRunner.close(envelope_field(v, "IMAG"), 6.0))
    r.check("complexMult", "(1+2i)*(3+4i)", c("complexMult", {"a": "1,2", "b": "3,4"}),
            lambda v: envelope_ok(v, "COMPLEX_MULT")
            and TestRunner.close(envelope_field(v, "REAL"), -5.0)
            and TestRunner.close(envelope_field(v, "IMAG"), 10.0))
    r.check("complexDiv", "(1+2i)/(3+4i)", c("complexDiv", {"a": "1,2", "b": "3,4"}),
            lambda v: envelope_ok(v, "COMPLEX_DIV")
            and TestRunner.close(envelope_field(v, "REAL"), 0.44, 1e-4))
    r.check("complexConjugate", "3+5i", c("complexConjugate", {"z": "3,5"}),
            lambda v: envelope_ok(v, "COMPLEX_CONJUGATE")
            and TestRunner.close(envelope_field(v, "IMAG"), -5.0))
    r.check("complexPower", "(1+i)^2", c("complexPower", {"z": "1,1", "exponent": "2"}),
            lambda v: envelope_ok(v, "COMPLEX_POWER")
            and TestRunner.close(envelope_field(v, "IMAG"), 2.0, 1e-6))
    r.check("complexMagnitude", "3+4i", c("complexMagnitude", {"z": "3,4"}),
            lambda v: TestRunner.close(envelope_result(v, "COMPLEX_MAGNITUDE"), 5.0))
    r.check("complexPhase", "i", c("complexPhase", {"z": "0,1"}),
            lambda v: TestRunner.close(envelope_result(v, "COMPLEX_PHASE"), 90.0))
    r.check("polarToRect", "r=2,θ=90°", c("polarToRect", {"magnitude": "2", "angleDegrees": "90"}),
            lambda v: envelope_ok(v, "POLAR_TO_RECT")
            and TestRunner.close(envelope_field(v, "IMAG"), 2.0, 1e-9))
    # Regression: f64 cos(60°) drifts to 0.5000000000000001; the
    # 15-sig-digit snap restores the exact 0.5 decimal a student expects.
    r.check("polarToRect", "r=1,θ=60° snaps clean half",
            c("polarToRect", {"magnitude": "1", "angleDegrees": "60"}),
            lambda v: envelope_ok(v, "POLAR_TO_RECT")
            and envelope_field(v, "REAL") == "0.5")
    r.check("rectToPolar", "2i", c("rectToPolar", {"z": "0,2"}),
            lambda v: envelope_ok(v, "RECT_TO_POLAR")
            and TestRunner.close(envelope_field(v, "MAGNITUDE"), 2.0))
    r.check("complexSqrt", "-1", c("complexSqrt", {"z": "-1,0"}),
            lambda v: envelope_ok(v, "COMPLEX_SQRT")
            and TestRunner.close(envelope_field(v, "IMAG"), 1.0, 1e-9))


# --------------------------------------------------------------------------- #
#  Crypto/Encoding
# --------------------------------------------------------------------------- #


def test_crypto(r: TestRunner) -> None:
    r.category("crypto", 10)
    c = r.client.call

    r.check("hashMd5", "abc", c("hashMd5", {"input": "abc"}),
            lambda v: envelope_result(v, "HASH_MD5") == "900150983cd24fb0d6963f7d28e17f72")
    r.check("hashSha1", "abc", c("hashSha1", {"input": "abc"}),
            lambda v: envelope_result(v, "HASH_SHA1") == "a9993e364706816aba3e25717850c26c9cd0d89d")
    r.check("hashSha256", "abc", c("hashSha256", {"input": "abc"}),
            lambda v: envelope_result(v, "HASH_SHA256")
            == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    r.check("hashSha512", "abc prefix", c("hashSha512", {"input": "abc"}),
            lambda v: envelope_ok(v, "HASH_SHA512")
            and envelope_result(v, "HASH_SHA512").startswith("ddaf35a193617aba"))

    r.check("base64Encode", "Hello, world!", c("base64Encode", {"input": "Hello, world!"}),
            lambda v: envelope_result(v, "BASE64_ENCODE") == "SGVsbG8sIHdvcmxkIQ==")
    r.check("base64Decode", "SGVsbG8sIHdvcmxkIQ==", c("base64Decode", {"input": "SGVsbG8sIHdvcmxkIQ=="}),
            lambda v: envelope_result(v, "BASE64_DECODE") == "Hello, world!")

    r.check("urlEncode", "hello world!", c("urlEncode", {"input": "hello world!"}),
            lambda v: envelope_result(v, "URL_ENCODE") == "hello%20world%21")
    r.check("urlDecode", "encoded", c("urlDecode", {"input": "hello%20world%21"}),
            lambda v: envelope_result(v, "URL_DECODE") == "hello world!")
    # Form-encoded `+` must decode to space — matches WHATWG / every
    # mainstream decoder (Python unquote_plus, Java URLDecoder, …). A
    # decoder that leaves `+` literal silently corrupts real form payloads.
    r.check("urlDecode", "form-encoded + becomes space",
            c("urlDecode", {"input": "hello+world"}),
            lambda v: envelope_result(v, "URL_DECODE") == "hello world")
    # Escaped `%2B` preserves a literal `+` in the decoded output.
    r.check("urlDecode", "%2B preserves literal +",
            c("urlDecode", {"input": "a%2Bb"}),
            lambda v: envelope_result(v, "URL_DECODE") == "a+b")

    r.check("hexEncode", "ABC", c("hexEncode", {"input": "ABC"}),
            lambda v: envelope_result(v, "HEX_ENCODE") == "414243")
    r.check("crc32", "123456789", c("crc32", {"input": "123456789"}),
            lambda v: envelope_ok(v, "CRC32") and envelope_field(v, "HEX") == "cbf43926")


# --------------------------------------------------------------------------- #
#  Matrices
# --------------------------------------------------------------------------- #


def test_matrices(r: TestRunner) -> None:
    r.category("matrices", 10)
    c = r.client.call

    r.check("matrixAdd", "2x2", c("matrixAdd", {"a": "1,2;3,4", "b": "5,6;7,8"}),
            lambda v: envelope_ok(v, "MATRIX_ADD")
            and envelope_field(v, "MATRIX") == "6.0,8.0;10.0,12.0")
    r.check("matrixMultiply", "2x2*I", c("matrixMultiply", {"a": "1,2;3,4", "b": "1,0;0,1"}),
            lambda v: envelope_ok(v, "MATRIX_MULT")
            and envelope_field(v, "MATRIX") == "1.0,2.0;3.0,4.0")
    r.check("matrixTranspose", "2x3", c("matrixTranspose", {"a": "1,2,3;4,5,6"}),
            lambda v: envelope_ok(v, "MATRIX_TRANSPOSE")
            and envelope_field(v, "MATRIX") == "1.0,4.0;2.0,5.0;3.0,6.0")
    r.check("matrixDeterminant", "2x2", c("matrixDeterminant", {"a": "1,2;3,4"}),
            lambda v: TestRunner.close(envelope_result(v, "MATRIX_DETERMINANT"), -2.0))
    r.check("matrixInverse", "2x2 invertible", c("matrixInverse", {"a": "1,2;3,4"}),
            lambda v: envelope_ok(v, "MATRIX_INVERSE"))
    r.check("matrixTrace", "diag(1,2,3)", c("matrixTrace", {"a": "1,0,0;0,2,0;0,0,3"}),
            lambda v: TestRunner.close(envelope_result(v, "MATRIX_TRACE"), 6.0))
    r.check("matrixRank", "full rank 2x2", c("matrixRank", {"a": "1,2;3,4"}),
            lambda v: envelope_result(v, "MATRIX_RANK") == "2")
    r.check("matrixEigenvalues2x2", "diag(2,3)", c("matrixEigenvalues2x2", {"a": "2,0;0,3"}),
            lambda v: envelope_ok(v, "MATRIX_EIGENVALUES_2X2")
            and envelope_field(v, "KIND") == "real")
    r.check("crossProduct", "i×j=k", c("crossProduct", {"a": "1,0,0", "b": "0,1,0"}),
            lambda v: envelope_result(v, "CROSS_PRODUCT") == "0.0,0.0,1.0")
    r.check("gaussianElimination", "2x3 system", c("gaussianElimination", {"coefficients": "1,1,3;2,3,8"}),
            lambda v: envelope_ok(v, "GAUSSIAN_ELIMINATION")
            and envelope_field(v, "SOLUTION") == "1.0,2.0")
    # x+y=1, 2x+2y=3 → rank(A)=1, rank(A|b)=2 → inconsistent (no solution).
    r.check("gaussianElimination", "inconsistent system",
            c("gaussianElimination", {"coefficients": "1,1,1;2,2,3"}),
            lambda v: envelope_error(v, "GAUSSIAN_ELIMINATION", "DOMAIN_ERROR")
            and "inconsistent" in v.lower())
    # x+y=1, 2x+2y=2 → rank(A)=rank(A|b)=1 → underdetermined (infinite).
    r.check("gaussianElimination", "underdetermined system",
            c("gaussianElimination", {"coefficients": "1,1,1;2,2,2"}),
            lambda v: envelope_error(v, "GAUSSIAN_ELIMINATION", "DOMAIN_ERROR")
            and "underdetermined" in v.lower())


# --------------------------------------------------------------------------- #
#  Physics
# --------------------------------------------------------------------------- #


def test_physics(r: TestRunner) -> None:
    r.category("physics", 12)
    c = r.client.call
    import math

    r.check("kinematics", "v0=0,a=10,t=2", c("kinematics",
                                             {"initialVelocity": "0", "acceleration": "10", "time": "2"}),
            lambda v: envelope_ok(v, "KINEMATICS")
            and TestRunner.close(envelope_field(v, "FINAL_VELOCITY"), 20.0)
            and TestRunner.close(envelope_field(v, "DISPLACEMENT"), 20.0))

    # Regression: the f64 path used to return `0.1899999999999995` for the
    # textbook `v0=10, a=-9.81, t=1` (fma rounds 10 + -9.81 one ULP low).
    # The BigDecimal recomputation preserves the caller's decimal
    # precision so the response matches what a student would write.
    r.check("kinematics", "v0=10,a=-9.81,t=1 (no f64 drift)",
            c("kinematics",
              {"initialVelocity": "10", "acceleration": "-9.81", "time": "1"}),
            lambda v: envelope_ok(v, "KINEMATICS")
            and envelope_field(v, "FINAL_VELOCITY") == "0.19"
            and envelope_field(v, "DISPLACEMENT") == "5.095")

    r.check("projectileMotion", "v=10,θ=45,g=9.81",
            c("projectileMotion", {"speed": "10", "angleDegrees": "45", "gravity": "9.81"}),
            lambda v: envelope_ok(v, "PROJECTILE_MOTION")
            and TestRunner.close(envelope_field(v, "RANGE"), 100.0 / 9.81, 1e-3))

    r.check("newtonsForce", "m=5,a=2", c("newtonsForce", {"mass": "5", "acceleration": "2"}),
            lambda v: TestRunner.close(envelope_result(v, "NEWTONS_FORCE"), 10.0))

    r.check("gravitationalForce", "unit masses 1m",
            c("gravitationalForce", {"m1": "1", "m2": "1", "distance": "1"}),
            lambda v: TestRunner.close(envelope_result(v, "GRAVITATIONAL_FORCE"), 6.67430e-11, 1e-15))

    r.check("dopplerEffect", "observer approaches",
            c("dopplerEffect", {"sourceFreq": "440", "soundSpeed": "340",
                                "sourceVelocity": "0", "observerVelocity": "170"}),
            lambda v: TestRunner.close(envelope_result(v, "DOPPLER_EFFECT"), 660.0, 1e-3))

    r.check("waveLength", "1MHz light",
            c("waveLength", {"frequency": "1000000", "waveSpeed": "300000000"}),
            lambda v: TestRunner.close(envelope_result(v, "WAVE_LENGTH"), 300.0))

    r.check("planckEnergy", "f=1Hz", c("planckEnergy", {"frequency": "1"}),
            lambda v: TestRunner.close(envelope_result(v, "PLANCK_ENERGY"), 6.62607015e-34, 1e-40))

    r.check("idealGasLaw", "solve V",
            c("idealGasLaw", {"pressure": "101325", "volume": "0",
                              "moles": "1", "temperature": "273.15", "solveFor": "V"}),
            lambda v: envelope_ok(v, "IDEAL_GAS_LAW")
            and TestRunner.close(envelope_field(v, "VALUE"), 0.0224, 1e-3))

    r.check("heatTransfer", "k=10,A=1,ΔT=20,L=2",
            c("heatTransfer", {"thermalConductivity": "10", "area": "1",
                               "deltaTemp": "20", "thickness": "2"}),
            lambda v: TestRunner.close(envelope_result(v, "HEAT_TRANSFER"), 100.0))

    r.check("stefanBoltzmann", "ε=1,A=1,T=300",
            c("stefanBoltzmann", {"emissivity": "1", "area": "1", "temperatureK": "300"}),
            lambda v: TestRunner.close(envelope_result(v, "STEFAN_BOLTZMANN"),
                                       5.670e-8 * 300**4, 0.1))

    r.check("escapeVelocity", "Earth",
            c("escapeVelocity", {"mass": "5.972e24", "radius": "6.371e6"}),
            lambda v: TestRunner.close(envelope_result(v, "ESCAPE_VELOCITY"), 11186.0, 10.0))

    r.check("orbitalVelocity", "ISS altitude",
            c("orbitalVelocity", {"mass": "5.972e24", "radius": "6.78e6"}),
            lambda v: TestRunner.close(envelope_result(v, "ORBITAL_VELOCITY"), 7660.0, 100.0))


# --------------------------------------------------------------------------- #
#  Chemistry
# --------------------------------------------------------------------------- #


def test_chemistry(r: TestRunner) -> None:
    r.category("chemistry", 9)
    c = r.client.call

    r.check("molarMass", "H2O", c("molarMass", {"formula": "H2O"}),
            lambda v: envelope_ok(v, "MOLAR_MASS")
            and TestRunner.close(envelope_field(v, "MOLAR_MASS_G_MOL"), 18.015, 1e-2))
    # Pre-2016 IUPAC provisional names (Uuo = Oganesson, Uut = Nihonium…)
    # are aliases so legacy literature parses cleanly.
    r.check("molarMass", "Uuo alias for Og", c("molarMass", {"formula": "Uuo"}),
            lambda v: envelope_ok(v, "MOLAR_MASS")
            and envelope_field(v, "MOLAR_MASS_G_MOL") == "294")
    r.check("ph", "[H+]=1e-7", c("ph", {"hConcentration": "0.0000001"}),
            lambda v: TestRunner.close(envelope_result(v, "PH"), 7.0, 1e-9))
    r.check("poh", "[OH-]=1e-3", c("poh", {"ohConcentration": "0.001"}),
            lambda v: TestRunner.close(envelope_result(v, "POH"), 3.0, 1e-9))
    r.check("molarity", "1mol/0.5L", c("molarity", {"moles": "1", "volumeLitres": "0.5"}),
            lambda v: TestRunner.close(envelope_result(v, "MOLARITY"), 2.0))
    r.check("molality", "1mol/0.5kg", c("molality", {"moles": "1", "kilogramsSolvent": "0.5"}),
            lambda v: TestRunner.close(envelope_result(v, "MOLALITY"), 2.0))
    r.check("hendersonHasselbalch", "equal conc",
            c("hendersonHasselbalch", {"pka": "4.76", "conjugateBase": "1", "weakAcid": "1"}),
            lambda v: TestRunner.close(envelope_result(v, "HENDERSON_HASSELBALCH"), 4.76, 1e-6))
    r.check("halfLife", "λ=0.0693", c("halfLife", {"decayConstant": "0.0693147181"}),
            lambda v: TestRunner.close(envelope_result(v, "HALF_LIFE"), 10.0, 1e-3))
    r.check("decayConstant", "t½=10", c("decayConstant", {"halfLife": "10"}),
            lambda v: TestRunner.close(envelope_result(v, "DECAY_CONSTANT"), 0.0693147, 1e-6))
    r.check("idealGasMoles", "1atm, 22.4L, 273K",
            c("idealGasMoles", {"pressurePa": "101325", "volumeM3": "0.0224", "temperatureK": "273.15"}),
            lambda v: TestRunner.close(envelope_result(v, "IDEAL_GAS_MOLES"), 1.0, 1e-2))


# --------------------------------------------------------------------------- #
#  Main driver
# --------------------------------------------------------------------------- #


def main() -> int:
    if not os.path.isfile(BINARY):
        print(f"FATAL: binary not found at {BINARY}", file=sys.stderr)
        print("Build with: cargo build --release --bin math-calc-mcp", file=sys.stderr)
        return 2

    started_at = time.time()

    client = McpClient()
    try:
        client.initialize()
        tool_names = client.list_tools()
        print(f"Server reported {len(tool_names)} tools via tools/list (expected 173)")

        runner = TestRunner(client)

        test_basic(runner)
        test_scientific(runner)
        test_programmable(runner)
        test_vector(runner)
        test_financial(runner)
        test_calculus(runner)
        test_unit_converter(runner)
        test_cooking(runner)
        test_measure_reference(runner)
        test_datetime(runner)
        test_printing(runner)
        test_graphing(runner)
        test_network(runner)
        test_analog(runner)
        test_digital(runner)
        test_statistics(runner)
        test_combinatorics(runner)
        test_geometry(runner)
        test_complex(runner)
        test_crypto(runner)
        test_matrices(runner)
        test_physics(runner)
        test_chemistry(runner)

        # --- summary --- #
        total = len(runner.results)
        passed = sum(1 for row in runner.results if row[3])
        failures = [row for row in runner.results if not row[3]]

        per_cat: dict[str, list[int]] = {}
        for cat, _tool, _desc, ok, _detail in runner.results:
            bucket = per_cat.setdefault(cat, [0, 0])
            bucket[0] += 1
            if ok:
                bucket[1] += 1

        print("\n" + "=" * 60)
        print("CATEGORY SUMMARY")
        for cat, (n_total, n_ok) in per_cat.items():
            print(f"  {cat:22s} {n_ok}/{n_total}")

        print("=" * 60)
        print(f"RESULTS: {passed}/{total} passed, {total - passed} failed (174 tools expected)")
        if failures:
            print("FAILURES:")
            for _cat, tool, desc, _ok, detail in failures:
                print(f"  - {tool}({desc}): {detail}")
        print(f"Elapsed: {time.time() - started_at:.2f}s")
        print("=" * 60)

        tested_set = {row[1] for row in runner.results}
        missing = set(tool_names) - tested_set
        if missing:
            print(f"NOTE: {len(missing)} tools reported by server but not covered:")
            for name in sorted(missing):
                print(f"  - {name}")

        return 0 if not failures else 1
    finally:
        client.close()


if __name__ == "__main__":
    sys.exit(main())
