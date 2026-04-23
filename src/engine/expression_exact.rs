//! Exact-precision expression evaluator.
//!
//! Hybrid backend: every arithmetic operator (`+ - * / % ^ unary-minus`, `abs`,
//! `ceil`, `floor`) runs on [`BigDecimal`] directly, so simple inputs like
//! `0.1 + 0.2` return exactly `"0.3"`. Transcendentals (`sqrt`, `sin`, `cos`,
//! `tan`, `log`, `log10`, non-integer `^`) dip into 128-bit `astro-float` for
//! computation and round back to `BigDecimal` on return. Division scales to
//! 128 significant digits; `HALF_UP` rounding matches the Java reference.
//!
//! Grammar and error messages mirror [`crate::engine::expression`]; only the
//! numeric backend differs.

use std::collections::HashMap;
use std::hash::BuildHasher;
use std::str::FromStr;

use astro_float::{BigFloat, Consts, Radix, RoundingMode as AfRm};
use bigdecimal::{BigDecimal, RoundingMode as BdRm};
use num_traits::{Signed, ToPrimitive, Zero};

use crate::engine::bigdecimal_ext::strip_plain;
use crate::engine::expression::ExpressionError;

/// Number of decimal digits retained across transcendental round-trips and
/// division. Tuned to ~38 digits so the `BigDecimal` payload stays short
/// enough for LLM consumption while swallowing the binary-to-decimal noise
/// that astro-float leaks on the last few digits.
const EXACT_PRECISION: u64 = 38;
/// `astro-float` mantissa precision (in bits) used during transcendentals.
const AF_PRECISION: usize = 192;
/// Cap on the estimated printed length of an integer-exponent `pow` result.
/// Mirrors the guard in [`crate::tools::basic::power`] — expressions like
/// `2^1000000` would otherwise produce a 300k-character payload that exceeds
/// MCP token limits. The bound `len(base) * exp` is a safe upper estimate.
const MAX_POWER_RESULT_LEN: u64 = 10_000;
const DEG_TO_RAD_LITERAL: &str =
    "0.017453292519943295769236907684886127134428718885417254560971914401710091146034";
const RAD_TO_DEG_LITERAL: &str =
    "57.29577951308232087679815481410517033240547246656432154916024386120284714832156";

/// Built-in constants exposed to expressions. Strings to preserve full precision
/// when round-tripping through `BigDecimal`. All decimals truncated to ~38 digits
/// matching [`EXACT_PRECISION`].
const PI_LITERAL: &str = "3.1415926535897932384626433832795028842";
const E_LITERAL: &str = "2.7182818284590452353602874713526624978";
const TAU_LITERAL: &str = "6.2831853071795864769252867665590057684";
const PHI_LITERAL: &str = "1.6180339887498948482045868343656381177";

/// Resolve a bare identifier as an exact-precision built-in constant.
fn lookup_constant(name: &str) -> Option<BigDecimal> {
    let literal = match name {
        "pi" => PI_LITERAL,
        "e" => E_LITERAL,
        "tau" => TAU_LITERAL,
        "phi" => PHI_LITERAL,
        _ => return None,
    };
    BigDecimal::from_str(literal).ok()
}

/// Evaluate an expression exactly.
///
/// The returned string is a normalized `BigDecimal` — trailing zeros stripped,
/// plain (non-scientific) notation.
///
/// # Errors
/// Returns [`ExpressionError`] if the expression is blank, malformed, references
/// an unknown variable, calls an unknown function, or triggers a domain
/// violation (e.g. `sqrt(-1)`, `log(0)`).
///
/// # Panics
/// Panics if the `astro-float` runtime fails to initialize its shared
/// constants table — practically impossible on a functional allocator.
pub fn evaluate(expression: &str) -> Result<String, ExpressionError> {
    evaluate_with_variables(expression, &HashMap::new())
}

/// Evaluate with variable bindings.
///
/// Values are parsed as `BigDecimal`, so passing strings like
/// `"3.141592653589793238462643383279502884"` preserves every digit — unlike
/// the f64 variant which truncates at ~17 digits.
///
/// # Errors
/// Returns [`ExpressionError`] on the same conditions as [`evaluate`]: blank
/// input, malformed syntax, unknown identifier, or transcendental domain
/// violation.
///
/// # Panics
/// Panics if the `astro-float` runtime fails to initialize its shared
/// constants table — practically impossible on a functional allocator.
pub fn evaluate_with_variables<S: BuildHasher>(
    expression: &str,
    variables: &HashMap<String, String, S>,
) -> Result<String, ExpressionError> {
    if expression.trim().is_empty() {
        return Err(ExpressionError::Empty);
    }
    let mut consts = Consts::new().expect("init astro-float Consts");
    let result = {
        let mut parser = Parser::new(expression, variables, &mut consts);
        let value = parser.parse_expression()?;
        parser.skip_whitespace();
        if let Some(ch) = parser.current_char() {
            return Err(ExpressionError::UnexpectedChar {
                pos: parser.pos,
                ch,
            });
        }
        value
    };
    Ok(strip_plain(&result))
}

// --------------------------------------------------------------------------- //
//  BigDecimal ↔ BigFloat bridge (used only for transcendentals)
// --------------------------------------------------------------------------- //

fn bd_to_bf(value: &BigDecimal, consts: &mut Consts) -> BigFloat {
    BigFloat::parse(
        &value.to_plain_string(),
        Radix::Dec,
        AF_PRECISION,
        AfRm::None,
        consts,
    )
}

fn bf_to_bd(value: &BigFloat, consts: &mut Consts) -> BigDecimal {
    let formatted = value
        .format(Radix::Dec, AfRm::ToEven, consts)
        .unwrap_or_else(|_| "0".to_string());
    BigDecimal::from_str(&formatted)
        .unwrap_or_else(|_| BigDecimal::zero())
        .with_prec(EXACT_PRECISION)
}

