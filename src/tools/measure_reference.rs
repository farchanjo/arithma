//! Port of `MeasureReferenceTool.java` — introspection helpers for the unit
//! registry (category listing, unit listing, conversion factor, explanation).
//!
//! All public functions return `String` using the response envelope: inline
//! payload on success, three-line envelope on failure.

use crate::engine::bigdecimal_ext::strip_plain;
use crate::engine::unit_registry::{self, UnitCategory, UnitError};
use crate::mcp::message::{ErrorCode, Response, error, error_with_detail};

const TOOL_LIST_CATEGORIES: &str = "LIST_CATEGORIES";
const TOOL_LIST_UNITS: &str = "LIST_UNITS";
const TOOL_GET_CONVERSION_FACTOR: &str = "GET_CONVERSION_FACTOR";
const TOOL_EXPLAIN_CONVERSION: &str = "EXPLAIN_CONVERSION";

/// List every registered category as `COUNT: n | VALUES: csv`.
#[must_use]
pub fn list_categories() -> String {
    let values: Vec<&str> = unit_registry::list_categories()
        .iter()
        .map(UnitCategory::as_str)
        .collect();
    Response::ok(TOOL_LIST_CATEGORIES)
        .field("COUNT", values.len().to_string())
        .field("VALUES", values.join(","))
        .build()
}

/// List every unit in `category` as `CATEGORY: X | COUNT: n | VALUES: csv`.
#[must_use]
pub fn list_units(category: &str) -> String {
    let cat = match UnitCategory::parse(category) {
        Ok(c) => c,
        Err(UnitError::UnknownCategory(name)) => {
            return error_with_detail(
                TOOL_LIST_UNITS,
                ErrorCode::InvalidInput,
                "category is not a recognized category",
                &format!("category={name}"),
            );
        }
        Err(other) => {
            return error(TOOL_LIST_UNITS, ErrorCode::InvalidInput, &other.to_string());
        }
    };
    let units = unit_registry::list_units(cat);
    if units.is_empty() {
        return error_with_detail(
            TOOL_LIST_UNITS,
            ErrorCode::InvalidInput,
            "category has no registered units",
            &format!("category={category}"),
        );
    }
    let values: Vec<&str> = units.iter().map(|u| u.code.as_str()).collect();
    Response::ok(TOOL_LIST_UNITS)
        .field("CATEGORY", cat.as_str())
        .field("COUNT", values.len().to_string())
        .field("VALUES", values.join(","))
        .build()
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
        UnitError::TemperatureFactor => error(
            tool,
            ErrorCode::InvalidInput,
            "temperature uses formulas, not a fixed factor",
        ),
        other => error(tool, ErrorCode::InvalidInput, &other.to_string()),
    }
}

/// Return the multiplicative factor that maps `from_unit` to `to_unit`.
#[must_use]
pub fn get_conversion_factor(from_unit: &str, to_unit: &str) -> String {
    match unit_registry::conversion_factor(from_unit, to_unit) {
        Ok(factor) => Response::ok(TOOL_GET_CONVERSION_FACTOR)
            .result(strip_plain(&factor))
            .build(),
        Err(e) => map_registry_error(TOOL_GET_CONVERSION_FACTOR, from_unit, to_unit, &e),
    }
}

/// Human-readable explanation of a unit conversion. Multi-field envelope:
/// `FROM | TO | FACTOR | FORMULA`. For temperature pairs `FACTOR` is empty
/// and `FORMULA` carries the relationship; for linear pairs `FORMULA` is
/// empty and `FACTOR` carries the multiplier.
#[must_use]
pub fn explain_conversion(from_unit: &str, to_unit: &str) -> String {
    // The registry returns either "1 <from_name> = <factor> <to_name>" for
    // linear pairs or a formula string for temperature pairs. Split those
    // apart so the envelope exposes them as discrete fields.
    let raw = match unit_registry::explain_conversion(from_unit, to_unit) {
        Ok(s) => s,
        Err(e) => return map_registry_error(TOOL_EXPLAIN_CONVERSION, from_unit, to_unit, &e),
    };

    let (factor, formula) = split_factor_and_formula(&raw);
    Response::ok(TOOL_EXPLAIN_CONVERSION)
        .field("FROM", from_unit.to_string())
        .field("TO", to_unit.to_string())
        .field("FACTOR", factor)
        .field("FORMULA", formula)
        .build()
}

