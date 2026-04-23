//! Complex number arithmetic.
//!
//! Rectangular form `a+bi` is parsed from `"real,imag"` CSV pairs. Polar
//! conversions accept/return `"magnitude,angle"` with angle in **degrees**
//! (matches the trig conventions elsewhere in arithma).

use std::f64::consts::PI;

use crate::mcp::message::{ErrorCode, Response, error, error_with_detail};

const TOOL_COMPLEX_ADD: &str = "COMPLEX_ADD";
const TOOL_COMPLEX_MULT: &str = "COMPLEX_MULT";
const TOOL_COMPLEX_DIV: &str = "COMPLEX_DIV";
const TOOL_COMPLEX_CONJUGATE: &str = "COMPLEX_CONJUGATE";
const TOOL_COMPLEX_POWER: &str = "COMPLEX_POWER";
const TOOL_COMPLEX_MAGNITUDE: &str = "COMPLEX_MAGNITUDE";
const TOOL_COMPLEX_PHASE: &str = "COMPLEX_PHASE";
const TOOL_POLAR_TO_RECT: &str = "POLAR_TO_RECT";
const TOOL_RECT_TO_POLAR: &str = "RECT_TO_POLAR";
const TOOL_COMPLEX_SQRT: &str = "COMPLEX_SQRT";

const DEG_TO_RAD: f64 = PI / 180.0;
const RAD_TO_DEG: f64 = 180.0 / PI;

#[derive(Copy, Clone, Debug)]
struct C {
    re: f64,
    im: f64,
}

impl C {
    const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    fn add(self, other: Self) -> Self {
        Self::new(self.re + other.re, self.im + other.im)
    }

    fn mul(self, other: Self) -> Self {
        Self::new(
            self.re.mul_add(other.re, -(self.im * other.im)),
            self.re.mul_add(other.im, self.im * other.re),
        )
    }

    fn conj(self) -> Self {
        Self::new(self.re, -self.im)
    }

    fn magnitude(self) -> f64 {
        self.re.hypot(self.im)
    }

    fn phase_deg(self) -> f64 {
        // `atan2` returns `(-π, π]` in radians, which becomes `(-180°, 180°]`
        // after scaling — almost. For `(-x, -0.0)` (imag = signed-negative
        // zero) `atan2` bottoms out at `-π`, so the degree form lands on
        // `-180.0`, which is *outside* the documented `(-180, 180]` interval.
        // Map the lower boundary back to `+180` to match the docstring. The
        // exact equality test with `-180.0` is deliberate: atan2 of signed
        // zero produces that bitwise value, never a near-match like `-179.9…`.
        let raw = self.im.atan2(self.re) * RAD_TO_DEG;
        #[allow(clippy::float_cmp)]
        {
            if raw == -180.0 { 180.0 } else { raw }
        }
    }

    fn div(self, other: Self) -> Option<Self> {
        let denom = other.re.mul_add(other.re, other.im * other.im);
        if denom == 0.0 {
            return None;
        }
        Some(Self::new(
            self.re.mul_add(other.re, self.im * other.im) / denom,
            self.im.mul_add(other.re, -(self.re * other.im)) / denom,
        ))
    }
}

fn parse_complex(tool: &str, label: &str, input: &str) -> Result<C, String> {
    let parts: Vec<&str> = input.split(',').collect();
    if parts.len() != 2 {
        return Err(error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            "complex number requires exactly two comma-separated values (real,imag)",
            &format!("{label}={input}"),
        ));
    }
    let re = parts[0].trim().parse::<f64>().map_err(|_| {
        error_with_detail(
            tool,
            ErrorCode::ParseError,
            "real component is not a valid number",
            &format!("{label}={input}"),
        )
    })?;
    let im = parts[1].trim().parse::<f64>().map_err(|_| {
        error_with_detail(
            tool,
            ErrorCode::ParseError,
            "imaginary component is not a valid number",
            &format!("{label}={input}"),
        )
    })?;
    if !re.is_finite() || !im.is_finite() {
        return Err(error_with_detail(
            tool,
            ErrorCode::InvalidInput,
            "complex components must be finite",
            &format!("{label}={input}"),
        ));
    }
    Ok(C::new(re, im))
}

fn parse_f64(tool: &str, label: &str, value: &str) -> Result<f64, String> {
    value.trim().parse::<f64>().map_err(|_| {
        error_with_detail(
            tool,
            ErrorCode::ParseError,
            "value is not a valid number",
            &format!("{label}={value}"),
        )
    })
}

