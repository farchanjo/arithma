//! SIMD-accelerated array arithmetic — formatted through the canonical envelope.
//!
//! Uses the portable-SIMD [`wide`] crate which auto-dispatches to SSE2/AVX2/AVX-512/NEON
//! at runtime based on `target-cpu` flags. A 256-bit `f64x4` vector width is used for
//! the hot inner loop; the tail is processed scalar.

use wide::f64x4;

use crate::mcp::message::{ErrorCode, Response, error, error_with_detail};

const TOOL_SUM_ARRAY: &str = "SUM_ARRAY";
const TOOL_DOT_PRODUCT: &str = "DOT_PRODUCT";
const TOOL_SCALE_ARRAY: &str = "SCALE_ARRAY";
const TOOL_MAGNITUDE_ARRAY: &str = "MAGNITUDE_ARRAY";

/// Parse a comma-separated list of f64 values. Each failure surfaces through the
/// tool-scoped envelope so the caller sees which token broke.
///
/// A fully blank input yields an empty `Vec` so the caller can map it to the
/// canonical `INVALID_INPUT "input array must not be empty"` — matching the
/// statistics helpers instead of fielding a spurious `PARSE_ERROR`.
fn parse_array(tool: &str, label: &str, input: &str) -> Result<Vec<f64>, String> {
    if input.trim().is_empty() {
        return Ok(Vec::new());
    }
    let parts: Vec<&str> = input.split(',').collect();
    let mut result = Vec::with_capacity(parts.len());
    for part in parts {
        let trimmed = part.trim();
        match trimmed.parse::<f64>() {
            Ok(value) => result.push(value),
            Err(_) => {
                return Err(error_with_detail(
                    tool,
                    ErrorCode::ParseError,
                    "array element is not a valid number",
                    &format!("{label}={trimmed}"),
                ));
            }
        }
    }
    Ok(result)
}

/// Format a single f64 the same way Java's `String.valueOf(double)` does.
fn format_f64(value: f64) -> String {
    format!("{value:?}")
}

fn ok_result(tool: &str, value: String) -> String {
    Response::ok(tool).result(value).build()
}

/// Convert a silently-saturating IEEE ±∞ or NaN into a structured OVERFLOW
/// error. Input elements themselves may be ±∞ (already flagged at parse time
/// would surprise callers), so we only check the reduced accumulator.
fn guard_overflow(tool: &str, result: f64) -> Option<String> {
    if result.is_finite() {
        return None;
    }
    let reason = if result.is_nan() {
        "result is NaN — an intermediate computation is undefined"
    } else {
        "result exceeds the f64 finite range"
    };
    Some(error_with_detail(
        tool,
        ErrorCode::Overflow,
        reason,
        &format!("result={result:?}"),
    ))
}

/// Sum all elements of a numeric array.
#[must_use]
pub fn sum_array(numbers: &str) -> String {
    let array = match parse_array(TOOL_SUM_ARRAY, "numbers", numbers) {
        Ok(arr) => arr,
        Err(e) => return e,
    };
    if array.is_empty() {
        return error(
            TOOL_SUM_ARRAY,
            ErrorCode::InvalidInput,
            "input array must not be empty",
        );
    }

    let mut acc = f64x4::splat(0.0);
    let lanes = 4;
    let bound = array.len() - (array.len() % lanes);
    let mut idx = 0;
    while idx < bound {
        let chunk = f64x4::new([array[idx], array[idx + 1], array[idx + 2], array[idx + 3]]);
        acc += chunk;
        idx += lanes;
    }
    let mut result = acc.reduce_add();
    while idx < array.len() {
        result += array[idx];
        idx += 1;
    }
    if let Some(err) = guard_overflow(TOOL_SUM_ARRAY, result) {
        return err;
    }
    ok_result(TOOL_SUM_ARRAY, format_f64(result))
}

