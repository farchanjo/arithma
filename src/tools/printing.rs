//! Port of `PrintingCalculatorTool.java` — tape calculator with running totals.
//!
//! Behavioral parity:
//! * Arithmetic is exact, backed by `bigdecimal::BigDecimal`.
//! * Division scale = 20, rounding mode = HALF_UP.
//! * Display scale = 2 (applied per row and to the final total).
//! * `=` emits a subtotal row. `C` clears the running total. `T` emits the
//!   grand total and resets.
//!
//! Response envelope uses block layout: a `FINAL` and `STEPS` header followed
//! by `ROW_N: op=... | value=... | running=...` rows. Errors surface through
//! the standard three-line envelope.

use std::str::FromStr;

use bigdecimal::{BigDecimal, RoundingMode};
use num_traits::Zero;
use serde_json::Value;

use crate::mcp::message::{ErrorCode, Response, error, error_with_detail};

const TOOL_CALCULATE_WITH_TAPE: &str = "CALCULATE_WITH_TAPE";
const DISPLAY_SCALE: i64 = 2;
const DIVISION_SCALE: i64 = 20;

/// Run a sequence of operations and return the tape envelope.
pub fn calculate_with_tape(operations_json: &str) -> String {
    if operations_json.trim().is_empty() {
        return error(
            TOOL_CALCULATE_WITH_TAPE,
            ErrorCode::InvalidInput,
            "operations must not be null or empty",
        );
    }

    let entries = match parse_entries(operations_json) {
        Ok(entries) => entries,
        Err(msg) => return msg,
    };

    match build_rows(&entries) {
        Ok((rows, final_total)) => render_envelope(&rows, &final_total),
        Err(msg) => msg,
    }
}

// --------------------------------------------------------------------------- //
//  Parsing
// --------------------------------------------------------------------------- //

#[derive(Debug)]
struct Entry {
    operation: String,
    value: Option<String>,
}

fn parse_entries(json: &str) -> Result<Vec<Entry>, String> {
    let parsed: Value = serde_json::from_str(json).map_err(|_| {
        error(
            TOOL_CALCULATE_WITH_TAPE,
            ErrorCode::ParseError,
            "operations must be a JSON array",
        )
    })?;

    let array = parsed.as_array().ok_or_else(|| {
        error(
            TOOL_CALCULATE_WITH_TAPE,
            ErrorCode::ParseError,
            "operations must be a JSON array",
        )
    })?;

    let mut entries = Vec::with_capacity(array.len());
    for item in array {
        let obj = item.as_object().ok_or_else(|| {
            error(
                TOOL_CALCULATE_WITH_TAPE,
                ErrorCode::ParseError,
                "operations must be a JSON array",
            )
        })?;

        let op = obj.get("op").ok_or_else(|| {
            error(
                TOOL_CALCULATE_WITH_TAPE,
                ErrorCode::InvalidInput,
                "missing field: op",
            )
        })?;
        let op_str = match op {
            Value::String(s) => s.clone(),
            Value::Null => {
                return Err(error(
                    TOOL_CALCULATE_WITH_TAPE,
                    ErrorCode::InvalidInput,
                    "missing field: op",
                ));
            }
            other => other.to_string(),
        };

        if !obj.contains_key("value") {
            return Err(error(
                TOOL_CALCULATE_WITH_TAPE,
                ErrorCode::InvalidInput,
                "missing field: value",
            ));
        }
        let value_str = match obj.get("value") {
            Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Number(n)) => Some(n.to_string()),
            Some(Value::Bool(b)) => Some(b.to_string()),
            Some(_) | None => {
                return Err(error(
                    TOOL_CALCULATE_WITH_TAPE,
                    ErrorCode::InvalidInput,
                    "missing field: value",
                ));
            }
        };

        entries.push(Entry {
            operation: op_str,
            value: value_str,
        });
    }
    Ok(entries)
}

// --------------------------------------------------------------------------- //
//  Tape rows
// --------------------------------------------------------------------------- //

#[derive(Debug)]
struct Row {
    operation: String,
    value: String,
    running: String,
}