fn to_radians(degrees: &BigDecimal, consts: &mut Consts) -> BigFloat {
    let factor = BigFloat::parse(
        DEG_TO_RAD_LITERAL,
        Radix::Dec,
        AF_PRECISION,
        AfRm::None,
        consts,
    );
    let deg_bf = bd_to_bf(degrees, consts);
    deg_bf.mul(&factor, AF_PRECISION, AfRm::ToEven)
}

fn radians_to_degrees(rad: &BigFloat, consts: &mut Consts) -> BigFloat {
    let factor = BigFloat::parse(
        RAD_TO_DEG_LITERAL,
        Radix::Dec,
        AF_PRECISION,
        AfRm::None,
        consts,
    );
    rad.mul(&factor, AF_PRECISION, AfRm::ToEven)
}

// --------------------------------------------------------------------------- //
//  Exact arithmetic helpers
// --------------------------------------------------------------------------- //

fn divide(lhs: &BigDecimal, rhs: &BigDecimal) -> Result<BigDecimal, ExpressionError> {
    if rhs.is_zero() {
        return Err(ExpressionError::DivisionByZero);
    }
    Ok((lhs / rhs).with_prec(EXACT_PRECISION))
}

fn modulo(lhs: &BigDecimal, rhs: &BigDecimal) -> Result<BigDecimal, ExpressionError> {
    if rhs.is_zero() {
        return Err(ExpressionError::DivisionByZero);
    }
    Ok(lhs % rhs)
}

/// If `exp` is a non-negative integer that fits in `u32`, return it.
fn as_nonneg_u32(exp: &BigDecimal) -> Option<u32> {
    if !exp.is_integer() || exp.is_negative() {
        return None;
    }
    exp.to_u32()
}

/// Reject astro-float results that leaked NaN / ±Inf — those mean the operand
/// left the transcendental's real-valued domain (e.g. `log(0)`, `sqrt(-2)`).
/// Without this guard, `bf_to_bd` would silently turn them into `0`.
fn finite_or_domain(bf: &BigFloat, op: &str, value: &BigDecimal) -> Result<(), ExpressionError> {
    if bf.is_nan() || bf.is_inf() {
        return Err(ExpressionError::DomainError {
            op: op.to_string(),
            value: value.to_plain_string(),
        });
    }
    Ok(())
}

/// Decide whether an integer-exponent power can stay exact. Zero / `|base| <= 1`
/// short-circuit (the result stays small regardless of exp). Otherwise the
/// printed result length is bounded by `base_len * exp`; anything above
/// [`MAX_POWER_RESULT_LEN`] would blow up both memory and output size.
fn integer_power_fits(base: &BigDecimal, exp: u32) -> bool {
    if exp == 0 || base.is_zero() {
        return true;
    }
    let base_len = base.to_plain_string().len() as u64;
    base_len.saturating_mul(u64::from(exp)) <= MAX_POWER_RESULT_LEN
}

/// Float-pow fallback for bases/exponents the integer path can't keep exact.
/// Required when `(1 + 1/1_000_000)^1_000_000 → e` — naive repeated
/// multiplication would balloon the decimal scale, but `BigFloat` at 192-bit
/// precision converges fine.
fn power_via_bigfloat(
    base: &BigDecimal,
    exp: &BigDecimal,
    consts: &mut Consts,
) -> Result<BigDecimal, ExpressionError> {
    let base_bf = bd_to_bf(base, consts);
    let exp_bf = bd_to_bf(exp, consts);
    let out = base_bf.pow(&exp_bf, AF_PRECISION, AfRm::ToEven, consts);
    if out.is_nan() || out.is_inf() {
        return Err(ExpressionError::DomainError {
            op: "pow".into(),
            value: format!("{}^{}", base.to_plain_string(), exp.to_plain_string()),
        });
    }
    Ok(bf_to_bd(&out, consts))
}

/// Exponentiation. Integer exponents whose expanded result fits within
/// [`MAX_POWER_RESULT_LEN`] stay exact via `BigDecimal::powi`; oversized
/// integer exponents fall through to `BigFloat` so `(1 + 1/n)^n → e` keeps
/// converging. Negative integers invert the base when the magnitude fits.
fn power(
    base: &BigDecimal,
    exp: &BigDecimal,
    consts: &mut Consts,
) -> Result<BigDecimal, ExpressionError> {
    if let Some(e) = as_nonneg_u32(exp) {
        if integer_power_fits(base, e) {
            return Ok(base.powi(i64::from(e)));
        }
        return power_via_bigfloat(base, exp, consts);
    }
    if exp.is_integer()
        && exp.is_negative()
        && let Some(abs_e) = exp.abs().to_u32()
        && integer_power_fits(base, abs_e)
    {
        let positive = base.powi(i64::from(abs_e));
        return divide(&BigDecimal::from(1), &positive);
    }
    power_via_bigfloat(base, exp, consts)
}

fn ceil(value: &BigDecimal) -> BigDecimal {
    value.with_scale_round(0, BdRm::Ceiling)
}

fn floor(value: &BigDecimal) -> BigDecimal {
    value.with_scale_round(0, BdRm::Floor)
}

fn sqrt_bd(value: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let bf = bd_to_bf(value, consts);
    let out = bf.sqrt(AF_PRECISION, AfRm::ToEven);
    finite_or_domain(&out, "sqrt", value)?;
    Ok(bf_to_bd(&out, consts))
}

fn ln_bd(value: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let bf = bd_to_bf(value, consts);
    let out = bf.ln(AF_PRECISION, AfRm::ToEven, consts);
    finite_or_domain(&out, "log", value)?;
    Ok(bf_to_bd(&out, consts))
}

fn log10_bd(value: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let bf = bd_to_bf(value, consts);
    let out = bf.log10(AF_PRECISION, AfRm::ToEven, consts);
    finite_or_domain(&out, "log10", value)?;
    Ok(bf_to_bd(&out, consts))
}

fn log2_bd(value: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let bf = bd_to_bf(value, consts);
    let out = bf.log2(AF_PRECISION, AfRm::ToEven, consts);
    finite_or_domain(&out, "log2", value)?;
    Ok(bf_to_bd(&out, consts))
}

