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

BINARY = os.environ.get(
    "MATH_CALC_MCP",
    os.path.join(os.path.dirname(__file__), "..", "target", "release", "math-calc-mcp"),
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
    r.category("programmable", 4)
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
    r.check("evaluateExactWithVariables", "pi*2",
            c("evaluateExactWithVariables",
              {"expression": "pi * 2",
               "variables": '{"pi":"3.1415926535897932384626433"}'}),
            lambda v: envelope_result(v, "EVALUATE_EXACT_WITH_VARIABLES")
                      == "6.2831853071795864769252866")


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
    r.category("financial", 6)
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


def test_calculus(r: TestRunner) -> None:
    r.category("calculus", 4)
    c = r.client.call
    r.check("derivative", "x^2 at 3", c("derivative", {
        "expression": "x^2", "variable": "x", "point": 3.0
    }), lambda v: TestRunner.close(envelope_result(v, "DERIVATIVE"), 6.0, 1e-4))
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


def test_measure_reference(r: TestRunner) -> None:
    r.category("measure reference", 4)
    c = r.client.call
    cats = c("listCategories", {})
    r.check("listCategories", "", cats,
            lambda v: envelope_ok(v, "LIST_CATEGORIES")
            and (envelope_field(v, "COUNT") is not None
                 or envelope_field(v, "RESULT") is not None),
            detail_render=lambda v: f"count={envelope_field(v, 'COUNT')}, "
                                    f"result={envelope_field(v, 'RESULT')!s:.60}")

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
    r.category("network", 13)
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

    summary = c("summarizeSubnets", {
        "subnets": '["192.168.0.0/25","192.168.0.128/25"]'
    })
    r.check("summarizeSubnets", "two /25s", summary,
            lambda v: envelope_result(v, "SUMMARIZE_SUBNETS") == "192.168.0.0/24")

    r.check("expandIpv6", "::1", c("expandIpv6", {"address": "::1"}),
            lambda v: envelope_result(v, "EXPAND_IPV6")
            == "0000:0000:0000:0000:0000:0000:0000:0001")

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


def test_analog(r: TestRunner) -> None:
    r.category("analog electronics", 14)
    c = r.client.call

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


def test_digital(r: TestRunner) -> None:
    r.category("digital electronics", 10)
    c = r.client.call

    r.check("convertBase", "255 dec->hex", c("convertBase", {
        "value": "255", "fromBase": 10, "toBase": 16
    }), lambda v: envelope_result(v, "CONVERT_BASE") == "FF")

    r.check("twosComplement", "-5 8-bit", c("twosComplement", {
        "value": "-5", "bits": 8, "direction": "toTwos"
    }), lambda v: envelope_result(v, "TWOS_COMPLEMENT") == "11111011")

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
        print(f"Server reported {len(tool_names)} tools via tools/list (expected 87)")

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
        print(f"RESULTS: {passed}/{total} passed, {total - passed} failed (87 tools expected)")
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