fn build_rows(entries: &[Entry]) -> Result<(Vec<Row>, BigDecimal), String> {
    let mut total = BigDecimal::zero();
    let mut rows: Vec<Row> = Vec::with_capacity(entries.len());

    for entry in entries {
        if needs_value(&entry.operation) {
            let raw = entry.value.as_deref().ok_or_else(|| {
                error(
                    TOOL_CALCULATE_WITH_TAPE,
                    ErrorCode::InvalidInput,
                    "missing field: value",
                )
            })?;
            let value = BigDecimal::from_str(raw).map_err(|_| {
                error_with_detail(
                    TOOL_CALCULATE_WITH_TAPE,
                    ErrorCode::ParseError,
                    "value is not a valid decimal number",
                    &format!("value={raw}"),
                )
            })?;
            total = apply_arithmetic(&total, &value, &entry.operation)?;
            rows.push(Row {
                operation: entry.operation.clone(),
                value: display(&value),
                running: display(&total),
            });
        } else {
            total = apply_control(&mut rows, &total, &entry.operation)?;
        }
    }

    Ok((rows, total))
}

fn apply_arithmetic(
    total: &BigDecimal,
    value: &BigDecimal,
    operation: &str,
) -> Result<BigDecimal, String> {
    match operation {
        "+" => Ok(total + value),
        "-" => Ok(total - value),
        "*" => Ok(total * value),
        "/" => {
            if value.is_zero() {
                return Err(error(
                    TOOL_CALCULATE_WITH_TAPE,
                    ErrorCode::DivisionByZero,
                    "division by zero",
                ));
            }
            Ok(total.with_scale_round(DIVISION_SCALE, RoundingMode::HalfUp)
                / value.with_scale_round(DIVISION_SCALE, RoundingMode::HalfUp))
        }
        other => Err(error_with_detail(
            TOOL_CALCULATE_WITH_TAPE,
            ErrorCode::InvalidInput,
            "unknown arithmetic operation",
            &format!("op={other}"),
        )),
    }
}

fn apply_control(
    rows: &mut Vec<Row>,
    total: &BigDecimal,
    operation: &str,
) -> Result<BigDecimal, String> {
    match operation {
        "=" => {
            rows.push(Row {
                operation: "=".to_string(),
                value: display(total),
                running: display(total),
            });
            Ok(total.clone())
        }
        "C" => {
            let zero = BigDecimal::zero();
            rows.push(Row {
                operation: "C".to_string(),
                value: display(&zero),
                running: display(&zero),
            });
            Ok(zero)
        }
        "T" => {
            rows.push(Row {
                operation: "T".to_string(),
                value: display(total),
                running: display(total),
            });
            Ok(BigDecimal::zero())
        }
        other => Err(error_with_detail(
            TOOL_CALCULATE_WITH_TAPE,
            ErrorCode::InvalidInput,
            "unknown control operation",
            &format!("op={other}"),
        )),
    }
}

fn needs_value(operation: &str) -> bool {
    matches!(operation, "+" | "-" | "*" | "/")
}

fn display(value: &BigDecimal) -> String {
    value
        .with_scale_round(DISPLAY_SCALE, RoundingMode::HalfUp)
        .to_plain_string()
}

// --------------------------------------------------------------------------- //
//  Envelope rendering
// --------------------------------------------------------------------------- //

fn render_envelope(rows: &[Row], final_total: &BigDecimal) -> String {
    let mut builder = Response::ok(TOOL_CALCULATE_WITH_TAPE)
        .field("FINAL", display(final_total))
        .field("STEPS", rows.len().to_string());
    for (idx, row) in rows.iter().enumerate() {
        let key = format!("ROW_{}", idx + 1);
        let line = format!(
            "op={} | value={} | running={}",
            row.operation, row.value, row.running
        );
        builder = builder.field(key, line);
    }
    builder.block().build()
}

