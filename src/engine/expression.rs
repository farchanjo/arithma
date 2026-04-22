//! Recursive-descent expression evaluator.
//!
//! Pure-Rust port of `com.archanjo.mathcalculator.engine.ExpressionEvaluator`.
//! Behavior (including error-message strings) mirrors the Java source exactly.
//!
//! # Grammar
//! ```text
//! expression = term (('+' | '-') term)*
//! term       = power (('*' | '/' | '%') power)*
//! power      = unary ('^' power)?          // right-associative
//! unary      = '-' unary | primary
//! primary    = NUMBER | VARIABLE | FUNCTION '(' expression ')' | '(' expression ')'
//! ```
//!
//! # Semantics
//! * Trigonometric functions accept **degrees** and convert internally.
//! * Division or modulo by zero surface as [`ExpressionError::DivisionByZero`]
//!   (instead of IEEE ±Inf / NaN leaking to the caller).
//! * Transcendentals (`sqrt`, `log`, `log10`) that leave their real domain
//!   surface as [`ExpressionError::DomainError`].
//! * Unary minus is a prefix operator — number literals never carry a sign.

use std::collections::HashMap;
use std::f64::consts::PI;

/// Public error type returned when parsing or evaluation fails.
///
/// `Display` output matches the Java `IllegalArgumentException` messages verbatim.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExpressionError {
    /// Input was `null`/blank (whitespace-only).
    #[error("Expression must not be null or blank")]
    Empty,
    /// Encountered an unexpected character at the given byte position.
    #[error("Unexpected character at position {pos}: '{ch}'")]
    UnexpectedChar {
        /// Byte position (after whitespace stripping) where the character occurred.
        pos: usize,
        /// The offending character.
        ch: char,
    },
    /// Input ended while a primary expression was expected.
    #[error("Unexpected end of expression")]
    UnexpectedEnd,
    /// A number token failed to parse as `f64`.
    #[error("Invalid number: {0}")]
    InvalidNumber(String),
    /// Identifier used as a variable was not present in the variable map.
    #[error("Unknown variable: {0}")]
    UnknownVariable(String),
    /// Identifier followed by `(` was not a known built-in function.
    #[error("Unknown function: {0}")]
    UnknownFunction(String),
    /// Missing closing parenthesis at the given position.
    #[error("Expected ')' at position {pos}")]
    ExpectedCloseParen {
        /// Position where `)` was expected.
        pos: usize,
    },
    /// Division or modulo by exact zero.
    #[error("Division by zero")]
    DivisionByZero,
    /// A transcendental evaluated outside its real-valued domain (e.g.
    /// `sqrt(-1)`, `log(0)`, `log(-x)`, `tan(90)`).
    #[error("Domain error in {op}: value={value}")]
    DomainError {
        /// Operation name (`sqrt`, `log`, `log10`, `pow`, `tan`, …).
        op: String,
        /// String representation of the offending input.
        value: String,
    },
    /// Arithmetic result overflowed f64 range (produced ±Inf). The f64
    /// backend returns this instead of leaking IEEE-754 infinities to callers.
    #[error("Arithmetic overflow in {op}")]
    Overflow {
        /// Operator that produced the non-finite result (`+`, `-`, `*`, `/`, `^`).
        op: String,
    },
}

/// Evaluates a mathematical expression without variables.
///
/// # Errors
/// Returns [`ExpressionError`] if the expression is blank, malformed, references
/// an unknown variable, or calls an unknown function.
///
/// # Examples
/// ```
/// # use math_calc::engine::expression::evaluate;
/// assert!((evaluate("2 + 3 * 4").unwrap() - 14.0).abs() < 1e-12);
/// ```
pub fn evaluate(expression: &str) -> Result<f64, ExpressionError> {
    evaluate_with_variables(expression, &HashMap::new())
}

