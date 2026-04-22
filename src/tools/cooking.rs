//! Port of `CookingConverterTool.java` — narrow cooking-specific converters
//! for volume, weight, and oven temperature (including UK gas mark).
//!
//! All public functions return `String` using the response envelope: inline
//! `RESULT: value` on success, three-line error envelope on failure.

use std::str::FromStr;

use bigdecimal::BigDecimal;
use num_traits::{Signed, ToPrimitive};

use crate::engine::bigdecimal_ext::strip_plain;
use crate::engine::unit_registry::{self, UnitError};
use crate::mcp::message::{ErrorCode, Response, error, error_with_detail};

const TOOL_VOLUME: &str = "CONVERT_COOKING_VOLUME";
const TOOL_WEIGHT: &str = "CONVERT_COOKING_WEIGHT";
const TOOL_OVEN: &str = "CONVERT_OVEN_TEMPERATURE";

const VOLUME_UNITS: &[&str] = &[
    "l", "ml", "uscup", "cup", "tbsp", "tsp", "usfloz", "floz", "usgal", "gal", "igal",
];
const WEIGHT_UNITS: &[&str] = &["kg", "g", "mg", "lb", "oz"];
const TEMP_UNITS: &[&str] = &["c", "f"];
const GAS_MARK: &str = "gasmark";

fn resolve_volume_alias(code: &str) -> String {
    let lower = code.to_ascii_lowercase();
    match lower.as_str() {
        "cup" => "uscup".to_string(),
        "floz" => "usfloz".to_string(),
        "gal" => "usgal".to_string(),
        _ => lower,
    }
}

fn parse_value(tool: &str, raw: &str) -> Result<BigDecimal, String> {
    BigDecimal::from_str(raw).map_err(|_| {
        error_with_detail(
            tool,
            ErrorCode::ParseError,
            "value is not a valid decimal number",
            &format!("value={raw}"),
        )
    })
}

fn reject_negative(tool: &str, value: &BigDecimal, raw: &str) -> Result<(), String> {
    if value.is_negative() {
        Err(error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            "value must not be negative",
            &format!("value={raw}"),
        ))
    } else {
        Ok(())
    }
}

fn validate_allowed(tool: &str, code: &str, allowed: &[&str]) -> Result<(), String> {
    let lower = code.to_ascii_lowercase();
    if allowed.iter().any(|a| *a == lower) {
        Ok(())
    } else {
        Err(error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            "unit is not a recognized unit",
            &format!("unit={code}"),
        ))
    }
}

fn map_registry_error(tool: &str, from_unit: &str, to_unit: &str, err: &UnitError) -> String {
    match err {
        UnitError::UnknownUnit(code) => error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            "unit is not a recognized unit",
            &format!("unit={code}"),
        ),
        UnitError::CrossCategory { .. } | UnitError::WrongCategory { .. } => error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            "units are not in the same category",
            &format!("from={from_unit}, to={to_unit}"),
        ),
        other => error(tool, ErrorCode::InvalidInput, &other.to_string()),
    }
}

/// Convert a cooking volume (l, ml, cups, tbsp, tsp, floz, gal, etc.).
#[must_use]
pub fn convert_cooking_volume(value: &str, from_unit: &str, to_unit: &str) -> String {
    if let Err(msg) = validate_allowed(TOOL_VOLUME, from_unit, VOLUME_UNITS) {
        return msg;
    }
    if let Err(msg) = validate_allowed(TOOL_VOLUME, to_unit, VOLUME_UNITS) {
        return msg;
    }
    let from = resolve_volume_alias(from_unit);
    let dest = resolve_volume_alias(to_unit);
    let parsed = match parse_value(TOOL_VOLUME, value) {
        Ok(v) => v,
        Err(msg) => return msg,
    };
    if let Err(msg) = reject_negative(TOOL_VOLUME, &parsed, value) {
        return msg;
    }
    match unit_registry::convert(&parsed, &from, &dest) {
        Ok(out) => Response::ok(TOOL_VOLUME).result(strip_plain(&out)).build(),
        Err(e) => map_registry_error(TOOL_VOLUME, from_unit, to_unit, &e),
    }
}

