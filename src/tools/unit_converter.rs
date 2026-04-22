//! Port of `UnitConverterTool.java` — measurement unit conversions with
//! category validation and auto-detection.
//!
//! All public functions return `String` using the response envelope: inline
//! `RESULT: value` on success, three-line error envelope on failure.

use std::str::FromStr;

use bigdecimal::BigDecimal;

use crate::engine::bigdecimal_ext::strip_plain;
use crate::engine::unit_registry::{self, UnitCategory, UnitError};
use crate::mcp::message::{ErrorCode, Response, error, error_with_detail};

const TOOL_CONVERT: &str = "CONVERT";
const TOOL_CONVERT_AUTO_DETECT: &str = "CONVERT_AUTO_DETECT";

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

fn validate_category(
    tool: &str,
    code: &str,
    expected: UnitCategory,
    other_code: &str,
) -> Result<(), String> {
    match unit_registry::find_unit(code) {
        None => Err(error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            "unit is not a recognized unit",
            &format!("unit={code}"),
        )),
        Some(def) if def.category != expected => Err(error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            "unit is not in the requested category",
            &format!(
                "from={code}, to={other_code}, category={}",
                expected.as_str()
            ),
        )),
        Some(_) => Ok(()),
    }
}

fn category_error(tool: &str, err: &UnitError) -> String {
    match err {
        UnitError::UnknownCategory(name) => error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            "category is not a recognized category",
            &format!("category={name}"),
        ),
        other => error(tool, ErrorCode::InvalidInput, &other.to_string()),
    }
}

fn convert_error(tool: &str, from_unit: &str, to_unit: &str, err: &UnitError) -> String {
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
        UnitError::UnknownTemperatureUnit(code) => error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            "temperature unit is not recognized",
            &format!("unit={code}"),
        ),
        other => error(tool, ErrorCode::InvalidInput, &other.to_string()),
    }
}

/// Convert `value` between two units within an explicit `category`.
#[must_use]
pub fn convert(value: &str, from_unit: &str, to_unit: &str, category: &str) -> String {
    let cat = match UnitCategory::parse(category) {
        Ok(c) => c,
        Err(e) => return category_error(TOOL_CONVERT, &e),
    };
    if let Err(msg) = validate_category(TOOL_CONVERT, from_unit, cat, to_unit) {
        return msg;
    }
    if let Err(msg) = validate_category(TOOL_CONVERT, to_unit, cat, from_unit) {
        return msg;
    }
    let parsed = match parse_value(TOOL_CONVERT, value) {
        Ok(v) => v,
        Err(msg) => return msg,
    };
    match unit_registry::convert(&parsed, from_unit, to_unit) {
        Ok(out) => Response::ok(TOOL_CONVERT).result(strip_plain(&out)).build(),
        Err(e) => convert_error(TOOL_CONVERT, from_unit, to_unit, &e),
    }
}

/// Convert `value` between two units, auto-detecting the shared category.
#[must_use]
pub fn convert_auto_detect(value: &str, from_unit: &str, to_unit: &str) -> String {
    let parsed = match parse_value(TOOL_CONVERT_AUTO_DETECT, value) {
        Ok(v) => v,
        Err(msg) => return msg,
    };
    match unit_registry::convert(&parsed, from_unit, to_unit) {
        Ok(out) => Response::ok(TOOL_CONVERT_AUTO_DETECT)
            .result(strip_plain(&out))
            .build(),
        Err(e) => convert_error(TOOL_CONVERT_AUTO_DETECT, from_unit, to_unit, &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn km_to_m_happy_path() {
        assert_eq!(
            convert("1", "km", "m", "LENGTH"),
            "CONVERT: OK | RESULT: 1000"
        );
    }

    #[test]
    fn category_parse_case_insensitive() {
        assert_eq!(
            convert("1", "km", "m", "length"),
            "CONVERT: OK | RESULT: 1000"
        );
    }

    #[test]
    fn cross_category_unit_in_wrong_category() {
        assert_eq!(
            convert("1", "kg", "m", "LENGTH"),
            "CONVERT: ERROR\nREASON: [INVALID_INPUT] unit is not in the requested category\nDETAIL: from=kg, to=m, category=LENGTH"
        );
    }

    #[test]
    fn unknown_category_error() {
        assert_eq!(
            convert("1", "km", "m", "NOT_A_CATEGORY"),
            "CONVERT: ERROR\nREASON: [INVALID_INPUT] category is not a recognized category\nDETAIL: category=NOT_A_CATEGORY"
        );
    }

    #[test]
    fn unknown_unit_error() {
        assert_eq!(
            convert("1", "zzz", "m", "LENGTH"),
            "CONVERT: ERROR\nREASON: [INVALID_INPUT] unit is not a recognized unit\nDETAIL: unit=zzz"
        );
    }

    #[test]
    fn auto_detect_km_to_mi() {
        assert_eq!(
            convert_auto_detect("1", "km", "mi"),
            "CONVERT_AUTO_DETECT: OK | RESULT: 0.6213711922373339696174341843633182"
        );
    }

    #[test]
    fn auto_detect_cross_category_error() {
        assert_eq!(
            convert_auto_detect("1", "kg", "m"),
            "CONVERT_AUTO_DETECT: ERROR\nREASON: [INVALID_INPUT] units are not in the same category\nDETAIL: from=kg, to=m"
        );
    }

    #[test]
    fn auto_detect_temperature_celsius_fahrenheit() {
        assert_eq!(
            convert_auto_detect("100", "c", "f"),
            "CONVERT_AUTO_DETECT: OK | RESULT: 212"
        );
    }

    #[test]
    fn invalid_number_returns_error() {
        assert_eq!(
            convert("not-a-number", "km", "m", "LENGTH"),
            "CONVERT: ERROR\nREASON: [PARSE_ERROR] value is not a valid decimal number\nDETAIL: value=not-a-number"
        );
    }
}