/// Evaluates a mathematical expression with variable substitution.
///
/// # Errors
/// Returns [`ExpressionError`] if the expression is blank, malformed, references
/// an unknown variable, or calls an unknown function.
///
/// # Examples
/// ```
/// # use std::collections::HashMap;
/// # use math_calc::engine::expression::evaluate_with_variables;
/// let mut vars = HashMap::new();
/// vars.insert("x".to_string(), 5.0);
/// assert!((evaluate_with_variables("x^2", &vars).unwrap() - 25.0).abs() < 1e-12);
/// ```
pub fn evaluate_with_variables(
    expression: &str,
    variables: &HashMap<String, f64>,
) -> Result<f64, ExpressionError> {
    if expression.trim().is_empty() {
        return Err(ExpressionError::Empty);
    }
    let mut parser = Parser::new(expression, variables);
    let result = parser.parse_expression()?;
    parser.skip_whitespace();
    if let Some(ch) = parser.current_char() {
        return Err(ExpressionError::UnexpectedChar {
            pos: parser.pos,
            ch,
        });
    }
    Ok(result)
}

// --------------------------------------------------------------------------- //
//  Recursive-descent parser
// --------------------------------------------------------------------------- //

struct Parser<'a> {
    /// Raw input preserved verbatim — whitespace is skipped on demand at
    /// token boundaries (see `skip_whitespace`). This avoids collapsing
    /// adjacent numbers: `"1 2"` must be rejected, not read as `12`.
    input: Vec<char>,
    variables: &'a HashMap<String, f64>,
    pos: usize,
    /// Open `(` counter — see the equivalent field in the exact parser for
    /// the parse-error-priority rationale (`((bad` → ExpectedCloseParen).
    paren_depth: u32,
}

impl<'a> Parser<'a> {
    fn new(input: &str, variables: &'a HashMap<String, f64>) -> Self {
        Self {
            input: input.chars().collect(),
            variables,
            pos: 0,
            paren_depth: 0,
        }
    }

    fn current_char(&self) -> Option<char> {
        self.input.get(self.pos).copied()
    }