/// Convert a cooking weight (kg, g, mg, lb, oz).
#[must_use]
pub fn convert_cooking_weight(value: &str, from_unit: &str, to_unit: &str) -> String {
    if let Err(msg) = validate_allowed(TOOL_WEIGHT, from_unit, WEIGHT_UNITS) {
        return msg;
    }
    if let Err(msg) = validate_allowed(TOOL_WEIGHT, to_unit, WEIGHT_UNITS) {
        return msg;
    }
    let parsed = match parse_value(TOOL_WEIGHT, value) {
        Ok(v) => v,
        Err(msg) => return msg,
    };
    if let Err(msg) = reject_negative(TOOL_WEIGHT, &parsed, value) {
        return msg;
    }
    match unit_registry::convert(&parsed, from_unit, to_unit) {
        Ok(out) => Response::ok(TOOL_WEIGHT).result(strip_plain(&out)).build(),
        Err(e) => map_registry_error(TOOL_WEIGHT, from_unit, to_unit, &e),
    }
}

fn validate_oven_unit(code: &str) -> Result<(), String> {
    if TEMP_UNITS.contains(&code) {
        Ok(())
    } else {
        Err(error_with_detail(
            TOOL_OVEN,
            ErrorCode::InvalidInput,
            "oven temperature unit must be c, f, or gasmark",
            &format!("unit={code}"),
        ))
    }
}

fn oven_to_celsius(input: &BigDecimal, source: &str) -> Result<BigDecimal, String> {
    if source == GAS_MARK {
        let mark = input.to_i32().ok_or_else(|| {
            error_with_detail(
                TOOL_OVEN,
                ErrorCode::InvalidInput,
                "gas mark must be an integer",
                &format!("value={input}"),
            )
        })?;
        unit_registry::gas_mark_to_celsius(mark)
            .map_err(|e| error(TOOL_OVEN, ErrorCode::InvalidInput, &e.to_string()))
    } else {
        validate_oven_unit(source)?;
        unit_registry::to_celsius(source, input)
            .map_err(|e| error(TOOL_OVEN, ErrorCode::InvalidInput, &e.to_string()))
    }
}

fn celsius_to_oven(celsius: &BigDecimal, target: &str) -> Result<BigDecimal, String> {
    if target == GAS_MARK {
        let mark = unit_registry::celsius_to_gas_mark(celsius)
            .map_err(|e| error(TOOL_OVEN, ErrorCode::InvalidInput, &e.to_string()))?;
        Ok(BigDecimal::from(mark))
    } else {
        validate_oven_unit(target)?;
        unit_registry::from_celsius(target, celsius)
            .map_err(|e| error(TOOL_OVEN, ErrorCode::InvalidInput, &e.to_string()))
    }
}