/// Dot product of two arrays of equal length.
#[must_use]
pub fn dot_product(first: &str, second: &str) -> String {
    let array_a = match parse_array(TOOL_DOT_PRODUCT, "first", first) {
        Ok(arr) => arr,
        Err(e) => return e,
    };
    let array_b = match parse_array(TOOL_DOT_PRODUCT, "second", second) {
        Ok(arr) => arr,
        Err(e) => return e,
    };
    if array_a.is_empty() || array_b.is_empty() {
        return error(
            TOOL_DOT_PRODUCT,
            ErrorCode::InvalidInput,
            "input array must not be empty",
        );
    }
    if array_a.len() != array_b.len() {
        return error_with_detail(
            TOOL_DOT_PRODUCT,
            ErrorCode::InvalidInput,
            "arrays must be the same length",
            &format!("length={}, expected={}", array_b.len(), array_a.len()),
        );
    }

    let mut acc = f64x4::splat(0.0);
    let lanes = 4;
    let bound = array_a.len() - (array_a.len() % lanes);
    let mut idx = 0;
    while idx < bound {
        let vector_a = f64x4::new([
            array_a[idx],
            array_a[idx + 1],
            array_a[idx + 2],
            array_a[idx + 3],
        ]);
        let vector_b = f64x4::new([
            array_b[idx],
            array_b[idx + 1],
            array_b[idx + 2],
            array_b[idx + 3],
        ]);
        acc += vector_a * vector_b;
        idx += lanes;
    }
    let mut result = acc.reduce_add();
    while idx < array_a.len() {
        result += array_a[idx] * array_b[idx];
        idx += 1;
    }
    if let Some(err) = guard_overflow(TOOL_DOT_PRODUCT, result) {
        return err;
    }
    ok_result(TOOL_DOT_PRODUCT, format_f64(result))
}

/// Multiply every element by a scalar, returning the CSV result.
#[must_use]
pub fn scale_array(numbers: &str, scalar: &str) -> String {
    let array = match parse_array(TOOL_SCALE_ARRAY, "numbers", numbers) {
        Ok(arr) => arr,
        Err(e) => return e,
    };
    if array.is_empty() {
        return error(
            TOOL_SCALE_ARRAY,
            ErrorCode::InvalidInput,
            "input array must not be empty",
        );
    }
    let trimmed_scalar = scalar.trim();
    let Ok(factor) = trimmed_scalar.parse::<f64>() else {
        return error_with_detail(
            TOOL_SCALE_ARRAY,
            ErrorCode::ParseError,
            "scalar is not a valid number",
            &format!("scalar={trimmed_scalar}"),
        );
    };

    let mut result = vec![0.0_f64; array.len()];
    let v_scalar = f64x4::splat(factor);
    let lanes = 4;
    let bound = array.len() - (array.len() % lanes);
    let mut idx = 0;
    while idx < bound {
        let vector_a = f64x4::new([array[idx], array[idx + 1], array[idx + 2], array[idx + 3]]);
        let scaled = (vector_a * v_scalar).to_array();
        result[idx] = scaled[0];
        result[idx + 1] = scaled[1];
        result[idx + 2] = scaled[2];
        result[idx + 3] = scaled[3];
        idx += lanes;
    }
    while idx < array.len() {
        result[idx] = array[idx] * factor;
        idx += 1;
    }

    if let Some(bad) = result.iter().copied().find(|v| !v.is_finite()) {
        return error_with_detail(
            TOOL_SCALE_ARRAY,
            ErrorCode::Overflow,
            "scaling produced a non-finite element (overflow/underflow to ±∞ or NaN)",
            &format!("element={bad:?}, scalar={factor}"),
        );
    }

    let csv = result
        .iter()
        .map(|val| format_f64(*val))
        .collect::<Vec<_>>()
        .join(",");
    ok_result(TOOL_SCALE_ARRAY, csv)
}

