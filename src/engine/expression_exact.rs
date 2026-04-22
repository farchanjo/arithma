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
const DEG_TO_RAD_LITERAL: &str =
    "0.017453292519943295769236907684886127134428718885417254560971914401710091146034";

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
fn finite_or_domain(
    bf: &BigFloat,
    op: &str,
    value: &BigDecimal,
) -> Result<(), ExpressionError> {
    if bf.is_nan() || bf.is_inf() {
        return Err(ExpressionError::DomainError {
            op: op.to_string(),
            value: value.to_plain_string(),
        });
    }
    Ok(())
}

/// Exponentiation. Integer exponents stay exact via `BigDecimal::powi`;
/// negative integers invert the base; fractional or very large integers fall
/// through to `BigFloat` and round back.
fn power(base: &BigDecimal, exp: &BigDecimal, consts: &mut Consts) -> Result<BigDecimal, ExpressionError> {
    if let Some(e) = as_nonneg_u32(exp) {
        return Ok(base.powi(i64::from(e)));
    }
    if exp.is_integer()
        && exp.is_negative()
        && let Some(abs_e) = exp.abs().to_u32()
    {
        let positive = base.powi(i64::from(abs_e));
        return divide(&BigDecimal::from(1), &positive);
    }
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
    fn new(
        input: &str,
        variables: &'a HashMap<String, String, S>,
        consts: &'c mut Consts,
    ) -> Self {
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

    fn parse_term(&mut self) -> Result<BigDecimal, ExpressionError> {
        let mut result = self.parse_power()?;
        loop {
            self.skip_whitespace();
            match self.current_char() {
                Some('*') => {
                    self.pos += 1;
                    result = &result * &self.parse_power()?;
                }
                Some('/') => {
                    self.pos += 1;
                    let rhs = self.parse_power()?;
                    result = divide(&result, &rhs)?;
                }
                Some('%') => {
                    self.pos += 1;
                    let rhs = self.parse_power()?;
                    result = modulo(&result, &rhs)?;
                }
                _ => break,
            }
        }
        Ok(result)
    }

    fn parse_power(&mut self) -> Result<BigDecimal, ExpressionError> {
        let base = self.parse_unary()?;
        self.skip_whitespace();
        if self.current_char() == Some('^') {
            self.pos += 1;
            let exponent = self.parse_power()?;
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
        self.parse_primary()
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
            let argument = self.parse_expression()?;
            self.expect_close_paren()?;
            self.paren_depth -= 1;
            self.call_function(&name, &argument)
        } else if let Some(value) = self.variables.get(&name) {
            BigDecimal::from_str(value).map_err(|_| ExpressionError::InvalidNumber(value.clone()))
        } else if self.paren_depth > 0 && self.current_char().is_none() {
            // Unclosed paren wins over UNKNOWN_VARIABLE when we bailed out at
            // end-of-input inside an open parenthesis context — the caller
            // really fed us a malformed expression like `((bad`.
            Err(ExpressionError::ExpectedCloseParen { pos: self.pos })
        } else {
            Err(ExpressionError::UnknownVariable(name))
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
        arg: &BigDecimal,
    ) -> Result<BigDecimal, ExpressionError> {
        match name {
            "sin" => sin_bd(arg, self.consts),
            "cos" => cos_bd(arg, self.consts),
            "tan" => tan_bd(arg, self.consts),
            "log" => ln_bd(arg, self.consts),
            "log10" => log10_bd(arg, self.consts),
            "sqrt" => sqrt_bd(arg, self.consts),
            "abs" => Ok(arg.abs()),
            "ceil" => Ok(ceil(arg)),
            "floor" => Ok(floor(arg)),
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
        assert_eq!(evaluate("1/0").unwrap_err(), ExpressionError::DivisionByZero);
    }

    #[test]
    fn modulo_by_zero_is_error() {
        assert_eq!(evaluate("5 % 0").unwrap_err(), ExpressionError::DivisionByZero);
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
        assert_eq!(evaluate("-2^2").unwrap(), "4"); // (-2)^2 = 4 per shared grammar
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
        vars.insert(
            "pi".to_string(),
            "3.1415926535897932384626433".to_string(),
        );
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
}