fn fmt(value: f64) -> String {
    format!("{value:?}")
}

/// Collapse numerically-dead residue from trig round-trips (e.g.
/// `sqrt(-1)` has real ≈ 6.1e-17 from `cos(π/2)`). The threshold scales with
/// the companion component so honest small values — like the 1e-17 imag of a
/// near-axis rotation — aren't mistakenly zeroed.
fn snap_to_zero(primary: f64, companion: f64) -> f64 {
    const ABS_EPS: f64 = 1e-12;
    const REL_EPS: f64 = 1e-12;
    if primary.abs() < ABS_EPS && primary.abs() < REL_EPS * companion.abs() {
        0.0
    } else {
        primary
    }
}

/// Snap a value to the nearest integer when the distance is within
/// `1e-12 · max(|v|, 1)`. De Moivre power tricks leave `(1+i)^8 = 16` as
/// `16.000000000000007`; this one-ulp cleanup brings the result back to
/// the textbook integer without affecting genuine non-integers (which
/// diverge from their nearest integer by far more than the threshold).
fn snap_near_integer(value: f64) -> f64 {
    if !value.is_finite() {
        return value;
    }
    let rounded = value.round();
    let delta = (value - rounded).abs();
    let scale = value.abs().max(1.0);
    if delta <= 1e-12 * scale {
        rounded
    } else {
        value
    }
}

fn ok_complex(tool: &str, c: C) -> String {
    // Two-stage cleanup for numerical residue:
    // 1. Snap near-zero components (trig round-trips).
    // 2. Snap the remaining value to the nearest integer when within one
    //    relative ulp — covers `(1+i)^8 = 16` / `e^(iπ) = -1` etc.
    let re = snap_near_integer(snap_to_zero(c.re, c.im));
    let im = snap_near_integer(snap_to_zero(c.im, c.re));
    Response::ok(tool)
        .field("REAL", fmt(re))
        .field("IMAG", fmt(im))
        .build()
}

#[must_use]
pub fn complex_add(a: &str, b: &str) -> String {
    let z1 = match parse_complex(TOOL_COMPLEX_ADD, "a", a) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let z2 = match parse_complex(TOOL_COMPLEX_ADD, "b", b) {
        Ok(v) => v,
        Err(e) => return e,
    };
    ok_complex(TOOL_COMPLEX_ADD, z1.add(z2))
}

#[must_use]
pub fn complex_mult(a: &str, b: &str) -> String {
    let z1 = match parse_complex(TOOL_COMPLEX_MULT, "a", a) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let z2 = match parse_complex(TOOL_COMPLEX_MULT, "b", b) {
        Ok(v) => v,
        Err(e) => return e,
    };
    ok_complex(TOOL_COMPLEX_MULT, z1.mul(z2))
}

#[must_use]
pub fn complex_div(a: &str, b: &str) -> String {
    let z1 = match parse_complex(TOOL_COMPLEX_DIV, "a", a) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let z2 = match parse_complex(TOOL_COMPLEX_DIV, "b", b) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let Some(q) = z1.div(z2) else {
        return error(
            TOOL_COMPLEX_DIV,
            ErrorCode::DivisionByZero,
            "cannot divide by complex zero",
        );
    };
    ok_complex(TOOL_COMPLEX_DIV, q)
}

#[must_use]
pub fn complex_conjugate(z: &str) -> String {
    let zv = match parse_complex(TOOL_COMPLEX_CONJUGATE, "z", z) {
        Ok(v) => v,
        Err(e) => return e,
    };
    ok_complex(TOOL_COMPLEX_CONJUGATE, zv.conj())
}

/// `z^n` for real `n` via De Moivre: `r^n * (cos(nθ) + i sin(nθ))`.
#[must_use]
pub fn complex_power(z: &str, exponent: &str) -> String {
    let zv = match parse_complex(TOOL_COMPLEX_POWER, "z", z) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let n = match parse_f64(TOOL_COMPLEX_POWER, "exponent", exponent) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let r = zv.magnitude();
    if r == 0.0 {
        if n <= 0.0 {
            return error(
                TOOL_COMPLEX_POWER,
                ErrorCode::DomainError,
                "0^n is undefined for n <= 0",
            );
        }
        return ok_complex(TOOL_COMPLEX_POWER, C::new(0.0, 0.0));
    }
    let theta = zv.im.atan2(zv.re);
    let new_r = r.powf(n);
    let new_t = theta * n;
    ok_complex(
        TOOL_COMPLEX_POWER,
        C::new(new_r * new_t.cos(), new_r * new_t.sin()),
    )
}

