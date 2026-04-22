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
fn parse_array(tool: &str, label: &str, input: &str) -> Result<Vec<f64>, String> {
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

/// Sum all elements of a numeric array.
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
    ok_result(TOOL_SUM_ARRAY, format_f64(result))
}

/// Dot product of two arrays of equal length.
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
    ok_result(TOOL_DOT_PRODUCT, format_f64(result))
}

/// Multiply every element by a scalar, returning the CSV result.
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
    let factor = match trimmed_scalar.parse::<f64>() {
        Ok(value) => value,
        Err(_) => {
            return error_with_detail(
                TOOL_SCALE_ARRAY,
                ErrorCode::ParseError,
                "scalar is not a valid number",
                &format!("scalar={trimmed_scalar}"),
            );
        }
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

    let csv = result
        .iter()
        .map(|val| format_f64(*val))
        .collect::<Vec<_>>()
        .join(",");
    ok_result(TOOL_SCALE_ARRAY, csv)
}

/// Euclidean norm (magnitude) of a vector: `sqrt(sum(x²))`.
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

    let mut acc = f64x4::splat(0.0);
    let lanes = 4;
    let bound = array.len() - (array.len() % lanes);
    let mut idx = 0;
    while idx < bound {
        let vector_a = f64x4::new([array[idx], array[idx + 1], array[idx + 2], array[idx + 3]]);
        acc += vector_a * vector_a;
        idx += lanes;
    }
    let mut sum_of_squares = acc.reduce_add();
    while idx < array.len() {
        sum_of_squares += array[idx] * array[idx];
        idx += 1;
    }
    ok_result(TOOL_MAGNITUDE_ARRAY, format_f64(sum_of_squares.sqrt()))
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
    fn sum_array_empty_string_is_parse_error() {
        // "" splits into [""] which fails to parse as f64.
        assert_eq!(
            sum_array(""),
            "SUM_ARRAY: ERROR\nREASON: [PARSE_ERROR] array element is not a valid number\nDETAIL: numbers="
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
        assert_eq!(
            magnitude_array("3,4"),
            "MAGNITUDE_ARRAY: OK | RESULT: 5.0"
        );
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
        // "" splits into [""] → parse error, not empty-array error.
        assert_eq!(
            magnitude_array(""),
            "MAGNITUDE_ARRAY: ERROR\nREASON: [PARSE_ERROR] array element is not a valid number\nDETAIL: numbers="
        );
    }
}
