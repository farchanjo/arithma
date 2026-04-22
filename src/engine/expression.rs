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
//! * Division by zero follows IEEE-754 (`±Inf`); modulo by zero yields `NaN`.
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
    /// Whitespace-stripped input as a `Vec<char>` so indexing is O(1) regardless of
    /// UTF-8 byte width — keeps position semantics identical to Java's `charAt`.
    input: Vec<char>,
    variables: &'a HashMap<String, f64>,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &str, variables: &'a HashMap<String, f64>) -> Self {
        let stripped: Vec<char> = input.chars().filter(|c| !c.is_whitespace()).collect();
        Self {
            input: stripped,
            variables,
            pos: 0,
        }
    }

    fn current_char(&self) -> Option<char> {
        self.input.get(self.pos).copied()
    }

    // ---- expression = term (('+' | '-') term)* ---- //
    fn parse_expression(&mut self) -> Result<f64, ExpressionError> {
        let mut result = self.parse_term()?;
        while let Some(ch) = self.current_char() {
            match ch {
                '+' => {
                    self.pos += 1;
                    result += self.parse_term()?;
                }
                '-' => {
                    self.pos += 1;
                    result -= self.parse_term()?;
                }
                _ => break,
            }
        }
        Ok(result)
    }

    // ---- term = power (('*' | '/' | '%') power)* ---- //
    fn parse_term(&mut self) -> Result<f64, ExpressionError> {
        let mut result = self.parse_power()?;
        while let Some(ch) = self.current_char() {
            match ch {
                '*' => {
                    self.pos += 1;
                    result *= self.parse_power()?;
                }
                '/' => {
                    self.pos += 1;
                    result /= self.parse_power()?;
                }
                '%' => {
                    self.pos += 1;
                    result %= self.parse_power()?;
                }
                _ => break,
            }
        }
        Ok(result)
    }

    // ---- power = unary ('^' power)?  (right-associative) ---- //
    fn parse_power(&mut self) -> Result<f64, ExpressionError> {
        let base = self.parse_unary()?;
        if self.current_char() == Some('^') {
            self.pos += 1;
            let exponent = self.parse_power()?;
            Ok(base.powf(exponent))
        } else {
            Ok(base)
        }
    }

    // ---- unary = '-' unary | primary ---- //
    fn parse_unary(&mut self) -> Result<f64, ExpressionError> {
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
        let ch = self.current_char().ok_or(ExpressionError::UnexpectedEnd)?;
        if ch == '(' {
            self.pos += 1;
            let value = self.parse_expression()?;
            self.expect_close_paren()?;
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

        if self.current_char() == Some('(') {
            self.pos += 1;
            let argument = self.parse_expression()?;
            self.expect_close_paren()?;
            call_function(&name, argument)
        } else if let Some(value) = self.variables.get(&name) {
            Ok(*value)
        } else {
            Err(ExpressionError::UnknownVariable(name))
        }
    }

    fn expect_close_paren(&mut self) -> Result<(), ExpressionError> {
        if self.current_char() != Some(')') {
            return Err(ExpressionError::ExpectedCloseParen { pos: self.pos });
        }
        self.pos += 1;
        Ok(())
    }
}

fn call_function(name: &str, arg: f64) -> Result<f64, ExpressionError> {
    match name {
        "sin" => Ok((arg * PI / 180.0).sin()),
        "cos" => Ok((arg * PI / 180.0).cos()),
        "tan" => Ok((arg * PI / 180.0).tan()),
        "log" => Ok(arg.ln()),
        "log10" => Ok(arg.log10()),
        "sqrt" => Ok(arg.sqrt()),
        "abs" => Ok(arg.abs()),
        "ceil" => Ok(arg.ceil()),
        "floor" => Ok(arg.floor()),
        _ => Err(ExpressionError::UnknownFunction(name.to_string())),
    }
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

    // ---- IEEE-754 division / modulo ----

    #[test]
    fn division_by_zero_is_infinity() {
        assert!(evaluate("1 / 0").unwrap().is_infinite());
        assert!(evaluate("-1 / 0").unwrap().is_infinite());
        assert!(evaluate("1 / 0").unwrap().is_sign_positive());
        assert!(evaluate("-1 / 0").unwrap().is_sign_negative());
    }

    #[test]
    fn modulo_by_zero_is_nan() {
        assert!(evaluate("1 % 0").unwrap().is_nan());
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
        // Whitespace stripped: "(1+2" — position 4 is past the last char.
        let err = evaluate("(1 + 2").unwrap_err();
        assert_eq!(err.to_string(), "Expected ')' at position 4");
    }

    #[test]
    fn err_expected_close_paren_in_function() {
        // Whitespace stripped: "sqrt(4" — length 6, position 6 is past end.
        let err = evaluate("sqrt(4").unwrap_err();
        assert_eq!(err.to_string(), "Expected ')' at position 6");
    }
}