/// Classify the registry explanation string. Linear explanations start with
/// `"1 "` and carry `" = <factor> "` in the middle. Everything else (the four
/// temperature formulas) is surfaced as a formula-only response.
fn split_factor_and_formula(raw: &str) -> (String, String) {
    if let Some(rest) = raw.strip_prefix("1 ")
        && let Some(eq_idx) = rest.find(" = ")
    {
        let after_eq = &rest[eq_idx + 3..];
        if let Some(space_idx) = after_eq.find(' ') {
            let factor = &after_eq[..space_idx];
            return (factor.to_string(), String::new());
        }
    }
    (String::new(), raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_categories_inline_envelope() {
        let out = list_categories();
        assert!(
            out.starts_with("LIST_CATEGORIES: OK | COUNT: 21 | VALUES: "),
            "got {out}"
        );
        assert!(out.contains("DATA_STORAGE"), "got {out}");
        assert!(out.contains("CURRENT"), "got {out}");
    }

    #[test]
    fn list_units_length() {
        assert_eq!(
            list_units("LENGTH"),
            "LIST_UNITS: OK | CATEGORY: LENGTH | COUNT: 9 | VALUES: m,mm,cm,km,in,ft,yd,mi,nmi"
        );
    }

    #[test]
    fn list_units_case_insensitive_category() {
        assert_eq!(
            list_units("length"),
            "LIST_UNITS: OK | CATEGORY: LENGTH | COUNT: 9 | VALUES: m,mm,cm,km,in,ft,yd,mi,nmi"
        );
    }

    #[test]
    fn list_units_unknown_category() {
        assert_eq!(
            list_units("BOGUS"),
            "LIST_UNITS: ERROR\nREASON: [INVALID_INPUT] category is not a recognized category\nDETAIL: category=BOGUS"
        );
    }

    #[test]
    fn conversion_factor_km_to_m() {
        assert_eq!(
            get_conversion_factor("km", "m"),
            "GET_CONVERSION_FACTOR: OK | RESULT: 1000"
        );
    }

    #[test]
    fn conversion_factor_temperature_error() {
        assert_eq!(
            get_conversion_factor("c", "f"),
            "GET_CONVERSION_FACTOR: ERROR\nREASON: [INVALID_INPUT] temperature uses formulas, not a fixed factor"
        );
    }

    #[test]
    fn conversion_factor_unknown_unit() {
        assert_eq!(
            get_conversion_factor("zzz", "m"),
            "GET_CONVERSION_FACTOR: ERROR\nREASON: [INVALID_INPUT] unit is not a recognized unit\nDETAIL: unit=zzz"
        );
    }

    #[test]
    fn explain_conversion_linear() {
        assert_eq!(
            explain_conversion("km", "mi"),
            "EXPLAIN_CONVERSION: OK | FROM: km | TO: mi | FACTOR: 0.6213711922373339696174341843633182 | FORMULA: "
        );
    }

    #[test]
    fn explain_conversion_temperature_formula() {
        assert_eq!(
            explain_conversion("c", "f"),
            "EXPLAIN_CONVERSION: OK | FROM: c | TO: f | FACTOR:  | FORMULA: F = C * 9/5 + 32"
        );
        assert_eq!(
            explain_conversion("k", "c"),
            "EXPLAIN_CONVERSION: OK | FROM: k | TO: c | FACTOR:  | FORMULA: C = K - 273.15"
        );
    }

    #[test]
    fn explain_conversion_cross_category_error() {
        assert_eq!(
            explain_conversion("kg", "m"),
            "EXPLAIN_CONVERSION: ERROR\nREASON: [INVALID_INPUT] units are not in the same category\nDETAIL: from=kg, to=m"
        );
    }
}