fn sin_bd(degrees: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let rad = to_radians(degrees, consts);
    let out = rad.sin(AF_PRECISION, AfRm::ToEven, consts);
    finite_or_domain(&out, "sin", degrees)?;
    Ok(bf_to_bd(&out, consts))
}

fn cos_bd(degrees: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let rad = to_radians(degrees, consts);
    let out = rad.cos(AF_PRECISION, AfRm::ToEven, consts);
    finite_or_domain(&out, "cos", degrees)?;
    Ok(bf_to_bd(&out, consts))
}

fn tan_bd(degrees: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let rad = to_radians(degrees, consts);
    let out = rad.tan(AF_PRECISION, AfRm::ToEven, consts);
    finite_or_domain(&out, "tan", degrees)?;
    Ok(bf_to_bd(&out, consts))
}

// Radian-input variants. `sin(pi)` interprets pi in degrees (~0.0548);
// `sin_r(pi)` returns 0 as callers expect when combining trig with the pi
// constant.
fn sin_rad_bd(radians: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let bf = bd_to_bf(radians, consts);
    let out = bf.sin(AF_PRECISION, AfRm::ToEven, consts);
    finite_or_domain(&out, "sin_r", radians)?;
    Ok(bf_to_bd(&out, consts))
}

fn cos_rad_bd(radians: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let bf = bd_to_bf(radians, consts);
    let out = bf.cos(AF_PRECISION, AfRm::ToEven, consts);
    finite_or_domain(&out, "cos_r", radians)?;
    Ok(bf_to_bd(&out, consts))
}

fn tan_rad_bd(radians: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let bf = bd_to_bf(radians, consts);
    let out = bf.tan(AF_PRECISION, AfRm::ToEven, consts);
    finite_or_domain(&out, "tan_r", radians)?;
    Ok(bf_to_bd(&out, consts))
}

fn exp_bd(value: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let bf = bd_to_bf(value, consts);
    let out = bf.exp(AF_PRECISION, AfRm::ToEven, consts);
    finite_or_domain(&out, "exp", value)?;
    Ok(bf_to_bd(&out, consts))
}

fn asin_bd(value: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let one = BigDecimal::from(1);
    let neg_one = BigDecimal::from(-1);
    if value < &neg_one || value > &one {
        return Err(ExpressionError::DomainError {
            op: "asin".into(),
            value: value.to_plain_string(),
        });
    }
    let bf = bd_to_bf(value, consts);
    let rad = bf.asin(AF_PRECISION, AfRm::ToEven, consts);
    finite_or_domain(&rad, "asin", value)?;
    let deg = radians_to_degrees(&rad, consts);
    Ok(bf_to_bd(&deg, consts))
}

fn acos_bd(value: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let one = BigDecimal::from(1);
    let neg_one = BigDecimal::from(-1);
    if value < &neg_one || value > &one {
        return Err(ExpressionError::DomainError {
            op: "acos".into(),
            value: value.to_plain_string(),
        });
    }
    let bf = bd_to_bf(value, consts);
    let rad = bf.acos(AF_PRECISION, AfRm::ToEven, consts);
    finite_or_domain(&rad, "acos", value)?;
    let deg = radians_to_degrees(&rad, consts);
    Ok(bf_to_bd(&deg, consts))
}

fn atan_bd(value: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let bf = bd_to_bf(value, consts);
    let rad = bf.atan(AF_PRECISION, AfRm::ToEven, consts);
    finite_or_domain(&rad, "atan", value)?;
    let deg = radians_to_degrees(&rad, consts);
    Ok(bf_to_bd(&deg, consts))
}

fn sinh_bd(value: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let bf = bd_to_bf(value, consts);
    let out = bf.sinh(AF_PRECISION, AfRm::ToEven, consts);
    finite_or_domain(&out, "sinh", value)?;
    Ok(bf_to_bd(&out, consts))
}

fn cosh_bd(value: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let bf = bd_to_bf(value, consts);
    let out = bf.cosh(AF_PRECISION, AfRm::ToEven, consts);
    finite_or_domain(&out, "cosh", value)?;
    Ok(bf_to_bd(&out, consts))
}

fn tanh_bd(value: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let bf = bd_to_bf(value, consts);
    let out = bf.tanh(AF_PRECISION, AfRm::ToEven, consts);
    finite_or_domain(&out, "tanh", value)?;
    Ok(bf_to_bd(&out, consts))
}

/// Common helper for the inverse hyperbolics — astro-float exposes them as
/// `BigFloat::asinh / acosh / atanh`. acosh requires `x >= 1`; atanh requires
/// `|x| < 1`. Domain checks happen on the `BigDecimal` operand before the
/// transcendental call so the error detail keeps the original input.
fn asinh_bd(value: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let bf = bd_to_bf(value, consts);
    let out = bf.asinh(AF_PRECISION, AfRm::ToEven, consts);
    finite_or_domain(&out, "asinh", value)?;
    Ok(bf_to_bd(&out, consts))
}

fn acosh_bd(value: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    if value < &BigDecimal::from(1) {
        return Err(ExpressionError::DomainError {
            op: "acosh".into(),
            value: value.to_plain_string(),
        });
    }
    let bf = bd_to_bf(value, consts);
    let out = bf.acosh(AF_PRECISION, AfRm::ToEven, consts);
    finite_or_domain(&out, "acosh", value)?;
    Ok(bf_to_bd(&out, consts))
}

fn atanh_bd(value: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let one = BigDecimal::from(1);
    let neg_one = BigDecimal::from(-1);
    if value <= &neg_one || value >= &one {
        return Err(ExpressionError::DomainError {
            op: "atanh".into(),
            value: value.to_plain_string(),
        });
    }
    let bf = bd_to_bf(value, consts);
    let out = bf.atanh(AF_PRECISION, AfRm::ToEven, consts);
    finite_or_domain(&out, "atanh", value)?;
    Ok(bf_to_bd(&out, consts))
}

