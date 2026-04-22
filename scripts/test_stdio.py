#!/usr/bin/env python3
"""
End-to-end stdio integration test for the math-calc-mcp binary.

Spawns the MCP server as a subprocess, handshakes, enumerates tools via
`tools/list`, and exercises every tool at least once via JSON-RPC over stdio.

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

    def call(self, name: str, arguments: dict):
        resp = self.send("tools/call", {"name": name, "arguments": arguments})
        if resp is None:
            return {"_error": "No response"}
        if "error" in resp and isinstance(resp["error"], dict):
            return {"_error": resp["error"].get("message", str(resp["error"]))}
        result = resp.get("result", {})
        if result.get("isError"):
            text = (result.get("content") or [{}])[0].get("text", "")
            return {"_error": text}
        text = (result.get("content") or [{}])[0].get("text", "")
        # Server returns plain strings, JSON objects, or JSON arrays. Only attempt
        # to JSON-decode when the payload LOOKS structured — a bare number token
        # like "7" or "11111011" is a string from the server's POV and decoding
        # it silently would turn it into an int, breaking string assertions.
        if isinstance(text, str):
            stripped = text.strip()
            if stripped.startswith(("{", "[")):
                try:
                    return json.loads(stripped)
                except json.JSONDecodeError:
                    return text
        return text

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


def stripped_equal(actual: str, *alternatives: str) -> bool:
    """Match a numeric string allowing trailing-zero differences."""
    def norm(s: str) -> str:
        s = str(s).strip()
        if "." in s:
            s = s.rstrip("0").rstrip(".")
        if s in ("", "-"):
            s = "0"
        return s

    a = norm(actual)
    return any(a == norm(alt) for alt in alternatives)


# --------------------------------------------------------------------------- #
#  Category test implementations
# --------------------------------------------------------------------------- #


def test_basic(r: TestRunner) -> None:
    r.category("basic", 7)
    c = r.client.call
    r.check("add", "0.1, 0.2", c("add", {"first": "0.1", "second": "0.2"}),
            lambda v: v == "0.3")
    r.check("subtract", "10, 3", c("subtract", {"first": "10", "second": "3"}),
            lambda v: v == "7")
    r.check("multiply", "3, 4", c("multiply", {"first": "3", "second": "4"}),
            lambda v: v == "12")
    r.check("divide", "10, 3", c("divide", {"first": "10", "second": "3"}),
            lambda v: v == "3.33333333333333333333")
    r.check("power", "2^10", c("power", {"base": "2", "exponent": "10"}),
            lambda v: v == "1024")
    r.check("modulo", "10 %% 3", c("modulo", {"first": "10", "second": "3"}),
            lambda v: v == "1")
    r.check("abs", "-5", c("abs", {"value": "-5"}),
            lambda v: v == "5")


def test_scientific(r: TestRunner) -> None:
    r.category("scientific", 7)
    c = r.client.call
    r.check("sqrt", "16", c("sqrt", {"number": 16.0}),
            lambda v: v == "4.0" or TestRunner.close(v, 4.0))
    r.check("log", "e", c("log", {"number": 2.718281828459045}),
            lambda v: TestRunner.close(v, 1.0, 1e-6))
    r.check("log10", "100", c("log10", {"number": 100.0}),
            lambda v: TestRunner.close(v, 2.0, 1e-6))
    r.check("factorial", "5", c("factorial", {"num": 5}),
            lambda v: v == "120")
    r.check("sin", "30deg", c("sin", {"degrees": 30.0}),
            lambda v: TestRunner.close(v, 0.5, 1e-9))
    r.check("cos", "60deg", c("cos", {"degrees": 60.0}),
            lambda v: TestRunner.close(v, 0.5, 1e-9))
    r.check("tan", "45deg", c("tan", {"degrees": 45.0}),
            lambda v: TestRunner.close(v, 1.0, 1e-9))


def test_programmable(r: TestRunner) -> None:
    r.category("programmable", 2)
    c = r.client.call
    r.check("evaluate", "2+3*4", c("evaluate", {"expression": "2+3*4"}),
            lambda v: TestRunner.close(v, 14.0, 1e-9))
    r.check("evaluateWithVariables", "2*x+y", c(
        "evaluateWithVariables",
        {"expression": "2*x + y", "variables": '{"x":3,"y":1}'},
    ), lambda v: TestRunner.close(v, 7.0, 1e-9))


def test_vector(r: TestRunner) -> None:
    r.category("vector", 4)
    c = r.client.call
    r.check("sumArray", "1..5", c("sumArray", {"numbers": "1,2,3,4,5"}),
            lambda v: TestRunner.close(v, 15.0, 1e-9))
    r.check("dotProduct", "[1,2,3].[4,5,6]", c(
        "dotProduct", {"first": "1,2,3", "second": "4,5,6"}),
            lambda v: TestRunner.close(v, 32.0, 1e-9))
    r.check("scaleArray", "[1,2,3]*2", c(
        "scaleArray", {"numbers": "1,2,3", "scalar": "2"}),
            lambda v: isinstance(v, str) and [float(x) for x in v.split(",")] == [2.0, 4.0, 6.0])
    r.check("magnitudeArray", "[3,4]", c("magnitudeArray", {"numbers": "3,4"}),
            lambda v: TestRunner.close(v, 5.0, 1e-9))


def test_financial(r: TestRunner) -> None:
    r.category("financial", 6)
    c = r.client.call
    r.check("compoundInterest", "1000@5%/10y/12", c("compoundInterest", {
        "principal": "1000", "annualRate": "5", "years": "10", "compoundsPerYear": 12
    }), lambda v: TestRunner.close(v, 1647.009497, 1.0))
    r.check("loanPayment", "100k@5%/30y", c("loanPayment", {
        "principal": "100000", "annualRate": "5", "years": "30"
    }), lambda v: TestRunner.close(v, 536.82, 2.0))
    r.check("presentValue", "fv=1000@5%/10y", c("presentValue", {
        "futureValue": "1000", "annualRate": "5", "years": "10"
    }), lambda v: TestRunner.close(v, 613.91, 2.0))
    r.check("futureValueAnnuity", "100@5%/10y", c("futureValueAnnuity", {
        "payment": "100", "annualRate": "5", "years": "10"
    }), lambda v: TestRunner.close(v, 1257.79, 3.0))
    # ROI = (gain - cost) / cost * 100. gain=1200, cost=1000 → 20%.
    r.check("returnOnInvestment", "gain=1200/cost=1000", c("returnOnInvestment", {
        "gain": "1200", "cost": "1000"
    }), lambda v: stripped_equal(v, "20"))

    schedule = c("amortizationSchedule", {
        "principal": "10000", "annualRate": "5", "years": "1"
    })
    def predicate(v):
        if not isinstance(v, list) or len(v) != 12:
            return False
        last = v[-1]
        bal = last.get("balance")
        try:
            return abs(float(bal)) < 0.01
        except (TypeError, ValueError):
            return str(bal).startswith("0")
    r.check("amortizationSchedule", "10k@5%/1y", schedule, predicate,
            detail_render=lambda v: f"{len(v)} entries, last.balance={v[-1].get('balance')}"
            if isinstance(v, list) and v else repr(v))


def test_calculus(r: TestRunner) -> None:
    r.category("calculus", 4)
    c = r.client.call
    r.check("derivative", "x^2 at 3", c("derivative", {
        "expression": "x^2", "variable": "x", "point": 3.0
    }), lambda v: TestRunner.close(v, 6.0, 1e-4))
    r.check("nthDerivative", "x^3 n=2 at 2", c("nthDerivative", {
        "expression": "x^3", "variable": "x", "point": 2.0, "order": 2
    }), lambda v: TestRunner.close(v, 12.0, 1e-2))
    r.check("definiteIntegral", "x^2 [0,1]", c("definiteIntegral", {
        "expression": "x^2", "variable": "x", "lower": 0.0, "upper": 1.0
    }), lambda v: TestRunner.close(v, 1.0 / 3.0, 1e-5))
    tangent = c("tangentLine", {"expression": "x^2", "variable": "x", "point": 3.0})
    r.check("tangentLine", "x^2 at 3", tangent,
            lambda v: isinstance(v, dict)
            and TestRunner.close(v.get("slope"), 6.0, 1e-3)
            and TestRunner.close(v.get("yIntercept"), -9.0, 1e-3),
            detail_render=lambda v: f"slope={v.get('slope')}, yIntercept={v.get('yIntercept')}"
            if isinstance(v, dict) else repr(v))


def test_unit_converter(r: TestRunner) -> None:
    r.category("unit converter", 2)
    c = r.client.call
    r.check("convert", "1km->mi", c("convert", {
        "value": "1", "fromUnit": "km", "toUnit": "mi", "category": "LENGTH"
    }), lambda v: isinstance(v, str) and v.startswith("0.6213711922"))
    r.check("convertAutoDetect", "100c->f", c("convertAutoDetect", {
        "value": "100", "fromUnit": "c", "toUnit": "f"
    }), lambda v: stripped_equal(v, "212"))


def test_cooking(r: TestRunner) -> None:
    r.category("cooking", 3)
    c = r.client.call
    vol = c("convertCookingVolume", {"value": "1", "fromUnit": "uscup", "toUnit": "tbsp"})
    r.check("convertCookingVolume", "1 uscup -> tbsp", vol,
            lambda v: TestRunner.close(v, 16.0, 0.5))
    r.check("convertCookingWeight", "1 lb -> oz", c("convertCookingWeight", {
        "value": "1", "fromUnit": "lb", "toUnit": "oz"
    }), lambda v: stripped_equal(v, "16"))
    r.check("convertOvenTemperature", "gasmark 4 -> c", c("convertOvenTemperature", {
        "value": "4", "fromUnit": "gasmark", "toUnit": "c"
    }), lambda v: stripped_equal(v, "180"))


def test_measure_reference(r: TestRunner) -> None:
    r.category("measure reference", 4)
    c = r.client.call
    cats = c("listCategories", {})
    r.check("listCategories", "", cats,
            lambda v: isinstance(v, list) and len(v) >= 1,
            detail_render=lambda v: f"{len(v) if isinstance(v, list) else '?'} categories")

    units = c("listUnits", {"category": "LENGTH"})
    def has_meter(v):
        if not isinstance(v, list):
            return False
        return any(isinstance(u, dict) and u.get("code") == "m" for u in v)
    r.check("listUnits", "LENGTH", units, has_meter,
            detail_render=lambda v: f"{len(v) if isinstance(v, list) else '?'} units")

    r.check("getConversionFactor", "km->m", c("getConversionFactor", {
        "fromUnit": "km", "toUnit": "m"
    }), lambda v: stripped_equal(v, "1000"))

    r.check("explainConversion", "c->f", c("explainConversion", {
        "fromUnit": "c", "toUnit": "f"
    }), lambda v: isinstance(v, str) and "F = C * 9/5 + 32" in v)


def test_datetime(r: TestRunner) -> None:
    r.category("datetime", 5)
    c = r.client.call
    tz = c("convertTimezone", {
        "datetime": "2026-03-03T12:00:00",
        "fromTimezone": "UTC",
        "toTimezone": "Asia/Tokyo",
    })
    r.check("convertTimezone", "UTC->Tokyo", tz,
            lambda v: isinstance(v, str) and "21:00:00" in v and "Tokyo" in v)

    r.check("formatDateTime", "epoch->iso", c("formatDateTime", {
        "datetime": "1709424000",
        "inputFormat": "epoch",
        "outputFormat": "iso-offset",
        "timezone": "UTC",
    }), lambda v: isinstance(v, str) and "2024-03-03" in v)

    now = c("currentDateTime", {"timezone": "UTC", "format": "iso"})
    r.check("currentDateTime", "UTC iso", now,
            lambda v: isinstance(v, str) and "T" in v and any(c.isdigit() for c in v))

    tzs = c("listTimezones", {"region": "Europe"})
    r.check("listTimezones", "Europe", tzs,
            lambda v: isinstance(v, list) and "Europe/Paris" in v,
            detail_render=lambda v: f"{len(v) if isinstance(v, list) else '?'} zones")

    diff = c("dateTimeDifference", {
        "datetime1": "2026-01-01T00:00:00",
        "datetime2": "2026-03-03T15:30:00",
        "timezone": "UTC",
    })
    r.check("dateTimeDifference", "", diff,
            lambda v: isinstance(v, dict) and v.get("totalSeconds", 0) > 0,
            detail_render=lambda v: f"totalSeconds={v.get('totalSeconds')}"
            if isinstance(v, dict) else repr(v))


def test_printing(r: TestRunner) -> None:
    r.category("printing", 1)
    c = r.client.call
    tape = c("calculateWithTape", {
        "operations": '[{"op":"+","value":"100"},{"op":"-","value":"30"},{"op":"=","value":null}]'
    })
    r.check("calculateWithTape", "100-30", tape,
            lambda v: isinstance(v, str) and "70" in v)


def test_graphing(r: TestRunner) -> None:
    r.category("graphing", 3)
    c = r.client.call
    pts = c("plotFunction", {
        "expression": "x^2", "variable": "x", "min": -2.0, "max": 2.0, "steps": 4
    })
    def plot_ok(v):
        if not isinstance(v, list) or len(v) != 5:
            return False
        endpoints_ok = (
            TestRunner.close(v[0].get("x"), -2.0) and TestRunner.close(v[0].get("y"), 4.0)
            and TestRunner.close(v[-1].get("x"), 2.0) and TestRunner.close(v[-1].get("y"), 4.0)
        )
        return endpoints_ok
    r.check("plotFunction", "x^2 [-2,2]", pts, plot_ok,
            detail_render=lambda v: f"{len(v) if isinstance(v, list) else '?'} points")

    root = c("solveEquation", {
        "expression": "x^2 - 4", "variable": "x", "initialGuess": 3.0
    })
    r.check("solveEquation", "x^2-4 near 3", root,
            lambda v: TestRunner.close(v, 2.0, 1e-4))

    roots = c("findRoots", {
        "expression": "x^2 - 4", "variable": "x", "min": -5.0, "max": 5.0
    })
    def roots_ok(v):
        if not isinstance(v, list) or len(v) < 2:
            return False
        vals = sorted(float(x) for x in v)
        return TestRunner.close(vals[0], -2.0, 0.1) and TestRunner.close(vals[-1], 2.0, 0.1)
    r.check("findRoots", "x^2-4 [-5,5]", roots, roots_ok,
            detail_render=lambda v: f"roots={v}" if isinstance(v, list) else repr(v))


def test_network(r: TestRunner) -> None:
    r.category("network", 13)
    c = r.client.call

    subnet = c("subnetCalculator", {"address": "192.168.1.0", "cidr": 24})
    r.check("subnetCalculator", "192.168.1.0/24", subnet,
            lambda v: isinstance(v, dict)
            and v.get("network") == "192.168.1.0"
            and int(v.get("usableHosts", 0)) == 254
            and v.get("ipClass") == "C",
            detail_render=lambda v: f"network={v.get('network')}, usableHosts={v.get('usableHosts')}, ipClass={v.get('ipClass')}"
            if isinstance(v, dict) else repr(v))

    r.check("ipToBinary", "192.168.1.1", c("ipToBinary", {"address": "192.168.1.1"}),
            lambda v: v == "11000000.10101000.00000001.00000001")

    r.check("binaryToIp", "192.168.1.1", c("binaryToIp", {
        "binary": "11000000.10101000.00000001.00000001"
    }), lambda v: v == "192.168.1.1")

    r.check("ipToDecimal", "192.168.1.1", c("ipToDecimal", {"address": "192.168.1.1"}),
            lambda v: v == "3232235777")

    r.check("decimalToIp", "3232235777", c("decimalToIp", {
        "decimal": "3232235777", "version": 4
    }), lambda v: v == "192.168.1.1")

    r.check("ipInSubnet", "100 in /24", c("ipInSubnet", {
        "address": "192.168.1.100", "network": "192.168.1.0", "cidr": 24
    }), lambda v: v == "true" or v is True)

    vlsm = c("vlsmSubnets", {
        "networkCidr": "192.168.1.0/24",
        "hostCounts": "[50,25,10]",
    })
    r.check("vlsmSubnets", "3 subnets", vlsm,
            lambda v: isinstance(v, list) and len(v) > 0,
            detail_render=lambda v: f"{len(v) if isinstance(v, list) else '?'} allocations")

    summary = c("summarizeSubnets", {
        "subnets": '["192.168.0.0/25","192.168.0.128/25"]'
    })
    r.check("summarizeSubnets", "two /25s", summary,
            lambda v: v == "192.168.0.0/24")

    r.check("expandIpv6", "::1", c("expandIpv6", {"address": "::1"}),
            lambda v: v == "0000:0000:0000:0000:0000:0000:0000:0001")

    r.check("compressIpv6", "2001:db8::1", c("compressIpv6", {
        "address": "2001:0db8:0000:0000:0000:0000:0000:0001"
    }), lambda v: v == "2001:db8::1")

    tt = c("transferTime", {
        "fileSize": "1", "fileSizeUnit": "gb",
        "bandwidth": "100", "bandwidthUnit": "mbps",
    })
    r.check("transferTime", "1GB/100Mbps", tt,
            lambda v: isinstance(v, dict) and "seconds" in v,
            detail_render=lambda v: f"seconds={v.get('seconds')}" if isinstance(v, dict) else repr(v))

    thr = c("throughput", {
        "dataSize": "100", "dataSizeUnit": "mb",
        "time": "10", "timeUnit": "s", "outputUnit": "mbps",
    })
    r.check("throughput", "100MB/10s->mbps", thr,
            lambda v: isinstance(v, str) and float(v) > 0)

    tcp = c("tcpThroughput", {
        "bandwidthMbps": "1000", "rttMs": "100", "windowSizeKb": "64"
    })
    r.check("tcpThroughput", "1Gbps/100ms/64kB", tcp,
            lambda v: (isinstance(v, str) and float(v) > 0)
            or (isinstance(v, (int, float)) and float(v) > 0))


def test_analog(r: TestRunner) -> None:
    r.category("analog electronics", 14)
    c = r.client.call

    ohms = c("ohmsLaw", {"voltage": "12", "current": "2", "resistance": "", "power": ""})
    r.check("ohmsLaw", "V=12 I=2", ohms,
            lambda v: isinstance(v, dict) and stripped_equal(v.get("resistance", ""), "6"),
            detail_render=lambda v: f"R={v.get('resistance')}, P={v.get('power')}" if isinstance(v, dict) else repr(v))

    r.check("resistorCombination", "series 10,20,30", c("resistorCombination", {
        "values": "10,20,30", "mode": "series"
    }), lambda v: stripped_equal(v, "60"))

    r.check("capacitorCombination", "parallel 10,20", c("capacitorCombination", {
        "values": "10,20", "mode": "parallel"
    }), lambda v: stripped_equal(v, "30"))

    r.check("inductorCombination", "series 5,10", c("inductorCombination", {
        "values": "5,10", "mode": "series"
    }), lambda v: stripped_equal(v, "15"))

    r.check("voltageDivider", "10, 1k, 1k", c("voltageDivider", {
        "vin": "10", "r1": "1000", "r2": "1000"
    }), lambda v: stripped_equal(v, "5"))

    cdiv = c("currentDivider", {"totalCurrent": "2", "r1": "1000", "r2": "1000"})
    r.check("currentDivider", "2A split", cdiv,
            lambda v: isinstance(v, dict) and "i1" in v and "i2" in v,
            detail_render=lambda v: f"i1={v.get('i1')}, i2={v.get('i2')}" if isinstance(v, dict) else repr(v))

    rc = c("rcTimeConstant", {"resistance": "1000", "capacitance": "0.000001"})
    r.check("rcTimeConstant", "1k, 1uF", rc,
            lambda v: isinstance(v, dict) and stripped_equal(v.get("tau", ""), "0.001"),
            detail_render=lambda v: f"tau={v.get('tau')}" if isinstance(v, dict) else repr(v))

    rl = c("rlTimeConstant", {"resistance": "10", "inductance": "0.001"})
    r.check("rlTimeConstant", "10, 1mH", rl,
            lambda v: isinstance(v, dict) and "tau" in v,
            detail_render=lambda v: f"tau={v.get('tau')}" if isinstance(v, dict) else repr(v))

    rlc = c("rlcResonance", {"r": "10", "l": "0.001", "c": "0.000001"})
    r.check("rlcResonance", "", rlc,
            lambda v: isinstance(v, dict) and "resonantFrequency" in v,
            detail_render=lambda v: f"fr={v.get('resonantFrequency')}" if isinstance(v, dict) else repr(v))

    imp = c("impedance", {
        "r": "100", "l": "0.001", "c": "0.000001", "frequency": "1000"
    })
    r.check("impedance", "RLC @ 1kHz", imp,
            lambda v: isinstance(v, dict) and "magnitude" in v and "phase" in v,
            detail_render=lambda v: f"|Z|={v.get('magnitude')}, ph={v.get('phase')}" if isinstance(v, dict) else repr(v))

    db = c("decibelConvert", {"value": "100", "mode": "powerToDb"})
    r.check("decibelConvert", "100 powerToDb", db,
            lambda v: TestRunner.close(v, 20.0, 1e-6))

    fc = c("filterCutoff", {
        "resistance": "1000", "reactive": "0.000001", "filterType": "lowpass"
    })
    r.check("filterCutoff", "RC low-pass", fc,
            lambda v: isinstance(v, dict) and "cutoffFrequency" in v,
            detail_render=lambda v: f"fc={v.get('cutoffFrequency')}" if isinstance(v, dict) else repr(v))

    # NOTE: LedResistorParams does NOT carry #[serde(rename_all = "camelCase")],
    # so the forward-current field stays as the Rust name `i_f`.
    led = c("ledResistor", {"vs": "5", "vf": "2", "i_f": "0.02"})
    r.check("ledResistor", "5V/2V/20mA", led,
            lambda v: TestRunner.close(v, 150.0, 0.5))

    wh = c("wheatstoneBridge", {"r1": "100", "r2": "200", "r3": "300"})
    r.check("wheatstoneBridge", "R1=100 R2=200 R3=300", wh,
            lambda v: TestRunner.close(v, 600.0, 1e-4))


def test_digital(r: TestRunner) -> None:
    r.category("digital electronics", 10)
    c = r.client.call

    r.check("convertBase", "255 dec->hex", c("convertBase", {
        "value": "255", "fromBase": 10, "toBase": 16
    }), lambda v: v == "FF")

    r.check("twosComplement", "-5 8-bit", c("twosComplement", {
        "value": "-5", "bits": 8, "direction": "toTwos"
    }), lambda v: v == "11111011")

    r.check("grayCode", "1010 toGray", c("grayCode", {
        "value": "1010", "direction": "toGray"
    }), lambda v: v == "1111")

    bw = c("bitwiseOp", {"a": "12", "b": "10", "operation": "AND"})
    r.check("bitwiseOp", "12 AND 10", bw,
            lambda v: isinstance(v, dict) and v.get("decimal") == "8",
            detail_render=lambda v: f"decimal={v.get('decimal')}, binary={v.get('binary')}" if isinstance(v, dict) else repr(v))

    adc = c("adcResolution", {"bits": 10, "vref": "5"})
    r.check("adcResolution", "10-bit @5V", adc,
            lambda v: isinstance(v, dict) and "lsb" in v and "stepCount" in v,
            detail_render=lambda v: f"lsb={v.get('lsb')}, stepCount={v.get('stepCount')}" if isinstance(v, dict) else repr(v))

    dac = c("dacOutput", {"bits": 10, "vref": "5", "code": 512})
    r.check("dacOutput", "10-bit code=512", dac,
            lambda v: TestRunner.close(v, 2.5, 0.01))

    ast = c("timer555Astable", {"r1": "1000", "r2": "1000", "c": "0.000001"})
    r.check("timer555Astable", "R1=R2=1k C=1uF", ast,
            lambda v: isinstance(v, dict) and "frequency" in v,
            detail_render=lambda v: f"f={v.get('frequency')}" if isinstance(v, dict) else repr(v))

    # Timer555MonostableParams uses fields `r` and `c`, NOT resistance/capacitance.
    mono = c("timer555Monostable", {"r": "1000", "c": "0.000001"})
    r.check("timer555Monostable", "R=1k C=1uF", mono,
            lambda v: isinstance(v, dict) and "pulseWidth" in v,
            detail_render=lambda v: f"pulseWidth={v.get('pulseWidth')}" if isinstance(v, dict) else repr(v))

    fp = c("frequencyPeriod", {"value": "1000", "mode": "freqToPeriod"})
    r.check("frequencyPeriod", "1000 freqToPeriod", fp,
            lambda v: stripped_equal(v, "0.001"))

    # NyquistParams uses `bandwidth_hz` which camelCases to `bandwidthHz`.
    nyq = c("nyquistRate", {"bandwidthHz": "20000"})
    r.check("nyquistRate", "20kHz", nyq,
            lambda v: isinstance(v, dict) and stripped_equal(v.get("minSampleRate", ""), "40000"),
            detail_render=lambda v: f"minSampleRate={v.get('minSampleRate')}" if isinstance(v, dict) else repr(v))


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
        print(f"Server reported {len(tool_names)} tools via tools/list")

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
        print(f"RESULTS: {passed}/{total} passed, {total - passed} failed")
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