    /// Advance past any whitespace at the current position. Called at every
    /// token boundary so that whitespace only separates tokens and never
    /// fuses them.
    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.current_char() {
            if ch.is_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn check_finite(value: f64, op: &str) -> Result<f64, ExpressionError> {
        if value.is_finite() {
            Ok(value)
        } else {
            Err(ExpressionError::Overflow { op: op.to_string() })
        }
    }

    // ---- expression = term (('+' | '-') term)* ---- //
    fn parse_expression(&mut self) -> Result<f64, ExpressionError> {
        let mut result = self.parse_term()?;
        loop {
            self.skip_whitespace();
            match self.current_char() {
                Some('+') => {
                    self.pos += 1;
                    result = Self::check_finite(result + self.parse_term()?, "+")?;
                }
                Some('-') => {
                    self.pos += 1;
                    result = Self::check_finite(result - self.parse_term()?, "-")?;
                }
                _ => break,
            }
        }
        Ok(result)
    }

    // ---- term = power (('*' | '/' | '%') power)* ---- //
    fn parse_term(&mut self) -> Result<f64, ExpressionError> {
        let mut result = self.parse_power()?;
        loop {
            self.skip_whitespace();
            match self.current_char() {
                Some('*') => {
                    self.pos += 1;
                    result = Self::check_finite(result * self.parse_power()?, "*")?;
                }
                Some('/') => {
                    self.pos += 1;
                    let rhs = self.parse_power()?;
                    if rhs == 0.0 {
                        return Err(ExpressionError::DivisionByZero);
                    }
                    result = Self::check_finite(result / rhs, "/")?;
                }
                Some('%') => {
                    self.pos += 1;
                    let rhs = self.parse_power()?;
                    if rhs == 0.0 {
                        return Err(ExpressionError::DivisionByZero);
                    }
                    result = Self::check_finite(result % rhs, "%")?;
                }
                _ => break,
            }
        }
        Ok(result)
    }

    // ---- power = unary ('^' power)?  (right-associative) ---- //
    fn parse_power(&mut self) -> Result<f64, ExpressionError> {
        let base = self.parse_unary()?;
        self.skip_whitespace();
        if self.current_char() == Some('^') {
            self.pos += 1;
            let exponent = self.parse_power()?;
            Self::check_finite(base.powf(exponent), "^")
        } else {
            Ok(base)
        }
    }

    // ---- unary = '-' unary | primary ---- //
    fn parse_unary(&mut self) -> Result<f64, ExpressionError> {
        self.skip_whitespace();
        if self.current_char() == Some('-') {
            self.pos += 1;
            let value = self.parse_unary()?;
            Ok(-value)
        } else {
            self.parse_primary()
        }
    }

    // ---- primary = NUMBER | VARIABLE | FUNCTION '(' expr ')' | '(' expr ')' ---- //
    fn parse_primary(&mut self) -> Result<f64, ExpressionError> {
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

    // ---- number parsing (decimal + optional exponent) ---- //
    fn parse_number(&mut self) -> Result<f64, ExpressionError> {
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
        token
            .parse::<f64>()
            .map_err(|_| ExpressionError::InvalidNumber(token))
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

    // ---- identifier parsing (function call or variable) ---- //
    fn parse_identifier(&mut self) -> Result<f64, ExpressionError> {
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
            call_function(&name, argument)
        } else if let Some(value) = self.variables.get(&name) {
            Ok(*value)
        } else if self.paren_depth > 0 && self.current_char().is_none() {
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
}

fn call_function(name: &str, arg: f64) -> Result<f64, ExpressionError> {
    let domain_err = |op: &str| ExpressionError::DomainError {
        op: op.to_string(),
        value: format_arg(arg),
    };
    match name {
        "sin" => Ok((arg * PI / 180.0).sin()),
        "cos" => Ok((arg * PI / 180.0).cos()),
        "tan" => {
            let value = (arg * PI / 180.0).tan();
            if !value.is_finite() {
                return Err(domain_err("tan"));
            }
            Ok(value)
        }
        "log" => {
            if arg <= 0.0 {
                return Err(domain_err("log"));
            }
            Ok(arg.ln())
        }
        "log10" => {
            if arg <= 0.0 {
                return Err(domain_err("log10"));
            }
            Ok(arg.log10())
        }
        "sqrt" => {
            if arg < 0.0 {
                return Err(domain_err("sqrt"));
            }
            Ok(arg.sqrt())
        }
        "abs" => Ok(arg.abs()),
        "ceil" => Ok(arg.ceil()),
        "floor" => Ok(arg.floor()),
        _ => Err(ExpressionError::UnknownFunction(name.to_string())),
    }
}

/// Render `value` for DETAIL output — strips the trailing `.0` that Rust's
/// default float formatter appends to integer-valued doubles, while falling
/// back to `Display` for fractional or out-of-`i64`-range inputs.
fn format_arg(value: f64) -> String {
    const I64_MIN_F: f64 = i64::MIN as f64;
    const I64_MAX_F: f64 = i64::MAX as f64;
    if value.is_finite()
        && value.fract() == 0.0
        && (I64_MIN_F..=I64_MAX_F).contains(&value)
    {
        #[allow(clippy::cast_possible_truncation)]
        {
            return format!("{}", value as i64);
        }
    }
    format!("{value}")
}

// --------------------------------------------------------------------------- //
//  Tests
// --------------------------------------------------------------------------- //

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = 1e-10;

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < EPS,
            "expected {expected}, got {actual}"
        );
    }

    // ---- basic arithmetic & precedence ----

    #[test]
    fn simple_addition() {
        assert_close(evaluate("1 + 2").unwrap(), 3.0);
    }

    #[test]
    fn simple_subtraction() {
        assert_close(evaluate("10 - 4").unwrap(), 6.0);
    }

    #[test]
    fn multiplication_precedence() {
        assert_close(evaluate("2 + 3 * 4").unwrap(), 14.0);
    }

    #[test]
    fn division_precedence() {
        assert_close(evaluate("20 / 4 + 1").unwrap(), 6.0);
    }

    #[test]
    fn modulo_operator() {
        assert_close(evaluate("10 % 3").unwrap(), 1.0);
    }

    #[test]
    fn left_to_right_same_precedence() {
        assert_close(evaluate("100 / 10 / 2").unwrap(), 5.0);
        assert_close(evaluate("10 - 3 - 2").unwrap(), 5.0);
    }

    #[test]
    fn parenthesized_grouping() {
        assert_close(evaluate("(2 + 3) * 4").unwrap(), 20.0);
    }

    #[test]
    fn nested_parentheses() {
        assert_close(evaluate("((1 + 2) * (3 + 4))").unwrap(), 21.0);
    }

    // ---- power ----

    #[test]
    fn power_basic() {
        assert_close(evaluate("2 ^ 10").unwrap(), 1024.0);
    }

    #[test]
    fn power_right_associative() {
        // 2^(3^2) = 2^9 = 512, NOT (2^3)^2 = 64
        assert_close(evaluate("2^3^2").unwrap(), 512.0);
    }

    #[test]
    fn power_binds_tighter_than_unary_minus() {
        // Per Java grammar: unary applies to full power, so -2^2 parses as -(2^2) = -4
        // Wait — actually: unary = '-' unary | primary, so -2^2 is -(unary) where
        // unary = primary = 2, then ^ is NOT consumed (power wraps unary). Tracing:
        //   parse_power -> parse_unary -> '-' then parse_unary -> primary = 2 -> returns 2
        //   back in outer parse_power: base = -2, sees '^', exponent = parse_power -> 2
        //   => (-2)^2 = 4
        assert_close(evaluate("-2^2").unwrap(), 4.0);
    }

    // ---- unary minus ----

    #[test]
    fn unary_minus_prefix() {
        assert_close(evaluate("-5 + 3").unwrap(), -2.0);
    }

    #[test]
    fn double_unary_minus() {
        assert_close(evaluate("--5").unwrap(), 5.0);
    }

    #[test]
    fn unary_minus_inside_parens() {
        assert_close(evaluate("3 * (-4)").unwrap(), -12.0);
    }

    // ---- numbers (decimal + scientific) ----

    #[test]
    fn decimal_number() {
        #[allow(clippy::approx_constant)]
        let expected = 6.28;
        assert_close(evaluate("3.14 * 2").unwrap(), expected);
    }

    #[test]
    fn leading_dot_number() {
        assert_close(evaluate(".5 + .25").unwrap(), 0.75);
    }

    #[test]
    fn scientific_notation_lowercase() {
        assert_close(evaluate("1.5e2").unwrap(), 150.0);
    }

    #[test]
    fn scientific_notation_uppercase() {
        assert_close(evaluate("2E10").unwrap(), 2e10);
    }

    #[test]
    fn scientific_notation_negative_exponent() {
        assert_close(evaluate("1.5e-3").unwrap(), 0.0015);
    }

    #[test]
    fn scientific_notation_positive_exponent_sign() {
        assert_close(evaluate("2.5e+2").unwrap(), 250.0);
    }

    // ---- functions ----

    #[test]
    fn fn_sin_degrees() {
        assert_close(evaluate("sin(0)").unwrap(), 0.0);
        assert_close(evaluate("sin(90)").unwrap(), 1.0);
        assert_close(evaluate("sin(180)").unwrap(), 0.0);
    }

    #[test]
    fn fn_cos_degrees() {
        assert_close(evaluate("cos(0)").unwrap(), 1.0);
        assert_close(evaluate("cos(90)").unwrap(), 0.0);
        assert_close(evaluate("cos(180)").unwrap(), -1.0);
    }

    #[test]
    fn fn_tan_degrees() {
        assert_close(evaluate("tan(0)").unwrap(), 0.0);
        assert_close(evaluate("tan(45)").unwrap(), 1.0);
    }

    #[test]
    fn fn_log_natural() {
        assert_close(evaluate("log(2.718281828459045)").unwrap(), 1.0);
        assert_close(evaluate("log(1)").unwrap(), 0.0);
    }

    #[test]
    fn fn_log10() {
        assert_close(evaluate("log10(1000)").unwrap(), 3.0);
        assert_close(evaluate("log10(1)").unwrap(), 0.0);
    }

    #[test]
    fn fn_sqrt() {
        assert_close(evaluate("sqrt(144)").unwrap(), 12.0);
        assert_close(evaluate("sqrt(2)").unwrap(), std::f64::consts::SQRT_2);
    }

    #[test]
    fn fn_abs() {
        assert_close(evaluate("abs(-7.5)").unwrap(), 7.5);
        assert_close(evaluate("abs(3)").unwrap(), 3.0);
    }

    #[test]
    fn fn_ceil() {
        assert_close(evaluate("ceil(2.1)").unwrap(), 3.0);
        assert_close(evaluate("ceil(-2.1)").unwrap(), -2.0);
    }

    #[test]
    fn fn_floor() {
        assert_close(evaluate("floor(2.9)").unwrap(), 2.0);
        assert_close(evaluate("floor(-2.1)").unwrap(), -3.0);
    }

    #[test]
    fn function_composition() {
        assert_close(evaluate("sqrt(abs(-16))").unwrap(), 4.0);
    }

    #[test]
    fn function_in_expression() {
        assert_close(evaluate("2 * sin(30) + 1").unwrap(), 2.0);
    }

    // ---- variables ----

    #[test]
    fn variable_lookup() {
        let mut vars = HashMap::new();
        vars.insert("x".to_string(), 5.0);
        vars.insert("y".to_string(), 3.0);
        assert_close(evaluate_with_variables("x^2 + y^2", &vars).unwrap(), 34.0);
    }

    #[test]
    fn variable_with_underscore_and_digits() {
        let mut vars = HashMap::new();
        vars.insert("_var1".to_string(), 10.0);
        vars.insert("a_b2".to_string(), 2.0);
        assert_close(
            evaluate_with_variables("_var1 * a_b2", &vars).unwrap(),
            20.0,
        );
    }

    // ---- whitespace ----

    #[test]
    fn whitespace_is_stripped() {
        assert_close(evaluate("  1\t+\n2  ").unwrap(), 3.0);
    }

    // ---- Division / modulo by zero ----

    #[test]
    fn division_by_zero_is_error() {
        assert!(matches!(
            evaluate("1 / 0").unwrap_err(),
            ExpressionError::DivisionByZero
        ));
        assert!(matches!(
            evaluate("-1 / 0").unwrap_err(),
            ExpressionError::DivisionByZero
        ));
    }

    #[test]
    fn modulo_by_zero_is_error() {
        assert!(matches!(
            evaluate("1 % 0").unwrap_err(),
            ExpressionError::DivisionByZero
        ));
    }

    // ---- Transcendental domain errors ----

    #[test]
    fn sqrt_of_negative_is_domain_error() {
        match evaluate("sqrt(-9)").unwrap_err() {
            ExpressionError::DomainError { op, value } => {
                assert_eq!(op, "sqrt");
                assert_eq!(value, "-9");
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

    // ---- errors (exact message matching) ----

    #[test]
    fn err_empty_input() {
        let err = evaluate("").unwrap_err();
        assert_eq!(err.to_string(), "Expression must not be null or blank");
        assert!(matches!(err, ExpressionError::Empty));
    }

    #[test]
    fn err_blank_input() {
        let err = evaluate("   \t \n ").unwrap_err();
        assert_eq!(err.to_string(), "Expression must not be null or blank");
    }

    #[test]
    fn err_unexpected_char_trailing() {
        let err = evaluate("1+2$").unwrap_err();
        assert_eq!(err.to_string(), "Unexpected character at position 3: '$'");
    }

    #[test]
    fn err_unexpected_char_leading() {
        let err = evaluate("@1").unwrap_err();
        assert_eq!(err.to_string(), "Unexpected character at position 0: '@'");
    }

    #[test]
    fn err_unexpected_end() {
        let err = evaluate("1+").unwrap_err();
        assert_eq!(err.to_string(), "Unexpected end of expression");
        assert!(matches!(err, ExpressionError::UnexpectedEnd));
    }

    #[test]
    fn err_invalid_number() {
        // A bare '.' with no digits around it fails f64::parse.
        let err = evaluate(".").unwrap_err();
        assert_eq!(err.to_string(), "Invalid number: .");
    }

    #[test]
    fn err_unknown_variable() {
        let err = evaluate("foo + 1").unwrap_err();
        assert_eq!(err.to_string(), "Unknown variable: foo");
    }

    #[test]
    fn err_unknown_function() {
        let err = evaluate("bogus(1)").unwrap_err();
        assert_eq!(err.to_string(), "Unknown function: bogus");
    }

    #[test]
    fn err_expected_close_paren() {
        // Whitespace is preserved in positions now — the 6-char input
        // bottoms out past the last byte at position 6.
        let err = evaluate("(1 + 2").unwrap_err();
        assert_eq!(err.to_string(), "Expected ')' at position 6");
    }

    #[test]
    fn err_expected_close_paren_in_function() {
        // Whitespace stripped: "sqrt(4" — length 6, position 6 is past end.
        let err = evaluate("sqrt(4").unwrap_err();
        assert_eq!(err.to_string(), "Expected ')' at position 6");
    }

    #[test]
    fn err_unclosed_paren_wins_over_unknown_variable() {
        // Regression: `((bad` used to surface as `UnknownVariable("bad")`
        // because the identifier parser fired before the outer `)`-expect.
        let err = evaluate("((bad").unwrap_err();
        assert!(
            matches!(err, ExpressionError::ExpectedCloseParen { .. }),
            "got {err:?}"
        );
    }

    // ---- Overflow guard: IEEE ±Inf must not leak ----

    #[test]
    fn multiplication_overflow_is_error() {
        match evaluate("1e308 * 1e308").unwrap_err() {
            ExpressionError::Overflow { op } => assert_eq!(op, "*"),
            other => panic!("expected Overflow, got {other:?}"),
        }
    }

    #[test]
    fn addition_overflow_is_error() {
        match evaluate("1e308 + 1e308").unwrap_err() {
            ExpressionError::Overflow { op } => assert_eq!(op, "+"),
            other => panic!("expected Overflow, got {other:?}"),
        }
    }

    #[test]
    fn negative_overflow_is_error() {
        match evaluate("-1e308 * 1e308").unwrap_err() {
            ExpressionError::Overflow { op } => assert_eq!(op, "*"),
            other => panic!("expected Overflow, got {other:?}"),
        }
    }

    #[test]
    fn power_overflow_is_error() {
        // 10^400 vastly exceeds f64::MAX — powf returns +Inf.
        match evaluate("10 ^ 400").unwrap_err() {
            ExpressionError::Overflow { op } => assert_eq!(op, "^"),
            other => panic!("expected Overflow, got {other:?}"),
        }
    }

    // ---- Whitespace no longer fuses adjacent numbers ----

    #[test]
    fn adjacent_numbers_reject() {
        // Previously: `"1 2 3"` collapsed to `123`. Now it must fail at
        // position 2, where the second operand appears without an operator.
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
    fn adjacent_numbers_partial_expression() {
        // `"1 + 2 3"` used to parse as `1 + 23 = 24`; now the trailing `3`
        // is surfaced as an unexpected char.
        let err = evaluate("1 + 2 3").unwrap_err();
        match err {
            ExpressionError::UnexpectedChar { pos, ch } => {
                assert_eq!(pos, 6);
                assert_eq!(ch, '3');
            }
            other => panic!("expected UnexpectedChar, got {other:?}"),
        }
    }

    #[test]
    fn whitespace_inside_number_rejected() {
        // `"10 20"` must not fuse into `1020`.
        let err = evaluate("10 20").unwrap_err();
        match err {
            ExpressionError::UnexpectedChar { pos, ch } => {
                assert_eq!(pos, 3);
                assert_eq!(ch, '2');
            }
            other => panic!("expected UnexpectedChar, got {other:?}"),
        }
    }
}