/// Euclidean norm (magnitude) of a vector: `sqrt(sum(x²))`.
///
/// Uses a scale-protected two-pass algorithm to avoid intermediate
/// overflow/underflow: the naive `sum(x²)` overflows to `+∞` as soon as any
/// `|x_i| > ~1.3e154` (because `|x_i|² > f64::MAX`) and underflows to `0`
/// when every `|x_i| < ~1.5e-154`. Both produce catastrophically wrong
/// answers for vectors whose actual magnitude is perfectly representable
/// in `f64` (`‖(3e200, 4e200)‖ = 5e200`, well within range).
///
/// The scaled form `M · √Σ(x_i/M)²` — with `M = max|x_i|` — keeps every
/// squared term inside `[0, 1]`, matching the approach used by `distance2D`,
/// `distance3D`, and `complexMagnitude`. SIMD is preserved inside the
/// scaled sum so the happy path stays vectorized.
#[must_use]
pub fn magnitude_array(numbers: &str) -> String {
    let array = match parse_array(TOOL_MAGNITUDE_ARRAY, "numbers", numbers) {
        Ok(arr) => arr,
        Err(e) => return e,
    };
    if array.is_empty() {
        return error(
            TOOL_MAGNITUDE_ARRAY,
            ErrorCode::InvalidInput,
            "input array must not be empty",
        );
    }

    let max_abs = array.iter().fold(0.0_f64, |acc, v| acc.max(v.abs()));
    if max_abs == 0.0 {
        return ok_result(TOOL_MAGNITUDE_ARRAY, format_f64(0.0));
    }
    if !max_abs.is_finite() {
        // A single non-finite component already forces an infinite magnitude
        // — short-circuit so the scaled-sum path is never fed a NaN ratio.
        return ok_result(TOOL_MAGNITUDE_ARRAY, format_f64(f64::INFINITY));
    }

    let inv_max = 1.0 / max_abs;
    let scale_lanes = f64x4::splat(inv_max);
    let mut acc = f64x4::splat(0.0);
    let lanes = 4;
    let bound = array.len() - (array.len() % lanes);
    let mut idx = 0;
    while idx < bound {
        let vector_a =
            f64x4::new([array[idx], array[idx + 1], array[idx + 2], array[idx + 3]]) * scale_lanes;
        acc += vector_a * vector_a;
        idx += lanes;
    }
    let mut sum_of_squares = acc.reduce_add();
    while idx < array.len() {
        let scaled = array[idx] * inv_max;
        sum_of_squares += scaled * scaled;
        idx += 1;
    }
    let magnitude = max_abs * sum_of_squares.sqrt();
    if let Some(err) = guard_overflow(TOOL_MAGNITUDE_ARRAY, magnitude) {
        return err;
    }
    ok_result(TOOL_MAGNITUDE_ARRAY, format_f64(magnitude))
}

// --------------------------------------------------------------------------- //
//  Tests
// --------------------------------------------------------------------------- //

#[cfg(test)]
mod tests {
    use super::*;

    // ---- sum_array ----

    #[test]
    fn sum_array_basic() {
        assert_eq!(
            sum_array("1,2,3,4,5,6,7,8,9,10"),
            "SUM_ARRAY: OK | RESULT: 55.0"
        );
    }

    #[test]
    fn sum_array_with_fractions() {
        assert_eq!(sum_array("1.5,2.5,3.0"), "SUM_ARRAY: OK | RESULT: 7.0");
    }

    #[test]
    fn sum_array_tail_only() {
        assert_eq!(sum_array("10,20,30"), "SUM_ARRAY: OK | RESULT: 60.0");
    }

    #[test]
    fn sum_array_invalid_input() {
        assert_eq!(
            sum_array("1,foo,3"),
            "SUM_ARRAY: ERROR\nREASON: [PARSE_ERROR] array element is not a valid number\nDETAIL: numbers=foo"
        );
    }

    #[test]
    fn sum_array_empty_string_is_invalid_input() {
        // Blank input is structurally empty, not a parse failure — report the
        // same `INVALID_INPUT` code that `mean`/`median` use.
        assert_eq!(
            sum_array(""),
            "SUM_ARRAY: ERROR\nREASON: [INVALID_INPUT] input array must not be empty"
        );
        assert_eq!(
            sum_array("   "),
            "SUM_ARRAY: ERROR\nREASON: [INVALID_INPUT] input array must not be empty"
        );
    }

    // ---- dot_product ----

    #[test]
    fn dot_product_known_identity() {
        assert_eq!(
            dot_product("1,2,3", "4,5,6"),
            "DOT_PRODUCT: OK | RESULT: 32.0"
        );
    }

    #[test]
    fn dot_product_longer_arrays() {
        assert_eq!(
            dot_product("1,2,3,4,5,6,7,8", "1,2,3,4,5,6,7,8"),
            "DOT_PRODUCT: OK | RESULT: 204.0"
        );
    }

    #[test]
    fn dot_product_mismatched_lengths() {
        assert_eq!(
            dot_product("1,2,3", "4,5"),
            "DOT_PRODUCT: ERROR\nREASON: [INVALID_INPUT] arrays must be the same length\nDETAIL: length=2, expected=3"
        );
    }