fn cbrt_bd(value: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    let bf = bd_to_bf(value, consts);
    let out = bf.cbrt(AF_PRECISION, AfRm::ToEven);
    finite_or_domain(&out, "cbrt", value)?;
    Ok(bf_to_bd(&out, consts))
}

fn round_bd(value: &BigDecimal) -> BigDecimal {
    value.with_scale_round(0, BdRm::HalfUp)
}

fn trunc_bd(value: &BigDecimal) -> BigDecimal {
    value.with_scale_round(0, BdRm::Down)
}

fn sign_bd(value: &BigDecimal) -> BigDecimal {
    if value.is_zero() {
        BigDecimal::zero()
    } else if value.is_negative() {
        BigDecimal::from(-1)
    } else {
        BigDecimal::from(1)
    }
}

fn factorial_bd(value: &BigDecimal) -> Result<BigDecimal, ExpressionError> {
    if !value.is_integer() || value.is_negative() {
        return Err(ExpressionError::DomainError {
            op: "factorial".into(),
            value: value.to_plain_string(),
        });
    }
    // Integer-in-range but above the 1000 cap is `OutOfRange`, not `Overflow`.
    // Overflow is reserved for results that actually leave the arithmetic
    // universe (e.g. f64 +Inf); factorial(2000) is well-defined — we just
    // refuse to compute it.
    let n = value.to_u32().ok_or_else(|| ExpressionError::OutOfRange {
        op: "factorial".into(),
        value: value.to_plain_string(),
        min: "0".into(),
        max: "1000".into(),
    })?;
    if n > 1000 {
        return Err(ExpressionError::OutOfRange {
            op: "factorial".into(),
            value: value.to_plain_string(),
            min: "0".into(),
            max: "1000".into(),
        });
    }
    let mut acc = BigDecimal::from(1);
    for i in 2..=n {
        acc = &acc * BigDecimal::from(i);
    }
    Ok(acc)
}

fn integer_binop_bd(
    lhs: &BigDecimal,
    rhs: &BigDecimal,
    op: &str,
    f: fn(u64, u64) -> u64,
) -> Result<BigDecimal, ExpressionError> {
    if !lhs.is_integer() || !rhs.is_integer() {
        return Err(ExpressionError::DomainError {
            op: op.to_string(),
            value: format!("{},{}", lhs.to_plain_string(), rhs.to_plain_string()),
        });
    }
    let a = lhs
        .abs()
        .to_u64()
        .ok_or_else(|| ExpressionError::DomainError {
            op: op.to_string(),
            value: lhs.to_plain_string(),
        })?;
    let b = rhs
        .abs()
        .to_u64()
        .ok_or_else(|| ExpressionError::DomainError {
            op: op.to_string(),
            value: rhs.to_plain_string(),
        })?;
    Ok(BigDecimal::from(f(a, b)))
}

const fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

const fn lcm_u64(a: u64, b: u64) -> u64 {
    if a == 0 || b == 0 {
        0
    } else {
        a / gcd_u64(a, b) * b
    }
}

fn check_arity_bd(args: &[BigDecimal], expected: usize, op: &str) -> Result<(), ExpressionError> {
    if args.len() == expected {
        Ok(())
    } else {
        Err(ExpressionError::DomainError {
            op: op.to_string(),
            value: format!("arity={}, expected={expected}", args.len()),
        })
    }
}

/// `atan2(y, x)` in degrees. astro-float doesn't expose atan2 directly, so we
/// reduce to `atan(y/x)` with quadrant fixups (±π) and special-case the axes.
/// Result range: (-180, 180].
fn atan2_bd(
    y: &BigDecimal,
    x: &BigDecimal,
    consts: &mut Consts,
) -> Result<BigDecimal, ExpressionError> {
    if x.is_zero() && y.is_zero() {
        return Err(ExpressionError::DomainError {
            op: "atan2".into(),
            value: "0,0".into(),
        });
    }
    if x.is_zero() {
        // y != 0 here. Result is ±90°.
        let sign = if y.is_negative() { -1 } else { 1 };
        return Ok(BigDecimal::from(sign * 90));
    }
    // Compute atan(y/x) (degrees).
    let ratio = y / x;
    let principal = atan_bd(&ratio, consts)?;
    if !x.is_negative() {
        // x > 0: principal value already correct.
        return Ok(principal);
    }
    // x < 0: shift by ±180° depending on sign of y.
    if y.is_negative() {
        Ok(principal - BigDecimal::from(180))
    } else {
        // y >= 0 here. y == 0 was handled above (x.is_zero false / y.is_zero true means
        // x != 0, y == 0 → we want 180° if x < 0, 0° if x > 0 — covered by principal+180).
        Ok(principal + BigDecimal::from(180))
    }
}

fn hypot_bd(
    x: &BigDecimal,
    y: &BigDecimal,
    consts: &mut Consts,
) -> Result<BigDecimal, ExpressionError> {
    let sq = x * x + y * y;
    sqrt_bd(&sq, consts)
}

// --------------------------------------------------------------------------- //
//  Recursive-descent parser
// --------------------------------------------------------------------------- //

struct Parser<'a, 'c, S: BuildHasher> {
    input: Vec<char>,
    variables: &'a HashMap<String, String, S>,
    pos: usize,
    /// Tracks unmatched `(` count so that an unknown identifier hitting
    /// end-of-input inside an open paren is reported as a parse error
    /// (unclosed paren) rather than `UNKNOWN_VARIABLE`.
    paren_depth: u32,
    consts: &'c mut Consts,
}

impl<'a, 'c, S: BuildHasher> Parser<'a, 'c, S> {
    fn new(input: &str, variables: &'a HashMap<String, String, S>, consts: &'c mut Consts) -> Self {
        Self {
            input: input.chars().collect(),
            variables,
            pos: 0,
            paren_depth: 0,
            consts,
        }
    }

    fn current_char(&self) -> Option<char> {
        self.input.get(self.pos).copied()
    }

