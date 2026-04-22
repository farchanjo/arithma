//! Helpers for interoperating with `bigdecimal::BigDecimal` as a drop-in for
//! `java.math.BigDecimal` + `MathContext.DECIMAL128`.
//!
//! Java `toPlainString()` vs `toString()`: the `bigdecimal` crate's `Display` already
//! produces plain notation without scientific suffixes, matching `toPlainString()`.

use std::sync::LazyLock;

use bigdecimal::BigDecimal;

/// DECIMAL128 precision — 34 significant digits — matches `MathContext.DECIMAL128`.
pub const DECIMAL128_PRECISION: u64 = 34;

/// Scale used for division across financial/electronics tools (20 decimal places, HALF_UP).
pub const DIVISION_SCALE: i64 = 20;

/// 2π with 40 fractional digits — sufficient for DECIMAL128 multiplications.
pub static TWO_PI: LazyLock<BigDecimal> = LazyLock::new(|| {
    "6.2831853071795864769252867665590057683943"
        .parse()
        .expect("valid TWO_PI literal")
});

/// π with 40 fractional digits.
pub static PI: LazyLock<BigDecimal> = LazyLock::new(|| {
    "3.1415926535897932384626433832795028841972"
        .parse()
        .expect("valid PI literal")
});

/// 1/ln(2) — used by 555-timer astable computations.
pub static LN2_RECIPROCAL: LazyLock<BigDecimal> = LazyLock::new(|| {
    "1.44269504088896340735992468100189213742665"
        .parse()
        .expect("valid 1/ln(2) literal")
});

/// Strip trailing zeros and render as plain (non-scientific) decimal.
///
/// Matches Java `value.stripTrailingZeros().toPlainString()` except that a pure
/// integer result is printed without a trailing `.0`.
pub fn strip_plain(value: &BigDecimal) -> String {
    value.normalized().to_plain_string()
}

/// True iff the value is exactly zero.
pub fn is_zero(value: &BigDecimal) -> bool {
    use num_traits::Zero;
    value.is_zero()
}