    #[test]
    fn dot_product_parse_error() {
        assert_eq!(
            dot_product("1,nope", "3,4"),
            "DOT_PRODUCT: ERROR\nREASON: [PARSE_ERROR] array element is not a valid number\nDETAIL: first=nope"
        );
    }

    // ---- scale_array ----

    #[test]
    fn scale_array_basic() {
        assert_eq!(
            scale_array("1,2,3,4,5", "2"),
            "SCALE_ARRAY: OK | RESULT: 2.0,4.0,6.0,8.0,10.0"
        );
    }

    #[test]
    fn scale_array_with_negative_scalar() {
        assert_eq!(
            scale_array("1.5,-2.5", "-2"),
            "SCALE_ARRAY: OK | RESULT: -3.0,5.0"
        );
    }

    #[test]
    fn scale_array_invalid_scalar() {
        assert_eq!(
            scale_array("1,2,3", "abc"),
            "SCALE_ARRAY: ERROR\nREASON: [PARSE_ERROR] scalar is not a valid number\nDETAIL: scalar=abc"
        );
    }

    // ---- magnitude_array ----

    #[test]
    fn magnitude_array_pythagoras() {
        assert_eq!(magnitude_array("3,4"), "MAGNITUDE_ARRAY: OK | RESULT: 5.0");
    }

    #[test]
    fn magnitude_array_3d() {
        assert_eq!(
            magnitude_array("2,3,6"),
            "MAGNITUDE_ARRAY: OK | RESULT: 7.0"
        );
    }

    #[test]
    fn magnitude_array_many_elements() {
        let expected = format!("MAGNITUDE_ARRAY: OK | RESULT: {:?}", 8.0_f64.sqrt());
        assert_eq!(magnitude_array("1,1,1,1,1,1,1,1"), expected);
    }

    #[test]
    fn magnitude_array_empty() {
        // Blank input is structurally empty, not a parse failure.
        assert_eq!(
            magnitude_array(""),
            "MAGNITUDE_ARRAY: ERROR\nREASON: [INVALID_INPUT] input array must not be empty"
        );
    }

    #[test]
    fn magnitude_array_large_components_no_overflow() {
        // Regression: the naive `sum(x²)` formula returns `+∞` for any
        // component `> ~1.3e154` because `x²` exceeds `f64::MAX`. The
        // actual magnitude `‖(3e200, 4e200)‖ = 5e200` is perfectly
        // representable — scale protection recovers it.
        let out = magnitude_array("3e200,4e200");
        assert!(out.contains("5e200"), "got {out}");
        assert!(!out.contains("inf"), "got {out}");

        // ‖(1e300, 1e300)‖ = √2 · 1e300 ≈ 1.4142…e300. Prefix-match to 14
        // digits so last-ULP drift between f64 representations of √2 (the
        // SIMD reduction can land on `…951` or `…952` depending on order)
        // doesn't make the test brittle.
        let out2 = magnitude_array("1e300,1e300");
        assert!(out2.contains("1.41421356237309"), "got {out2}");
        assert!(out2.ends_with("e300"), "got {out2}");
    }

    #[test]
    fn magnitude_array_tiny_components_no_underflow() {
        // Regression: `(1e-200)² = 1e-400` flushes to denormal zero, so the
        // naive sum collapses to `0` even though the magnitude
        // `√2 · 1e-200` is representable. Scale protection restores it.
        let out = magnitude_array("1e-200,1e-200");
        assert!(!out.contains("RESULT: 0\n"), "got {out}");
        assert!(out.contains("1.41421356237309"), "got {out}");
        assert!(out.ends_with("e-200"), "got {out}");
    }

    #[test]
    fn magnitude_array_pythagorean_scaled() {
        // `‖(5e150, 12e150)‖ = 1.3e151` — the scaled algorithm gives
        // `1.2999…e151` after SIMD reduction, one ULP below exact 1.3e151.
        // Parse the numeric part and compare by relative tolerance so a
        // future change in reduction order doesn't fail this test.
        let out = magnitude_array("5e150,12e150");
        let value: f64 = out
            .strip_prefix("MAGNITUDE_ARRAY: OK | RESULT: ")
            .and_then(|rest| rest.split(char::is_whitespace).next())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or_else(|| panic!("could not parse value from {out}"));
        let expected = 1.3e151_f64;
        let relative = (value - expected).abs() / expected;
        assert!(relative < 1e-14, "got {value}, expected {expected}");
    }
}