    /// Skip whitespace at the current position. Matches the f64 parser:
    /// whitespace only separates tokens, it never fuses them.
    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.current_char() {
            if ch.is_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn parse_expression(&mut self) -> Result<BigDecimal, ExpressionError> {
        let mut result = self.parse_term()?;
        loop {
            self.skip_whitespace();
            match self.current_char() {
                Some('+') => {
                    self.pos += 1;
                    result = &result + &self.parse_term()?;
                }
                Some('-') => {
                    self.pos += 1;
                    result = &result - &self.parse_term()?;
                }
                _ => break,
            }
        }
        Ok(result)
    }

    // Grammar (mirrors the f64 evaluator — see `expression.rs` for the
    // rationale): term → unary → power → primary, so unary minus stays below
    // `^` and `-2^2` evaluates as `-(2^2) = -4`.
    fn parse_term(&mut self) -> Result<BigDecimal, ExpressionError> {
        let mut result = self.parse_unary()?;
        loop {
            self.skip_whitespace();
            match self.current_char() {
                Some('*') => {
                    self.pos += 1;
                    result = &result * &self.parse_unary()?;
                }
                Some('/') => {
                    self.pos += 1;
                    let rhs = self.parse_unary()?;
                    result = divide(&result, &rhs)?;
                }
                Some('%') => {
                    self.pos += 1;
                    let rhs = self.parse_unary()?;
                    result = modulo(&result, &rhs)?;
                }
                _ => break,
            }
        }
        Ok(result)
    }

    fn parse_power(&mut self) -> Result<BigDecimal, ExpressionError> {
        let base = self.parse_primary()?;
        self.skip_whitespace();
        if self.current_char() == Some('^') {
            self.pos += 1;
            let exponent = self.parse_unary()?;
            return power(&base, &exponent, self.consts);
        }
        Ok(base)
    }

    fn parse_unary(&mut self) -> Result<BigDecimal, ExpressionError> {
        self.skip_whitespace();
        if self.current_char() == Some('-') {
            self.pos += 1;
            let value = self.parse_unary()?;
            return Ok(-value);
        }
        self.parse_power()
    }

    fn parse_primary(&mut self) -> Result<BigDecimal, ExpressionError> {
        self.skip_whitespace();
        let ch = self.current_char().ok_or(ExpressionError::UnexpectedEnd)?;
        if ch == '(' {
            self.pos += 1;
            self.paren_depth += 1;
            let value = self.parse_expression()?;
            self.expect_close_paren()?;
            self.paren_depth -= 1;
            Ok(value)
        } else if ch.is_ascii_digit() || ch == '.' {
            self.parse_number()
        } else if ch.is_alphabetic() || ch == '_' {
            self.parse_identifier()
        } else {
            Err(ExpressionError::UnexpectedChar { pos: self.pos, ch })
        }
    }

    fn parse_number(&mut self) -> Result<BigDecimal, ExpressionError> {
        let start = self.pos;
        self.consume_digits();
        if self.current_char() == Some('.') {
            self.pos += 1;
            self.consume_digits();
        }
        if matches!(self.current_char(), Some('e' | 'E')) {
            self.pos += 1;
            if matches!(self.current_char(), Some('+' | '-')) {
                self.pos += 1;
            }
            self.consume_digits();
        }
        let token: String = self.input[start..self.pos].iter().collect();
        BigDecimal::from_str(&token).map_err(|_| ExpressionError::InvalidNumber(token))
    }

    fn consume_digits(&mut self) {
        while let Some(ch) = self.current_char() {
            if ch.is_ascii_digit() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn parse_identifier(&mut self) -> Result<BigDecimal, ExpressionError> {
        let start = self.pos;
        while let Some(ch) = self.current_char() {
            if ch.is_alphanumeric() || ch == '_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let name: String = self.input[start..self.pos].iter().collect();
        self.skip_whitespace();

        if self.current_char() == Some('(') {
            self.pos += 1;
            self.paren_depth += 1;
            let args = self.parse_call_arguments()?;
            self.paren_depth -= 1;
            self.call_function(&name, &args)
        } else if let Some(value) = self.variables.get(&name) {
            BigDecimal::from_str(value).map_err(|_| ExpressionError::InvalidNumber(value.clone()))
        } else if let Some(value) = lookup_constant(&name) {
            Ok(value)
        } else if self.paren_depth > 0 && self.current_char().is_none() {
            // Unclosed paren wins over UNKNOWN_VARIABLE when we bailed out at
            // end-of-input inside an open parenthesis context — the caller
            // really fed us a malformed expression like `((bad`.
            Err(ExpressionError::ExpectedCloseParen { pos: self.pos })
        } else {
            Err(ExpressionError::UnknownVariable(name))
        }
    }

    fn parse_call_arguments(&mut self) -> Result<Vec<BigDecimal>, ExpressionError> {
        self.skip_whitespace();
        if self.current_char() == Some(')') {
            self.pos += 1;
            return Ok(Vec::new());
        }
        let mut args = vec![self.parse_expression()?];
        loop {
            self.skip_whitespace();
            match self.current_char() {
                Some(',') => {
                    self.pos += 1;
                    args.push(self.parse_expression()?);
                }
                Some(')') => {
                    self.pos += 1;
                    return Ok(args);
                }
                Some(_) | None => {
                    return Err(ExpressionError::ExpectedCloseParen { pos: self.pos });
                }
            }
        }
    }

    fn expect_close_paren(&mut self) -> Result<(), ExpressionError> {
        self.skip_whitespace();
        if self.current_char() != Some(')') {
            return Err(ExpressionError::ExpectedCloseParen { pos: self.pos });
        }
        self.pos += 1;
        Ok(())
    }

    fn call_function(
        &mut self,
        name: &str,
        args: &[BigDecimal],
    ) -> Result<BigDecimal, ExpressionError> {
        match name {
            "sin" | "cos" | "tan" | "sin_r" | "cos_r" | "tan_r" | "asin" | "acos" | "atan"
            | "atan2" => self.dispatch_trig(name, args),
            "sinh" | "cosh" | "tanh" | "asinh" | "acosh" | "atanh" => {
                self.dispatch_hyperbolic(name, args)
            }
            "exp" | "log" | "ln" | "log10" | "log2" | "sqrt" | "cbrt" => {
                self.dispatch_exp_log(name, args)
            }
            "abs" | "ceil" | "floor" | "round" | "trunc" | "sign" | "factorial" => {
                Self::dispatch_round_sign(name, args)
            }
            "min" | "max" | "mod" | "hypot" | "pow" | "gcd" | "lcm" => {
                self.dispatch_multi_arg(name, args)
            }
            _ => Err(ExpressionError::UnknownFunction(name.to_string())),
        }
    }

    fn dispatch_trig(
        &mut self,
        name: &str,
        args: &[BigDecimal],
    ) -> Result<BigDecimal, ExpressionError> {
        match name {
            "sin" => {
                check_arity_bd(args, 1, "sin")?;
                sin_bd(&args[0], self.consts)
            }
            "cos" => {
                check_arity_bd(args, 1, "cos")?;
                cos_bd(&args[0], self.consts)
            }
            "tan" => {
                check_arity_bd(args, 1, "tan")?;
                tan_bd(&args[0], self.consts)
            }
            "sin_r" => {
                check_arity_bd(args, 1, "sin_r")?;
                sin_rad_bd(&args[0], self.consts)
            }
            "cos_r" => {
                check_arity_bd(args, 1, "cos_r")?;
                cos_rad_bd(&args[0], self.consts)
            }
            "tan_r" => {
                check_arity_bd(args, 1, "tan_r")?;
                tan_rad_bd(&args[0], self.consts)
            }
            "asin" => {
                check_arity_bd(args, 1, "asin")?;
                asin_bd(&args[0], self.consts)
            }
            "acos" => {
                check_arity_bd(args, 1, "acos")?;
                acos_bd(&args[0], self.consts)
            }
            "atan" => {
                check_arity_bd(args, 1, "atan")?;
                atan_bd(&args[0], self.consts)
            }
            "atan2" => {
                check_arity_bd(args, 2, "atan2")?;
                atan2_bd(&args[0], &args[1], self.consts)
            }
            _ => Err(ExpressionError::UnknownFunction(name.to_string())),
        }
    }

    fn dispatch_hyperbolic(
        &mut self,
        name: &str,
        args: &[BigDecimal],
    ) -> Result<BigDecimal, ExpressionError> {
        match name {
            "sinh" => {
                check_arity_bd(args, 1, "sinh")?;
                sinh_bd(&args[0], self.consts)
            }
            "cosh" => {
                check_arity_bd(args, 1, "cosh")?;
                cosh_bd(&args[0], self.consts)
            }
            "tanh" => {
                check_arity_bd(args, 1, "tanh")?;
                tanh_bd(&args[0], self.consts)
            }
            "asinh" => {
                check_arity_bd(args, 1, "asinh")?;
                asinh_bd(&args[0], self.consts)
            }
            "acosh" => {
                check_arity_bd(args, 1, "acosh")?;
                acosh_bd(&args[0], self.consts)
            }
            "atanh" => {
                check_arity_bd(args, 1, "atanh")?;
                atanh_bd(&args[0], self.consts)
            }
            _ => Err(ExpressionError::UnknownFunction(name.to_string())),
        }
    }

    fn dispatch_exp_log(
        &mut self,
        name: &str,
        args: &[BigDecimal],
    ) -> Result<BigDecimal, ExpressionError> {
        match name {
            "exp" => {
                check_arity_bd(args, 1, "exp")?;
                exp_bd(&args[0], self.consts)
            }
            "log" | "ln" => {
                check_arity_bd(args, 1, name)?;
                ln_bd(&args[0], self.consts)
            }
            "log10" => {
                check_arity_bd(args, 1, "log10")?;
                log10_bd(&args[0], self.consts)
            }
            "log2" => {
                check_arity_bd(args, 1, "log2")?;
                log2_bd(&args[0], self.consts)
            }
            "sqrt" => {
                check_arity_bd(args, 1, "sqrt")?;
                sqrt_bd(&args[0], self.consts)
            }
            "cbrt" => {
                check_arity_bd(args, 1, "cbrt")?;
                cbrt_bd(&args[0], self.consts)
            }
            _ => Err(ExpressionError::UnknownFunction(name.to_string())),
        }
    }

    fn dispatch_round_sign(name: &str, args: &[BigDecimal]) -> Result<BigDecimal, ExpressionError> {
        match name {
            "abs" => {
                check_arity_bd(args, 1, "abs")?;
                Ok(args[0].abs())
            }
            "ceil" => {
                check_arity_bd(args, 1, "ceil")?;
                Ok(ceil(&args[0]))
            }
            "floor" => {
                check_arity_bd(args, 1, "floor")?;
                Ok(floor(&args[0]))
            }
            "round" => {
                check_arity_bd(args, 1, "round")?;
                Ok(round_bd(&args[0]))
            }
            "trunc" => {
                check_arity_bd(args, 1, "trunc")?;
                Ok(trunc_bd(&args[0]))
            }
            "sign" => {
                check_arity_bd(args, 1, "sign")?;
                Ok(sign_bd(&args[0]))
            }
            "factorial" => {
                check_arity_bd(args, 1, "factorial")?;
                factorial_bd(&args[0])
            }
            _ => Err(ExpressionError::UnknownFunction(name.to_string())),
        }
    }

    fn dispatch_multi_arg(
        &mut self,
        name: &str,
        args: &[BigDecimal],
    ) -> Result<BigDecimal, ExpressionError> {
        match name {
            "min" => {
                if args.is_empty() {
                    return Err(ExpressionError::DomainError {
                        op: "min".into(),
                        value: "arity=0, expected>=1".into(),
                    });
                }
                Ok(args.iter().min().cloned().unwrap_or_else(BigDecimal::zero))
            }
            "max" => {
                if args.is_empty() {
                    return Err(ExpressionError::DomainError {
                        op: "max".into(),
                        value: "arity=0, expected>=1".into(),
                    });
                }
                Ok(args.iter().max().cloned().unwrap_or_else(BigDecimal::zero))
            }
            "mod" => {
                check_arity_bd(args, 2, "mod")?;
                modulo(&args[0], &args[1])
            }
            "hypot" => {
                check_arity_bd(args, 2, "hypot")?;
                hypot_bd(&args[0], &args[1], self.consts)
            }
            "pow" => {
                check_arity_bd(args, 2, "pow")?;
                power(&args[0], &args[1], self.consts)
            }
            "gcd" => {
                check_arity_bd(args, 2, "gcd")?;
                integer_binop_bd(&args[0], &args[1], "gcd", gcd_u64)
            }
            "lcm" => {
                check_arity_bd(args, 2, "lcm")?;
                integer_binop_bd(&args[0], &args[1], "lcm", lcm_u64)
            }
            _ => Err(ExpressionError::UnknownFunction(name.to_string())),
        }
    }
}

// --------------------------------------------------------------------------- //
//  Tests
// --------------------------------------------------------------------------- //

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_addition_is_exact() {
        assert_eq!(evaluate("0.1 + 0.2").unwrap(), "0.3");
        assert_eq!(evaluate("0.1 + 0.2 + 0.3").unwrap(), "0.6");
    }

    #[test]
    fn integer_arithmetic_strips_trailing_zeros() {
        assert_eq!(evaluate("2+3*4").unwrap(), "14");
        assert_eq!(evaluate("2^10").unwrap(), "1024");
        assert_eq!(evaluate("(2+3)*(4-1)").unwrap(), "15");
    }

    #[test]
    fn division_uses_exact_precision_scale() {
        let out = evaluate("1/3").unwrap();
        assert!(out.starts_with("0.333333333333333333"), "got {out}");
    }

    #[test]
    fn division_by_zero_is_error() {
        assert_eq!(
            evaluate("1/0").unwrap_err(),
            ExpressionError::DivisionByZero
        );
    }

    #[test]
    fn modulo_by_zero_is_error() {
        assert_eq!(
            evaluate("5 % 0").unwrap_err(),
            ExpressionError::DivisionByZero
        );
    }

    #[test]
    fn modulo_operator_exact() {
        assert_eq!(evaluate("10 % 3").unwrap(), "1");
        assert_eq!(evaluate("7.5 % 2").unwrap(), "1.5");
    }

    #[test]
    fn power_integer_exponent_is_exact() {
        assert_eq!(evaluate("2^3^2").unwrap(), "512");
        assert_eq!(evaluate("1.5^2").unwrap(), "2.25");
    }

    #[test]
    fn unary_minus() {
        // Unary minus has lower precedence than `^`; `-2^2` is `-(2^2) = -4`,
        // matching Python / NumPy / Excel and the engine's own `0 - 2^2 = -4`.
        assert_eq!(evaluate("-2^2").unwrap(), "-4");
        assert_eq!(evaluate("(-2)^2").unwrap(), "4");
        assert_eq!(evaluate("2^-3").unwrap(), "0.125");
        assert_eq!(evaluate("--5").unwrap(), "5");
    }

    #[test]
    fn abs_ceil_floor_exact() {
        assert_eq!(evaluate("abs(-3.14)").unwrap(), "3.14");
        assert_eq!(evaluate("floor(3.9)+ceil(3.1)").unwrap(), "7");
    }

    #[test]
    fn sqrt_irrational_has_many_digits() {
        let out = evaluate("sqrt(2)").unwrap();
        assert!(out.starts_with("1.4142135623730950488"), "got {out}");
    }

    #[test]
    fn long_decimal_variable_preserved() {
        let mut vars = HashMap::new();
        vars.insert("pi".to_string(), "3.1415926535897932384626433".to_string());
        let out = evaluate_with_variables("pi * 2", &vars).unwrap();
        assert_eq!(out, "6.2831853071795864769252866");
    }

    #[test]
    fn blank_expression_is_error() {
        assert_eq!(evaluate("").unwrap_err(), ExpressionError::Empty);
    }

    #[test]
    fn unknown_variable_is_error() {
        assert_eq!(
            evaluate("x + 1").unwrap_err(),
            ExpressionError::UnknownVariable("x".into())
        );
    }

    #[test]
    fn unknown_function_is_error() {
        assert_eq!(
            evaluate("foo(1)").unwrap_err(),
            ExpressionError::UnknownFunction("foo".into())
        );
    }

    #[test]
    fn sqrt_of_negative_is_domain_error() {
        // Before the finite-or-domain guard this silently returned "0".
        match evaluate("sqrt(-2)").unwrap_err() {
            ExpressionError::DomainError { op, value } => {
                assert_eq!(op, "sqrt");
                assert_eq!(value, "-2");
            }
            other => panic!("expected DomainError, got {other:?}"),
        }
    }

    #[test]
    fn log_of_zero_is_domain_error() {
        match evaluate("log(0)").unwrap_err() {
            ExpressionError::DomainError { op, value } => {
                assert_eq!(op, "log");
                assert_eq!(value, "0");
            }
            other => panic!("expected DomainError, got {other:?}"),
        }
    }

    #[test]
    fn log_of_negative_is_domain_error() {
        match evaluate("log(-1)").unwrap_err() {
            ExpressionError::DomainError { op, value } => {
                assert_eq!(op, "log");
                assert_eq!(value, "-1");
            }
            other => panic!("expected DomainError, got {other:?}"),
        }
    }

    #[test]
    fn log10_of_zero_is_domain_error() {
        match evaluate("log10(0)").unwrap_err() {
            ExpressionError::DomainError { op, value } => {
                assert_eq!(op, "log10");
                assert_eq!(value, "0");
            }
            other => panic!("expected DomainError, got {other:?}"),
        }
    }

    #[test]
    fn nested_domain_error_propagates() {
        // The outer addition should still surface the inner sqrt(-1) failure.
        assert!(matches!(
            evaluate("1 + sqrt(-1)").unwrap_err(),
            ExpressionError::DomainError { .. }
        ));
    }

    #[test]
    fn unclosed_paren_wins_over_unknown_variable() {
        // Regression: `((bad` used to surface as UnknownVariable("bad").
        let err = evaluate("((bad").unwrap_err();
        assert!(
            matches!(err, ExpressionError::ExpectedCloseParen { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn adjacent_numbers_reject() {
        // Regression: `"1 2 3"` used to collapse to `123` because the parser
        // stripped whitespace globally before tokenizing.
        let err = evaluate("1 2 3").unwrap_err();
        match err {
            ExpressionError::UnexpectedChar { pos, ch } => {
                assert_eq!(pos, 2);
                assert_eq!(ch, '2');
            }
            other => panic!("expected UnexpectedChar, got {other:?}"),
        }
    }

    #[test]
    fn whitespace_between_tokens_still_works() {
        // Whitespace must still be valid as a token separator.
        assert_eq!(evaluate("  1  +  2  ").unwrap(), "3");
        assert_eq!(evaluate("sqrt( 4 )").unwrap(), "2");
    }

    // ---- new constants ----

    #[test]
    fn const_pi_exact_truncated() {
        let out = evaluate("pi").unwrap();
        assert!(out.starts_with("3.14159265358979323846"), "got {out}");
    }

    #[test]
    fn const_e_exact_truncated() {
        let out = evaluate("e").unwrap();
        assert!(out.starts_with("2.71828182845904523536"), "got {out}");
    }

    #[test]
    fn const_tau_is_two_pi() {
        let out = evaluate("tau").unwrap();
        assert!(out.starts_with("6.28318530717958647692"), "got {out}");
    }

    #[test]
    fn const_phi_golden_ratio() {
        let out = evaluate("phi").unwrap();
        assert!(out.starts_with("1.61803398874989484820"), "got {out}");
    }

    // ---- new functions ----

    #[test]
    fn fn_exp_zero_is_one() {
        let out = evaluate("exp(0)").unwrap();
        assert!(out == "1" || out.starts_with("1.0"), "got {out}");
    }

    #[test]
    fn fn_exp_one_is_e() {
        let out = evaluate("exp(1)").unwrap();
        assert!(out.starts_with("2.71828"), "got {out}");
    }

    #[test]
    fn fn_ln_alias() {
        let out = evaluate("ln(e)").unwrap();
        assert!(out == "1" || out.starts_with("1.000000") || out.starts_with("0.99999"));
    }

    #[test]
    fn fn_inverse_trig_returns_degrees() {
        let asin = evaluate("asin(1)").unwrap();
        assert!(asin.starts_with("90"), "got {asin}");
        let acos = evaluate("acos(0)").unwrap();
        assert!(acos.starts_with("90"), "got {acos}");
        let atan = evaluate("atan(1)").unwrap();
        assert!(atan.starts_with("45"), "got {atan}");
    }

    #[test]
    fn fn_atan2_quadrants() {
        let q1 = evaluate("atan2(1, 1)").unwrap();
        assert!(q1.starts_with("45"), "got {q1}");
        let q2 = evaluate("atan2(1, -1)").unwrap();
        assert!(q2.starts_with("135"), "got {q2}");
    }

    #[test]
    fn fn_hyperbolic_zero() {
        assert_eq!(evaluate("sinh(0)").unwrap(), "0");
        let c = evaluate("cosh(0)").unwrap();
        assert!(c == "1" || c.starts_with("1.000"), "got {c}");
        assert_eq!(evaluate("tanh(0)").unwrap(), "0");
    }

    #[test]
    fn fn_round_trunc_sign_exact() {
        assert_eq!(evaluate("round(2.5)").unwrap(), "3");
        assert_eq!(evaluate("trunc(3.9)").unwrap(), "3");
        assert_eq!(evaluate("trunc(-3.9)").unwrap(), "-3");
        assert_eq!(evaluate("sign(-7)").unwrap(), "-1");
        assert_eq!(evaluate("sign(0)").unwrap(), "0");
        assert_eq!(evaluate("sign(5)").unwrap(), "1");
    }

    #[test]
    fn fn_factorial_exact_arbitrary_precision() {
        assert_eq!(evaluate("factorial(0)").unwrap(), "1");
        assert_eq!(evaluate("factorial(20)").unwrap(), "2432902008176640000");
        // 25! easily exceeds f64; exact mode handles it.
        assert_eq!(
            evaluate("factorial(25)").unwrap(),
            "15511210043330985984000000"
        );
    }

    #[test]
    fn fn_min_max_pick_extremes() {
        assert_eq!(evaluate("min(3, 1, 2)").unwrap(), "1");
        assert_eq!(evaluate("max(3, 1, 2)").unwrap(), "3");
    }

    #[test]
    fn fn_gcd_lcm_exact() {
        assert_eq!(evaluate("gcd(12, 18)").unwrap(), "6");
        assert_eq!(evaluate("lcm(4, 6)").unwrap(), "12");
    }

    #[test]
    fn fn_hypot_exact_pythagorean_triple() {
        assert_eq!(evaluate("hypot(3, 4)").unwrap(), "5");
    }

    #[test]
    fn fn_pow_two_args_exact() {
        assert_eq!(evaluate("pow(2, 10)").unwrap(), "1024");
        assert_eq!(evaluate("pow(1.5, 2)").unwrap(), "2.25");
    }

    #[test]
    fn fn_arity_mismatch_is_domain_error() {
        assert!(matches!(
            evaluate("sin(1, 2)").unwrap_err(),
            ExpressionError::DomainError { .. }
        ));
    }

    #[test]
    fn variable_shadows_constant() {
        let mut vars = HashMap::new();
        vars.insert("pi".to_string(), "3".to_string());
        assert_eq!(evaluate_with_variables("pi", &vars).unwrap(), "3");
    }
}