#[must_use]
pub fn complex_magnitude(z: &str) -> String {
    let zv = match parse_complex(TOOL_COMPLEX_MAGNITUDE, "z", z) {
        Ok(v) => v,
        Err(e) => return e,
    };
    Response::ok(TOOL_COMPLEX_MAGNITUDE)
        .result(fmt(zv.magnitude()))
        .build()
}

/// Phase angle in **degrees**, range (-180, 180].
#[must_use]
pub fn complex_phase(z: &str) -> String {
    let zv = match parse_complex(TOOL_COMPLEX_PHASE, "z", z) {
        Ok(v) => v,
        Err(e) => return e,
    };
    // The phase of 0+0i is mathematically undefined. Python's `cmath.phase(0)`,
    // numpy, and this crate's own `rect_to_polar(0,0)` all return 0 by
    // convention, so match that to keep the complex-tool surface consistent.
    let theta = if zv.re == 0.0 && zv.im == 0.0 {
        0.0
    } else {
        zv.phase_deg()
    };
    Response::ok(TOOL_COMPLEX_PHASE).result(fmt(theta)).build()
}

/// Polar `(magnitude, angleDegrees)` → rectangular `(real, imag)`.
#[must_use]
pub fn polar_to_rect(magnitude: &str, angle_degrees: &str) -> String {
    let r = match parse_f64(TOOL_POLAR_TO_RECT, "magnitude", magnitude) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let theta_deg = match parse_f64(TOOL_POLAR_TO_RECT, "angleDegrees", angle_degrees) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if r < 0.0 {
        return error_with_detail(
            TOOL_POLAR_TO_RECT,
            ErrorCode::DomainError,
            "magnitude must be non-negative",
            &format!("magnitude={r}"),
        );
    }
    let rad = theta_deg * DEG_TO_RAD;
    ok_complex(TOOL_POLAR_TO_RECT, C::new(r * rad.cos(), r * rad.sin()))
}

/// Rectangular `(real, imag)` → polar `(magnitude, angleDegrees)`.
#[must_use]
pub fn rect_to_polar(z: &str) -> String {
    let zv = match parse_complex(TOOL_RECT_TO_POLAR, "z", z) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let r = zv.magnitude();
    let theta = if zv.re == 0.0 && zv.im == 0.0 {
        0.0
    } else {
        zv.phase_deg()
    };
    Response::ok(TOOL_RECT_TO_POLAR)
        .field("MAGNITUDE", fmt(r))
        .field("ANGLE_DEG", fmt(theta))
        .build()
}

