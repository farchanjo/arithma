//! Shared f64 numeric helpers.
//!
//! Small utilities reused across tools that produce `f64` results and would
//! otherwise ship last-digit truncation noise (`1.0000000000000002`, `6.0 +
//! 1e-12`, …) in responses.

use crate::mcp::message::{ErrorCode, error_with_detail};

/// Refuse to format IEEE ±∞ / NaN as a successful result.
///
/// Many tools build their response envelope with `fmt(value)` straight from
/// an arithmetic chain (e.g. `PI * r * r * h`); once `r` is large enough the
/// result saturates to `+∞` and the original envelope would silently ship
/// `RESULT: inf`. Funnel every such path through this helper so callers see
/// a structured `OVERFLOW` instead. Returns a pre-rendered error envelope on
/// failure so tools can simply `?`-propagate into their existing
/// `Result<_, String>` flows.
///
/// # Errors
/// Returns an `OVERFLOW` error envelope when `value` is `±∞` or `NaN`.
#[must_use = "overflow guard must be propagated, not discarded"]
pub fn guard_finite(tool: &str, label: &str, value: f64) -> Result<f64, String> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(error_with_detail(
            tool,
            ErrorCode::Overflow,
            "result is non-finite (overflow/underflow to ±∞ or NaN)",
            &format!("{label}={value:?}"),
        ))
    }
}

/// Collapse noise to the nearest integer when the delta is within ~1e-9 of the
/// value's magnitude.
///
/// Numerical differentiation, trig with PI approximations, and polygon geometry
/// consistently leak a few ULPs around clean integer answers — `d/dx x²` at 3
/// prints `6.000000001282757`, and a regular hexagon's circumradius prints
/// `1.0000000000000002`. Anything farther than the threshold is genuine signal
/// and passes through untouched.
///
/// The scale term `value.abs().max(1.0)` is deliberately bounded below by 1 so
/// a truly small result (like a `1e-10` determinant) is not mistakenly
/// collapsed to zero — the threshold is absolute `1e-9` in that regime.
#[must_use]
pub fn snap_near_integer(value: f64) -> f64 {
    if !value.is_finite() {
        return value;
    }
    let rounded = value.round();
    // Skip when the target integer is `±0` — any value with magnitude below
    // `0.5` rounds to zero, and silently collapsing a legitimate `2.88e-11`
    // (e.g. the apothem of `regularPolygon(3, 1e-10)` = `1e-10 / (2·tan(60°))`
    // ≈ `2.88e-11`) to `0` destroys magnitude information. Residues near
    // zero from differentiation (`central_difference` drift) are already
    // bounded by the scale-relative check below, which leaves `|v| > 1e-9`
    // alone.
    if (rounded.to_bits() & !(1u64 << 63)) == 0 {
        return value;
    }
    let delta = (value - rounded).abs();
    let scale = value.abs().max(1.0);
    if delta <= 1e-9 * scale {
        rounded
    } else {
        value
    }
}

/// Collapse `-0.0` to `+0.0`, leaving every other value untouched.
///
/// IEEE-754 preserves the sign bit through negation and `log10(1) = 0`, so
/// `-log10(1) = -0.0`. That's mathematically fine but shows up in formatted
/// output as a user-visible `"-0.0"` where callers rightly expect `"0.0"`.
/// Adding `+0.0` produces the canonical `+0.0` (per IEEE-754 rule
/// `-0 + 0 = +0`) without perturbing any non-zero value or disturbing
/// NaN/∞ semantics. Cheaper than a branch and free of `float_cmp`.
#[must_use]
pub fn canonicalize_zero(value: f64) -> f64 {
    value + 0.0
}