/// Convert an oven temperature between Celsius, Fahrenheit, and UK gas mark.
#[must_use]
pub fn convert_oven_temperature(value: &str, from_unit: &str, to_unit: &str) -> String {
    let source = from_unit.to_ascii_lowercase();
    let target = to_unit.to_ascii_lowercase();
    let input = match parse_value(TOOL_OVEN, value) {
        Ok(v) => v,
        Err(msg) => return msg,
    };
    let celsius = match oven_to_celsius(&input, &source) {
        Ok(v) => v,
        Err(msg) => return msg,
    };
    match celsius_to_oven(&celsius, &target) {
        Ok(out) => Response::ok(TOOL_OVEN).result(strip_plain(&out)).build(),
        Err(msg) => msg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weight_lb_to_oz_happy_path() {
        assert_eq!(
            convert_cooking_weight("1", "lb", "oz"),
            "CONVERT_COOKING_WEIGHT: OK | RESULT: 16"
        );
    }

    #[test]
    fn weight_invalid_unit() {
        assert_eq!(
            convert_cooking_weight("1", "lb", "m"),
            "CONVERT_COOKING_WEIGHT: ERROR\nREASON: [INVALID_INPUT] unit is not a recognized unit\nDETAIL: unit=m"
        );
    }

    #[test]
    fn volume_cup_alias_to_tbsp() {
        assert_eq!(
            convert_cooking_volume("1", "cup", "tbsp"),
            "CONVERT_COOKING_VOLUME: OK | RESULT: 16"
        );
    }

    #[test]
    fn volume_floz_alias_to_ml() {
        assert_eq!(
            convert_cooking_volume("1", "floz", "ml"),
            "CONVERT_COOKING_VOLUME: OK | RESULT: 29.5735295625"
        );
    }

    #[test]
    fn volume_gal_alias_to_l() {
        assert_eq!(
            convert_cooking_volume("1", "gal", "l"),
            "CONVERT_COOKING_VOLUME: OK | RESULT: 3.785411784"
        );
    }

    #[test]
    fn volume_invalid_unit() {
        assert_eq!(
            convert_cooking_volume("1", "kg", "l"),
            "CONVERT_COOKING_VOLUME: ERROR\nREASON: [INVALID_INPUT] unit is not a recognized unit\nDETAIL: unit=kg"
        );
    }

    #[test]
    fn oven_celsius_to_fahrenheit() {
        assert_eq!(
            convert_oven_temperature("100", "c", "f"),
            "CONVERT_OVEN_TEMPERATURE: OK | RESULT: 212"
        );
    }

    #[test]
    fn oven_gas_mark_to_celsius() {
        assert_eq!(
            convert_oven_temperature("4", "gasmark", "c"),
            "CONVERT_OVEN_TEMPERATURE: OK | RESULT: 180"
        );
    }

    #[test]
    fn oven_celsius_to_gas_mark() {
        assert_eq!(
            convert_oven_temperature("200", "c", "gasmark"),
            "CONVERT_OVEN_TEMPERATURE: OK | RESULT: 6"
        );
    }

    #[test]
    fn oven_invalid_unit_error() {
        assert_eq!(
            convert_oven_temperature("100", "k", "c"),
            "CONVERT_OVEN_TEMPERATURE: ERROR\nREASON: [INVALID_INPUT] oven temperature unit must be c, f, or gasmark\nDETAIL: unit=k"
        );
    }

    #[test]
    fn oven_gasmark_roundtrip() {
        assert_eq!(
            convert_oven_temperature("6", "gasmark", "gasmark"),
            "CONVERT_OVEN_TEMPERATURE: OK | RESULT: 6"
        );
    }

    #[test]
    fn oven_fahrenheit_to_gasmark() {
        assert_eq!(
            convert_oven_temperature("356", "f", "gasmark"),
            "CONVERT_OVEN_TEMPERATURE: OK | RESULT: 4"
        );
    }

    #[test]
    fn volume_rejects_negative_value() {
        assert_eq!(
            convert_cooking_volume("-1", "cup", "ml"),
            "CONVERT_COOKING_VOLUME: ERROR\nREASON: [INVALID_INPUT] value must not be negative\nDETAIL: value=-1"
        );
    }

    #[test]
    fn weight_rejects_negative_value() {
        assert_eq!(
            convert_cooking_weight("-5", "kg", "g"),
            "CONVERT_COOKING_WEIGHT: ERROR\nREASON: [INVALID_INPUT] value must not be negative\nDETAIL: value=-5"
        );
    }

    #[test]
    fn oven_allows_negative_celsius() {
        // Temperatures can legitimately be below zero — keep oven converter permissive.
        assert_eq!(
            convert_oven_temperature("-10", "c", "f"),
            "CONVERT_OVEN_TEMPERATURE: OK | RESULT: 14"
        );
    }
}