// --------------------------------------------------------------------------- //
//  Tests
// --------------------------------------------------------------------------- //

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_subtract_with_subtotal() {
        let json = r#"[{"op":"+","value":"100"},{"op":"-","value":"30"},{"op":"=","value":null}]"#;
        let out = calculate_with_tape(json);
        assert_eq!(
            out,
            "CALCULATE_WITH_TAPE: OK\n\
             FINAL: 70.00\n\
             STEPS: 3\n\
             ROW_1: op=+ | value=100.00 | running=100.00\n\
             ROW_2: op=- | value=30.00 | running=70.00\n\
             ROW_3: op== | value=70.00 | running=70.00"
        );
    }

    #[test]
    fn multiply_divide_and_grand_total() {
        let json = r#"[
            {"op":"+","value":"10"},
            {"op":"*","value":"5"},
            {"op":"/","value":"2"},
            {"op":"T","value":null}
        ]"#;
        let out = calculate_with_tape(json);
        // 0 + 10 = 10, *5 = 50, /2 = 25 → T emits 25 and resets to 0.
        assert!(out.starts_with("CALCULATE_WITH_TAPE: OK\n"), "got {out}");
        assert!(out.contains("\nFINAL: 0.00\n"), "got {out}");
        assert!(out.contains("\nSTEPS: 4\n"), "got {out}");
        assert!(
            out.contains("ROW_4: op=T | value=25.00 | running=25.00"),
            "got {out}"
        );
    }

    #[test]
    fn clear_resets_total() {
        let json = r#"[
            {"op":"+","value":"100"},
            {"op":"C","value":null},
            {"op":"+","value":"5"},
            {"op":"=","value":null}
        ]"#;
        let out = calculate_with_tape(json);
        assert!(
            out.contains("ROW_2: op=C | value=0.00 | running=0.00"),
            "got {out}"
        );
        assert!(
            out.contains("ROW_4: op== | value=5.00 | running=5.00"),
            "got {out}"
        );
        assert!(out.contains("\nFINAL: 5.00\n"), "got {out}");
    }

    #[test]
    fn single_add_structure() {
        let json = r#"[{"op":"+","value":"1"}]"#;
        let out = calculate_with_tape(json);
        assert_eq!(
            out,
            "CALCULATE_WITH_TAPE: OK\n\
             FINAL: 1.00\n\
             STEPS: 1\n\
             ROW_1: op=+ | value=1.00 | running=1.00"
        );
    }

    #[test]
    fn err_empty_input() {
        let expected = "CALCULATE_WITH_TAPE: ERROR\nREASON: [INVALID_INPUT] operations must not be null or empty";
        assert_eq!(calculate_with_tape(""), expected);
        assert_eq!(calculate_with_tape("   "), expected);
    }

    #[test]
    fn err_not_a_json_array() {
        let expected = "CALCULATE_WITH_TAPE: ERROR\nREASON: [PARSE_ERROR] operations must be a JSON array";
        assert_eq!(calculate_with_tape("{\"op\":\"+\",\"value\":\"1\"}"), expected);
        assert_eq!(calculate_with_tape("garbage"), expected);
    }

    #[test]
    fn err_missing_op_field() {
        let json = r#"[{"value":"1"}]"#;
        assert_eq!(
            calculate_with_tape(json),
            "CALCULATE_WITH_TAPE: ERROR\nREASON: [INVALID_INPUT] missing field: op"
        );
    }

    #[test]
    fn err_missing_value_field() {
        let json = r#"[{"op":"+"}]"#;
        assert_eq!(
            calculate_with_tape(json),
            "CALCULATE_WITH_TAPE: ERROR\nREASON: [INVALID_INPUT] missing field: value"
        );
    }

    #[test]
    fn err_division_by_zero() {
        let json = r#"[{"op":"+","value":"10"},{"op":"/","value":"0"}]"#;
        assert_eq!(
            calculate_with_tape(json),
            "CALCULATE_WITH_TAPE: ERROR\nREASON: [DIVISION_BY_ZERO] division by zero"
        );
    }

    #[test]
    fn err_unknown_control_op() {
        let json = r#"[{"op":"X","value":null}]"#;
        assert_eq!(
            calculate_with_tape(json),
            "CALCULATE_WITH_TAPE: ERROR\nREASON: [INVALID_INPUT] unknown control operation\nDETAIL: op=X"
        );
        let json = r#"[{"op":"^","value":"2"}]"#;
        assert_eq!(
            calculate_with_tape(json),
            "CALCULATE_WITH_TAPE: ERROR\nREASON: [INVALID_INPUT] unknown control operation\nDETAIL: op=^"
        );
    }

    #[test]
    fn numeric_value_accepted() {
        let json = r#"[{"op":"+","value":42},{"op":"=","value":null}]"#;
        let out = calculate_with_tape(json);
        assert!(
            out.contains("ROW_1: op=+ | value=42.00 | running=42.00"),
            "got {out}"
        );
        assert!(
            out.contains("ROW_2: op== | value=42.00 | running=42.00"),
            "got {out}"
        );
    }
}