/// Principal square root of a complex number — returns one of two roots
/// (negate both real and imaginary parts to get the other).
#[must_use]
pub fn complex_sqrt(z: &str) -> String {
    let zv = match parse_complex(TOOL_COMPLEX_SQRT, "z", z) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let r = zv.magnitude();
    let sqrt_r = r.sqrt();
    // Principal: angle / 2
    let theta = zv.im.atan2(zv.re) / 2.0;
    ok_complex(
        TOOL_COMPLEX_SQRT,
        C::new(sqrt_r * theta.cos(), sqrt_r * theta.sin()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_field(out: &str, key: &str, expected: f64) {
        let primary = format!(" | {key}: ");
        let header = format!(": OK | {key}: ");
        let part = out
            .split(&primary)
            .nth(1)
            .or_else(|| out.split(&header).nth(1))
            .unwrap_or_else(|| panic!("field {key} not found in `{out}`"));
        let value_str: String = part
            .chars()
            .take_while(|c| *c != ' ' && *c != '\n')
            .collect();
        let v: f64 = value_str.parse().expect("parse");
        assert!(
            (v - expected).abs() < 1e-6,
            "{key}: expected ~{expected}, got {v} in `{out}`"
        );
    }

    #[test]
    fn add_basic() {
        // (1+2i) + (3+4i) = 4+6i
        let out = complex_add("1,2", "3,4");
        approx_field(&out, "REAL", 4.0);
        approx_field(&out, "IMAG", 6.0);
    }

    #[test]
    fn mult_basic() {
        // (1+2i) * (3+4i) = (3-8) + (4+6)i = -5+10i
        let out = complex_mult("1,2", "3,4");
        approx_field(&out, "REAL", -5.0);
        approx_field(&out, "IMAG", 10.0);
    }

    #[test]
    fn div_basic() {
        // (1+2i) / (3+4i) = (3+8 + (6-4)i) / 25 = 11/25 + 2/25 i = 0.44 + 0.08 i
        let out = complex_div("1,2", "3,4");
        approx_field(&out, "REAL", 0.44);
        approx_field(&out, "IMAG", 0.08);
    }

    #[test]
    fn div_by_zero_errors() {
        let out = complex_div("1,2", "0,0");
        assert!(out.starts_with("COMPLEX_DIV: ERROR\nREASON: [DIVISION_BY_ZERO]"));
    }

    #[test]
    fn conjugate_flips_imag_sign() {
        let out = complex_conjugate("3,5");
        approx_field(&out, "REAL", 3.0);
        approx_field(&out, "IMAG", -5.0);
    }

    #[test]
    fn power_squared() {
        // (1+i)^2 = 2i
        let out = complex_power("1,1", "2");
        approx_field(&out, "REAL", 0.0);
        approx_field(&out, "IMAG", 2.0);
    }

    #[test]
    fn power_zero_to_zero_errors() {
        let out = complex_power("0,0", "0");
        assert!(out.starts_with("COMPLEX_POWER: ERROR"));
    }

    #[test]
    fn magnitude_3_4_is_5() {
        let out = complex_magnitude("3,4");
        assert!(out.contains("RESULT: 5.0"), "got {out}");
    }

    #[test]
    fn phase_pure_imaginary_is_90() {
        let out = complex_phase("0,1");
        assert!(out.contains("RESULT: 90.0"), "got {out}");
    }

    #[test]
    fn phase_negative_real_is_180() {
        let out = complex_phase("-1,0");
        assert!(out.contains("RESULT: 180.0"), "got {out}");
    }

    #[test]
    fn phase_negative_real_epsilon_imag_stays_in_range() {
        // Documented range is `(-180, 180]` — `atan2` underflows to -π when
        // imag is a signed-negative zero, landing on -180° which is outside
        // the interval. The wrap correction must push that back to +180°.
        let out = complex_phase("-1,-0.0000000000000001");
        assert!(out.contains("RESULT: 180.0"), "got {out}");
    }

    #[test]
    fn phase_of_zero_is_zero_by_convention() {
        // Consistent with rect_to_polar(0,0) and Python cmath.phase(0j).
        let out = complex_phase("0,0");
        assert!(out.contains("RESULT: 0.0"), "got {out}");
    }

    #[test]
    fn complex_power_snaps_to_integer() {
        // (1+i)^8 = 16 exact mathematically; De Moivre via cos/sin leaks
        // 7e-15 residue in the real part and a matching 7e-15 in imag.
        // The snap cleanup brings both to their textbook values.
        let out = complex_power("1,1", "8");
        assert!(out.contains("REAL: 16.0"), "got {out}");
        assert!(out.contains("IMAG: 0.0"), "got {out}");
    }

    #[test]
    fn polar_to_rect_53deg_gives_clean_345() {
        // 5·(cos 53.13°, sin 53.13°) → (3, 4) — the 3-4-5 right triangle.
        // Without the snap, IMAG printed as 3.9999999999999996.
        let out = polar_to_rect("5", "53.13010235415598");
        assert!(out.contains("REAL: 3.0"), "got {out}");
        assert!(out.contains("IMAG: 4.0"), "got {out}");
    }

    #[test]
    fn polar_to_rect_basic() {
        // r=2, θ=90° → 0+2i
        let out = polar_to_rect("2", "90");
        approx_field(&out, "REAL", 0.0);
        approx_field(&out, "IMAG", 2.0);
    }

    #[test]
    fn rect_to_polar_basic() {
        // 0+2i → r=2, θ=90°
        let out = rect_to_polar("0,2");
        approx_field(&out, "MAGNITUDE", 2.0);
        approx_field(&out, "ANGLE_DEG", 90.0);
    }

    #[test]
    fn sqrt_of_minus_one_is_i() {
        let out = complex_sqrt("-1,0");
        approx_field(&out, "REAL", 0.0);
        approx_field(&out, "IMAG", 1.0);
    }

    #[test]
    fn sqrt_of_real_positive() {
        // sqrt(4+0i) = 2+0i
        let out = complex_sqrt("4,0");
        approx_field(&out, "REAL", 2.0);
        approx_field(&out, "IMAG", 0.0);
    }

    #[test]
    fn parse_error_propagates() {
        let out = complex_add("foo,1", "0,0");
        assert!(out.starts_with("COMPLEX_ADD: ERROR\nREASON: [PARSE_ERROR]"));
    }
}