/// Round a value to the given number of significant decimal digits.
///
/// Gauss-Jordan elimination and similar f64 cascades accumulate a few ULPs of
/// noise that expand into long tails like `1.4999999999999998` or
/// `-0.49999999999999994`. Rounding at 15 significant digits absorbs that
/// noise while still preserving every bit of genuine f64 information — the
/// type only guarantees 15–17 significant digits.
///
/// Zero and non-finite inputs pass through unchanged. Rounding goes through
/// Rust's decimal exponent formatter (`{:.*e}`) and its correctly-rounded
/// float parser, so the snap is bit-identical on every platform and never
/// overflows. The earlier formulation rescaled with `10f64::powf`, which
/// calls the C library's `pow`: on Windows (msvcrt, ~1 ULP error) the power
/// of ten itself drifted, so canonical extreme results like `1e-200` were
/// re-rounded onto the drifted scale factor (`9.999999999999995e-201`).
#[must_use]
pub fn snap_to_precision(value: f64, sig_digits: u32) -> f64 {
    if value == 0.0 || !value.is_finite() || sig_digits == 0 {
        return value;
    }
    // `{:.Xe}` prints `X + 1` significant digits in exponent form; parsing
    // that decimal back selects the nearest f64 (round-trip guarantee), so
    // every value inside a 15-significant-digit basin collapses onto its
    // canonical representative. Exponent form keeps extremes (2e-300) in
    // range — no intermediate `10^314` overflow.
    let precision = (sig_digits - 1) as usize;
    format!("{:.*e}", precision, value).parse().unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bit-pattern equality. `assert_eq!` on raw `f64` hits clippy's
    /// `float_cmp` lint — comparing bits is the canonical alternative for
    /// snap outputs, which are supposed to produce the canonical f64 value
    /// of their integer/rational target (so bitwise equality is both
    /// sufficient and strictest).
    fn assert_bits_eq(actual: f64, expected: f64) {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "actual={actual}, expected={expected}"
        );
    }

    #[test]
    fn snaps_hexagon_circumradius_residue() {
        assert_bits_eq(snap_near_integer(1.000_000_000_000_000_2), 1.0);
    }

    #[test]
    fn preserves_genuinely_non_integer_values() {
        assert_bits_eq(snap_near_integer(0.5), 0.5);
        assert_bits_eq(snap_near_integer(1.2345), 1.2345);
    }

    #[test]
    fn passes_through_nan_and_infinity() {
        assert!(snap_near_integer(f64::NAN).is_nan());
        assert!(snap_near_integer(f64::INFINITY).is_infinite());
    }

    #[test]
    fn snaps_negative_integer_residue() {
        assert_bits_eq(snap_near_integer(-9.000_000_003_848_271), -9.0);
    }

    #[test]
    fn preserves_tiny_values_near_zero_target() {
        // Regression: legitimate small-magnitude results like the apothem of
        // `regularPolygon(3, 1e-10)` (`≈ 2.88e-11`) must survive untouched.
        // The zero-rounded guard skips the snap whenever the nearest integer
        // is `±0`, preventing silent magnitude loss. Derivative drift near
        // a genuine zero result still gets handled downstream — callers
        // that need unconditional "tiny → zero" should use their own bound.
        assert_bits_eq(snap_near_integer(1e-12), 1e-12);
        assert_bits_eq(snap_near_integer(-1e-15), -1e-15);
        assert_bits_eq(snap_near_integer(2.88e-11), 2.88e-11);
    }

    #[test]
    fn still_snaps_near_nonzero_integer_from_derivative_drift() {
        // The original use case — collapsing `d/dx x²` at `x=3` (returns
        // `6.000000001282757` via central difference) back to `6` — still
        // works because `rounded = 6 ≠ 0`.
        assert_bits_eq(snap_near_integer(6.000_000_001_282_757), 6.0);
        assert_bits_eq(snap_near_integer(-1.999_999_999_999_999_8), -2.0);
    }

    #[test]
    fn precision_snap_removes_ulp_residue() {
        assert_bits_eq(snap_to_precision(1.499_999_999_999_999_8, 15), 1.5);
        assert_bits_eq(snap_to_precision(-1.999_999_999_999_999_6, 15), -2.0);
        assert_bits_eq(snap_to_precision(-0.499_999_999_999_999_94, 15), -0.5);
    }

    #[test]
    fn precision_snap_preserves_small_magnitudes() {
        // 1e-10 has magnitude -10, 15 sig digits keeps the value intact.
        assert_bits_eq(snap_to_precision(1e-10, 15), 1e-10);
        let tiny = 3.141_592_653_589_793e-7;
        assert!((snap_to_precision(tiny, 15) - tiny).abs() < 1e-20);
    }

    #[test]
    fn precision_snap_passes_through_edge_cases() {
        assert_bits_eq(snap_to_precision(0.0, 15), 0.0);
        assert!(snap_to_precision(f64::NAN, 15).is_nan());
        assert!(snap_to_precision(f64::INFINITY, 15).is_infinite());
        assert_bits_eq(snap_to_precision(1.5, 0), 1.5);
    }

    #[test]
    fn precision_snap_handles_extreme_magnitudes_without_nan() {
        // Regression: `2e-300` previously made the snap compute `10^314`,
        // which overflows to `+∞` and the subsequent `inf / inf` returned
        // `NaN`. That leaked into `matrixTrace(diag(1e-300, 1e-300))`
        // which reported `NaN` instead of `2e-300`. Normalizing the input
        // before rounding keeps every intermediate in f64 range.
        assert!(snap_to_precision(2e-300, 15).is_finite());
        assert_bits_eq(snap_to_precision(2e-300, 15), 2e-300);
        assert!(snap_to_precision(5e-250, 15).is_finite());
        assert!(snap_to_precision(3e250, 15).is_finite());
    }

    #[test]
    fn precision_snap_still_rounds_ulp_drift() {
        // Sanity: the original rounding behaviour for noisy mid-range
        // values must survive after the normalization change.
        assert_bits_eq(snap_to_precision(1.499_999_999_999_999_8, 15), 1.5);
        assert_bits_eq(snap_to_precision(-1.999_999_999_999_999_6, 15), -2.0);
    }

    #[test]
    fn canonicalize_zero_eliminates_negative_zero() {
        assert_bits_eq(canonicalize_zero(-0.0), 0.0);
        assert_bits_eq(canonicalize_zero(0.0), 0.0);
    }

    #[test]
    fn canonicalize_zero_preserves_other_values() {
        assert_bits_eq(canonicalize_zero(5.0), 5.0);
        assert_bits_eq(canonicalize_zero(-5.0), -5.0);
        assert_bits_eq(canonicalize_zero(1e-300), 1e-300);
        assert!(canonicalize_zero(f64::NAN).is_nan());
        assert!(canonicalize_zero(f64::INFINITY).is_infinite());
        assert!(canonicalize_zero(f64::NEG_INFINITY).is_infinite());
    }
}
