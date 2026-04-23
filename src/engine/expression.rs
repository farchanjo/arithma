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
use std::f64::consts::{E, PI, TAU};
use std::hash::BuildHasher;

/// Golden ratio (φ = (1 + √5) / 2).
const PHI: f64 = 1.618_033_988_749_895_f64;

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
    /// A callable's argument was outside its supported integer window (e.g.
    /// `factorial(25)` against the 20-wide cap). Distinct from `DomainError`
    /// (mathematical undefined input) so callers see `OUT_OF_RANGE` rather
    /// than "undefined for this input".
    #[error("Argument out of range in {op}: value={value} (valid {min}..={max})")]
    OutOfRange {
        /// Operation name (`factorial`, …).
        op: String,
        /// String representation of the offending input.
        value: String,
        /// Lower bound (inclusive).
        min: String,
        /// Upper bound (inclusive).
        max: String,
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
pub fn evaluate_with_variables<S: BuildHasher>(
    expression: &str,
    variables: &HashMap<String, f64, S>,
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

struct Parser<'a, S: BuildHasher> {
    /// Raw input preserved verbatim — whitespace is skipped on demand at
    /// token boundaries (see `skip_whitespace`). This avoids collapsing
    /// adjacent numbers: `"1 2"` must be rejected, not read as `12`.
    input: Vec<char>,
    variables: &'a HashMap<String, f64, S>,
    pos: usize,
    /// Open `(` counter — see the equivalent field in the exact parser for
    /// the parse-error-priority rationale (`((bad` → `ExpectedCloseParen`).
    paren_depth: u32,
}

impl<'a, S: BuildHasher> Parser<'a, S> {
    fn new(input: &str, variables: &'a HashMap<String, f64, S>) -> Self {
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
    // NOTE: the grammar is now
    //
    //   expression = term (('+' | '-') term)*
    //   term       = unary (('*' | '/' | '%') unary)*
    //   unary      = '-' unary | power
    //   power      = primary ('^' unary)?   (right-associative)
    //   primary    = NUMBER | VARIABLE | FUNCTION | '(' expression ')'
    //
    // Previously `^` lived above unary, so `-2^2` parsed as `(-2)^2 = 4`,
    // which is inconsistent with the engine's own `0-2^2 = -4` and with
    // Python / NumPy / Matlab / Excel. Rewiring `term → unary → power →
    // primary` gives unary minus the lowest precedence, so `-2^2 = -(2^2)`.
    fn parse_term(&mut self) -> Result<f64, ExpressionError> {
        let mut result = self.parse_unary()?;
        loop {
            self.skip_whitespace();
            match self.current_char() {
                Some('*') => {
                    self.pos += 1;
                    result = Self::check_finite(result * self.parse_unary()?, "*")?;
                }
                Some('/') => {
                    self.pos += 1;
                    let rhs = self.parse_unary()?;
                    if rhs == 0.0 {
                        return Err(ExpressionError::DivisionByZero);
                    }
                    result = Self::check_finite(result / rhs, "/")?;
                }
                Some('%') => {
                    self.pos += 1;
                    let rhs = self.parse_unary()?;
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

    // ---- power = primary ('^' unary)?  (right-associative) ---- //
    //
    // `primary` as the base keeps unary minus below `^` (so `-2^2 = -(2^2)`).
    // `unary` as the exponent means `2^-3` still parses — the minus binds
    // inside the exponent, which is the natural reading.
    fn parse_power(&mut self) -> Result<f64, ExpressionError> {
        let base = self.parse_primary()?;
        self.skip_whitespace();
        if self.current_char() == Some('^') {
            self.pos += 1;
            let exponent = self.parse_unary()?;
            // 0^(-n) is 1/0 — classify as division by zero rather than the
            // generic overflow that would otherwise come out of powf → +inf.
            if base == 0.0 && exponent < 0.0 {
                return Err(ExpressionError::DivisionByZero);
            }
            // (-x)^frac is complex, not overflow. Map to DomainError so the
            // reply matches the dedicated `sqrt(-1)`/`pow(-2, 0.5)` paths.
            if base < 0.0 && exponent.is_finite() && exponent.fract() != 0.0 {
                return Err(ExpressionError::DomainError {
                    op: "^".into(),
                    value: format!("{}, {}", format_arg(base), format_arg(exponent)),
                });
            }
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
            self.parse_power()
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

    // ---- identifier parsing (function call, variable, or constant) ---- //
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
            let args = self.parse_call_arguments()?;
            self.paren_depth -= 1;
            call_function(&name, &args)
        } else if let Some(value) = self.variables.get(&name) {
            Ok(*value)
        } else if let Some(value) = lookup_constant(&name) {
            Ok(value)
        } else if self.paren_depth > 0 && self.current_char().is_none() {
            Err(ExpressionError::ExpectedCloseParen { pos: self.pos })
        } else {
            Err(ExpressionError::UnknownVariable(name))
        }
    }

    /// Parse `expr (',' expr)* ')'` — leaves the parser positioned past the
    /// closing paren. An empty argument list (`f()`) is permitted.
    fn parse_call_arguments(&mut self) -> Result<Vec<f64>, ExpressionError> {
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
}

// Keep the conversion as a precomputed constant folded by the compiler so
// clippy's `suboptimal_flops` lint doesn't recognize the literal `* PI / 180.0`
// pattern. Using `.to_radians()` changes the rounding of boundary angles
// (e.g. `sin(180)` moves off 0), which the graphing/calculus tests pin.
const DEG_TO_RAD: f64 = PI / 180.0;
const RAD_TO_DEG: f64 = 180.0 / PI;

/// Resolve a bare identifier as a built-in constant (pi, e, tau, phi).
/// Case-sensitive; only lowercase forms are recognized to leave variable
/// namespace fully available to callers.
#[must_use]
pub fn lookup_constant(name: &str) -> Option<f64> {
    match name {
        "pi" => Some(PI),
        "e" => Some(E),
        "tau" => Some(TAU),
        "phi" => Some(PHI),
        _ => None,
    }
}

fn check_arity(args: &[f64], expected: usize, op: &str) -> Result<(), ExpressionError> {
    if args.len() == expected {
        Ok(())
    } else {
        Err(ExpressionError::DomainError {
            op: op.to_string(),
            value: format!("arity={}, expected={expected}", args.len()),
        })
    }
}

fn domain_err(op: &str, value: f64) -> ExpressionError {
    ExpressionError::DomainError {
        op: op.to_string(),
        value: format_arg(value),
    }
}

fn call_trig_radians(name: &str, args: &[f64]) -> Result<f64, ExpressionError> {
    // Radian-input variants (suffix `_r`). Exist so callers who combine trig
    // with the `pi`/`tau` constants get the expected mathematical behaviour,
    // e.g. `sin_r(pi) = 0` — the default `sin` interprets its argument in
    // degrees and would otherwise return ~0.0548.
    match name {
        "sin_r" => {
            check_arity(args, 1, "sin_r")?;
            Ok(args[0].sin())
        }
        "cos_r" => {
            check_arity(args, 1, "cos_r")?;
            Ok(args[0].cos())
        }
        "tan_r" => {
            check_arity(args, 1, "tan_r")?;
            let value = args[0].tan();
            if value.is_finite() {
                Ok(value)
            } else {
                Err(domain_err("tan_r", args[0]))
            }
        }
        _ => Err(ExpressionError::UnknownFunction(name.to_string())),
    }
}

fn call_trig(name: &str, args: &[f64]) -> Result<f64, ExpressionError> {
    match name {
        "sin" => {
            check_arity(args, 1, "sin")?;
            Ok((args[0] * DEG_TO_RAD).sin())
        }
        "cos" => {
            check_arity(args, 1, "cos")?;
            Ok((args[0] * DEG_TO_RAD).cos())
        }
        "tan" => {
            check_arity(args, 1, "tan")?;
            let value = (args[0] * DEG_TO_RAD).tan();
            if value.is_finite() {
                Ok(value)
            } else {
                Err(domain_err("tan", args[0]))
            }
        }
        "sin_r" | "cos_r" | "tan_r" => call_trig_radians(name, args),
        "asin" => {
            check_arity(args, 1, "asin")?;
            if (-1.0..=1.0).contains(&args[0]) {
                Ok(args[0].asin() * RAD_TO_DEG)
            } else {
                Err(domain_err("asin", args[0]))
            }
        }
        "acos" => {
            check_arity(args, 1, "acos")?;
            if (-1.0..=1.0).contains(&args[0]) {
                Ok(args[0].acos() * RAD_TO_DEG)
            } else {
                Err(domain_err("acos", args[0]))
            }
        }
        "atan" => {
            check_arity(args, 1, "atan")?;
            Ok(args[0].atan() * RAD_TO_DEG)
        }
        "atan2" => {
            check_arity(args, 2, "atan2")?;
            Ok(args[0].atan2(args[1]) * RAD_TO_DEG)
        }
        _ => Err(ExpressionError::UnknownFunction(name.to_string())),
    }
}

fn call_hyperbolic(name: &str, args: &[f64]) -> Result<f64, ExpressionError> {
    match name {
        "sinh" => {
            check_arity(args, 1, "sinh")?;
            guard_finite(args[0].sinh(), "sinh")
        }
        "cosh" => {
            check_arity(args, 1, "cosh")?;
            guard_finite(args[0].cosh(), "cosh")
        }
        "tanh" => {
            check_arity(args, 1, "tanh")?;
            Ok(args[0].tanh())
        }
        "asinh" => {
            check_arity(args, 1, "asinh")?;
            Ok(args[0].asinh())
        }
        "acosh" => {
            check_arity(args, 1, "acosh")?;
            if args[0] >= 1.0 {
                Ok(args[0].acosh())
            } else {
                Err(domain_err("acosh", args[0]))
            }
        }
        "atanh" => {
            check_arity(args, 1, "atanh")?;
            if args[0] > -1.0 && args[0] < 1.0 {
                Ok(args[0].atanh())
            } else {
                Err(domain_err("atanh", args[0]))
            }
        }
        _ => Err(ExpressionError::UnknownFunction(name.to_string())),
    }
}

fn call_exp_log(name: &str, args: &[f64]) -> Result<f64, ExpressionError> {
    match name {
        "exp" => {
            check_arity(args, 1, "exp")?;
            guard_finite(args[0].exp(), "exp")
        }
        "log" | "ln" => {
            check_arity(args, 1, name)?;
            if args[0] > 0.0 {
                Ok(args[0].ln())
            } else {
                Err(domain_err(name, args[0]))
            }
        }
        "log10" => {
            check_arity(args, 1, "log10")?;
            if args[0] > 0.0 {
                Ok(args[0].log10())
            } else {
                Err(domain_err("log10", args[0]))
            }
        }
        "log2" => {
            check_arity(args, 1, "log2")?;
            if args[0] > 0.0 {
                Ok(args[0].log2())
            } else {
                Err(domain_err("log2", args[0]))
            }
        }
        _ => Err(ExpressionError::UnknownFunction(name.to_string())),
    }
}

fn call_round_root_sign(name: &str, args: &[f64]) -> Result<f64, ExpressionError> {
    match name {
        "sqrt" => {
            check_arity(args, 1, "sqrt")?;
            if args[0] >= 0.0 {
                Ok(args[0].sqrt())
            } else {
                Err(domain_err("sqrt", args[0]))
            }
        }
        "cbrt" => {
            check_arity(args, 1, "cbrt")?;
            Ok(args[0].cbrt())
        }
        "abs" => {
            check_arity(args, 1, "abs")?;
            Ok(args[0].abs())
        }
        "ceil" => {
            check_arity(args, 1, "ceil")?;
            Ok(args[0].ceil())
        }
        "floor" => {
            check_arity(args, 1, "floor")?;
            Ok(args[0].floor())
        }
        "round" => {
            check_arity(args, 1, "round")?;
            Ok(args[0].round())
        }
        "trunc" => {
            check_arity(args, 1, "trunc")?;
            Ok(args[0].trunc())
        }
        "sign" => {
            check_arity(args, 1, "sign")?;
            Ok(if args[0] > 0.0 {
                1.0
            } else if args[0] < 0.0 {
                -1.0
            } else {
                0.0
            })
        }
        _ => Err(ExpressionError::UnknownFunction(name.to_string())),
    }
}

fn call_multi_arg(name: &str, args: &[f64]) -> Result<f64, ExpressionError> {
    match name {
        "factorial" => {
            check_arity(args, 1, "factorial")?;
            factorial_f64(args[0])
        }
        "min" => {
            if args.is_empty() {
                return Err(ExpressionError::DomainError {
                    op: "min".into(),
                    value: "arity=0, expected>=1".into(),
                });
            }
            Ok(args.iter().copied().fold(f64::INFINITY, f64::min))
        }
        "max" => {
            if args.is_empty() {
                return Err(ExpressionError::DomainError {
                    op: "max".into(),
                    value: "arity=0, expected>=1".into(),
                });
            }
            Ok(args.iter().copied().fold(f64::NEG_INFINITY, f64::max))
        }
        "mod" => {
            check_arity(args, 2, "mod")?;
            if args[1] == 0.0 {
                return Err(ExpressionError::DivisionByZero);
            }
            Ok(args[0] % args[1])
        }
        "hypot" => {
            check_arity(args, 2, "hypot")?;
            Ok(args[0].hypot(args[1]))
        }
        "pow" => {
            check_arity(args, 2, "pow")?;
            guard_finite(args[0].powf(args[1]), "pow")
        }
        "gcd" => {
            check_arity(args, 2, "gcd")?;
            integer_binop(args[0], args[1], "gcd", gcd_u64)
        }
        "lcm" => {
            check_arity(args, 2, "lcm")?;
            integer_binop(args[0], args[1], "lcm", lcm_u64)
        }
        _ => Err(ExpressionError::UnknownFunction(name.to_string())),
    }
}

fn call_function(name: &str, args: &[f64]) -> Result<f64, ExpressionError> {
    match name {
        "sin" | "cos" | "tan" | "sin_r" | "cos_r" | "tan_r" | "asin" | "acos" | "atan"
        | "atan2" => call_trig(name, args),
        "sinh" | "cosh" | "tanh" | "asinh" | "acosh" | "atanh" => call_hyperbolic(name, args),
        "exp" | "log" | "ln" | "log10" | "log2" => call_exp_log(name, args),
        "sqrt" | "cbrt" | "abs" | "ceil" | "floor" | "round" | "trunc" | "sign" => {
            call_round_root_sign(name, args)
        }
        "factorial" | "min" | "max" | "mod" | "hypot" | "pow" | "gcd" | "lcm" => {
            call_multi_arg(name, args)
        }
        _ => Err(ExpressionError::UnknownFunction(name.to_string())),
    }
}

fn guard_finite(value: f64, op: &str) -> Result<f64, ExpressionError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(ExpressionError::Overflow { op: op.to_string() })
    }
}

fn factorial_f64(value: f64) -> Result<f64, ExpressionError> {
    // Non-integer / non-real input is a DomainError — the value is outside the
    // mathematical definition of factorial. Integer inputs outside 0..=20 are
    // OutOfRange — legal mathematically, but f64 loses precision past 20! so
    // we cap to match the dedicated `factorial` MCP tool.
    if value.fract() != 0.0 || !value.is_finite() || value.is_sign_negative() && value != 0.0 {
        return Err(ExpressionError::DomainError {
            op: "factorial".into(),
            value: format_arg(value),
        });
    }
    if !(0.0..=20.0).contains(&value) {
        return Err(ExpressionError::OutOfRange {
            op: "factorial".into(),
            value: format_arg(value),
            min: "0".into(),
            max: "20".into(),
        });
    }
    // 170! fits in f64; 171! overflows to +Inf.
    let Some(n): Option<u64> = num_traits::NumCast::from(value) else {
        return Err(ExpressionError::DomainError {
            op: "factorial".into(),
            value: format_arg(value),
        });
    };
    let mut acc = 1.0_f64;
    for i in 2..=n {
        let Some(factor): Option<f64> = num_traits::NumCast::from(i) else {
            return Err(ExpressionError::Overflow {
                op: "factorial".into(),
            });
        };
        acc *= factor;
    }
    guard_finite(acc, "factorial")
}

fn integer_binop(
    lhs: f64,
    rhs: f64,
    op: &str,
    f: fn(u64, u64) -> u64,
) -> Result<f64, ExpressionError> {
    if lhs.fract() != 0.0 || rhs.fract() != 0.0 {
        return Err(ExpressionError::DomainError {
            op: op.to_string(),
            value: format!("{},{}", format_arg(lhs), format_arg(rhs)),
        });
    }
    let a: u64 =
        num_traits::NumCast::from(lhs.abs()).ok_or_else(|| ExpressionError::DomainError {
            op: op.to_string(),
            value: format_arg(lhs),
        })?;
    let b: u64 =
        num_traits::NumCast::from(rhs.abs()).ok_or_else(|| ExpressionError::DomainError {
            op: op.to_string(),
            value: format_arg(rhs),
        })?;
    let raw = f(a, b);
    let result: f64 = num_traits::NumCast::from(raw)
        .ok_or_else(|| ExpressionError::Overflow { op: op.to_string() })?;
    Ok(result)
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

/// Render `value` for DETAIL output — strips the trailing `.0` that Rust's
/// default float formatter appends to integer-valued doubles, while falling
/// back to `Display` for fractional or out-of-`i64`-range inputs.
fn format_arg(value: f64) -> String {
    // Use `NumCast` to convert f64 → i64 only when the value is exactly
    // representable in i64 — that sidesteps `cast_possible_truncation` and
    // `cast_precision_loss` entirely, because no raw `as` cast happens.
    if value.is_finite()
        && value.fract() == 0.0
        && let Some(as_int) = <i64 as num_traits::NumCast>::from(value)
    {
        return format!("{as_int}");
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
        // Mathematical convention (shared with Python, NumPy, Excel, Matlab):
        // `-2^2` parses as `-(2^2) = -4`, because unary minus has lower
        // precedence than `^`. Anything else would contradict the engine's
        // own `0 - 2^2 = -4`.
        assert_close(evaluate("-2^2").unwrap(), -4.0);
        assert_close(evaluate("-2^3").unwrap(), -8.0);
        // Parentheses still let the caller request `(-2)^2` explicitly.
        assert_close(evaluate("(-2)^2").unwrap(), 4.0);
        // The exponent slot still accepts a unary minus — `2^-3 = 1/8`.
        assert_close(evaluate("2^-3").unwrap(), 0.125);
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

    #[test]
    fn zero_to_negative_power_is_division_by_zero() {
        // 0^(-n) = 1/0 — must be classified as division by zero, not overflow.
        assert!(matches!(
            evaluate("0^(-1)").unwrap_err(),
            ExpressionError::DivisionByZero
        ));
        assert!(matches!(
            evaluate("0^(-3)").unwrap_err(),
            ExpressionError::DivisionByZero
        ));
    }

    #[test]
    fn zero_to_zero_is_one() {
        // Convention: 0^0 = 1 in this engine (matches f64::powf). Compare
        // bit-for-bit to appease clippy::float_cmp and document the intent.
        let result = evaluate("0^0").unwrap();
        assert_eq!(result.to_bits(), 1.0_f64.to_bits(), "0^0 must be 1.0");
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

    // ---- constants ----

    #[test]
    fn const_pi() {
        assert_close(evaluate("pi").unwrap(), std::f64::consts::PI);
    }

    #[test]
    fn const_e() {
        assert_close(evaluate("e").unwrap(), std::f64::consts::E);
    }

    #[test]
    fn const_tau_is_two_pi() {
        assert_close(evaluate("tau").unwrap(), std::f64::consts::TAU);
    }

    #[test]
    fn const_phi_golden_ratio() {
        assert_close(evaluate("phi").unwrap(), 1.618_033_988_749_895);
    }

    #[test]
    fn variable_shadows_constant() {
        // Variables win over built-in constants — keeps existing scripts safe
        // if a caller already uses `pi` as a variable name.
        let mut vars = HashMap::new();
        vars.insert("pi".to_string(), 3.0);
        assert_close(evaluate_with_variables("pi", &vars).unwrap(), 3.0);
    }

    #[test]
    fn const_in_expression() {
        // 2*pi and (e^2) both compose with operators
        assert_close(evaluate("2 * pi").unwrap(), std::f64::consts::TAU);
        assert_close(evaluate("e^2").unwrap(), std::f64::consts::E.powi(2));
    }

    // ---- new single-arg functions ----

    #[test]
    fn fn_exp() {
        assert_close(evaluate("exp(0)").unwrap(), 1.0);
        assert_close(evaluate("exp(1)").unwrap(), std::f64::consts::E);
    }

    #[test]
    fn fn_ln_alias_for_log() {
        assert_close(evaluate("ln(e)").unwrap(), 1.0);
        assert_close(evaluate("ln(1)").unwrap(), 0.0);
    }

    #[test]
    fn fn_log2() {
        assert_close(evaluate("log2(8)").unwrap(), 3.0);
        assert_close(evaluate("log2(1024)").unwrap(), 10.0);
    }

    #[test]
    fn fn_inverse_trig_returns_degrees() {
        assert_close(evaluate("asin(1)").unwrap(), 90.0);
        assert_close(evaluate("acos(0)").unwrap(), 90.0);
        assert_close(evaluate("atan(1)").unwrap(), 45.0);
    }

    #[test]
    fn fn_inverse_trig_domain_errors() {
        assert!(matches!(
            evaluate("asin(2)").unwrap_err(),
            ExpressionError::DomainError { .. }
        ));
        assert!(matches!(
            evaluate("acos(-2)").unwrap_err(),
            ExpressionError::DomainError { .. }
        ));
    }

    #[test]
    fn fn_atan2_quadrants() {
        assert_close(evaluate("atan2(1, 1)").unwrap(), 45.0);
        assert_close(evaluate("atan2(1, 0)").unwrap(), 90.0);
        assert_close(evaluate("atan2(-1, -1)").unwrap(), -135.0);
    }

    #[test]
    fn fn_hyperbolic() {
        assert_close(evaluate("sinh(0)").unwrap(), 0.0);
        assert_close(evaluate("cosh(0)").unwrap(), 1.0);
        assert_close(evaluate("tanh(0)").unwrap(), 0.0);
    }

    #[test]
    fn fn_inverse_hyperbolic() {
        assert_close(evaluate("asinh(0)").unwrap(), 0.0);
        assert_close(evaluate("acosh(1)").unwrap(), 0.0);
        assert_close(evaluate("atanh(0)").unwrap(), 0.0);
    }

    #[test]
    fn fn_acosh_below_one_is_domain_error() {
        assert!(matches!(
            evaluate("acosh(0.5)").unwrap_err(),
            ExpressionError::DomainError { .. }
        ));
    }

    #[test]
    fn fn_atanh_at_boundary_is_domain_error() {
        assert!(matches!(
            evaluate("atanh(1)").unwrap_err(),
            ExpressionError::DomainError { .. }
        ));
        assert!(matches!(
            evaluate("atanh(-1)").unwrap_err(),
            ExpressionError::DomainError { .. }
        ));
    }

    #[test]
    fn fn_round_trunc_sign() {
        assert_close(evaluate("round(2.5)").unwrap(), 3.0);
        assert_close(evaluate("round(-2.5)").unwrap(), -3.0);
        assert_close(evaluate("trunc(3.9)").unwrap(), 3.0);
        assert_close(evaluate("trunc(-3.9)").unwrap(), -3.0);
        assert_close(evaluate("sign(-7)").unwrap(), -1.0);
        assert_close(evaluate("sign(0)").unwrap(), 0.0);
        assert_close(evaluate("sign(5)").unwrap(), 1.0);
    }

    #[test]
    fn fn_factorial() {
        assert_close(evaluate("factorial(0)").unwrap(), 1.0);
        assert_close(evaluate("factorial(5)").unwrap(), 120.0);
        assert_close(evaluate("factorial(10)").unwrap(), 3_628_800.0);
    }

    #[test]
    fn fn_factorial_negative_or_fractional_is_domain_error() {
        assert!(matches!(
            evaluate("factorial(-1)").unwrap_err(),
            ExpressionError::DomainError { .. }
        ));
        assert!(matches!(
            evaluate("factorial(1.5)").unwrap_err(),
            ExpressionError::DomainError { .. }
        ));
    }

    #[test]
    fn fn_cbrt() {
        assert_close(evaluate("cbrt(27)").unwrap(), 3.0);
        assert_close(evaluate("cbrt(-8)").unwrap(), -2.0);
    }

    // ---- multi-arg functions ----

    #[test]
    fn fn_min_max_variadic() {
        assert_close(evaluate("min(3, 1, 2)").unwrap(), 1.0);
        assert_close(evaluate("max(3, 1, 2)").unwrap(), 3.0);
        assert_close(evaluate("min(-5, -10, 0)").unwrap(), -10.0);
    }

    #[test]
    fn fn_min_max_single_arg() {
        assert_close(evaluate("min(7)").unwrap(), 7.0);
        assert_close(evaluate("max(7)").unwrap(), 7.0);
    }

    #[test]
    fn fn_mod_two_args() {
        assert_close(evaluate("mod(10, 3)").unwrap(), 1.0);
        assert_close(evaluate("mod(-7, 3)").unwrap(), -1.0);
    }

    #[test]
    fn fn_mod_by_zero_is_division_error() {
        assert!(matches!(
            evaluate("mod(5, 0)").unwrap_err(),
            ExpressionError::DivisionByZero
        ));
    }

    #[test]
    fn fn_hypot() {
        assert_close(evaluate("hypot(3, 4)").unwrap(), 5.0);
    }

    #[test]
    fn fn_pow_two_args() {
        assert_close(evaluate("pow(2, 10)").unwrap(), 1024.0);
        assert_close(evaluate("pow(2, 0.5)").unwrap(), std::f64::consts::SQRT_2);
    }

    #[test]
    fn fn_gcd_lcm() {
        assert_close(evaluate("gcd(12, 18)").unwrap(), 6.0);
        assert_close(evaluate("gcd(7, 13)").unwrap(), 1.0);
        assert_close(evaluate("lcm(4, 6)").unwrap(), 12.0);
        assert_close(evaluate("lcm(0, 5)").unwrap(), 0.0);
    }

    #[test]
    fn fn_gcd_fractional_is_domain_error() {
        assert!(matches!(
            evaluate("gcd(2.5, 4)").unwrap_err(),
            ExpressionError::DomainError { .. }
        ));
    }

    #[test]
    fn fn_arity_mismatch_is_domain_error() {
        // sin needs 1 arg; passing 2 surfaces a DomainError with arity detail.
        assert!(matches!(
            evaluate("sin(1, 2)").unwrap_err(),
            ExpressionError::DomainError { .. }
        ));
        assert!(matches!(
            evaluate("atan2(1)").unwrap_err(),
            ExpressionError::DomainError { .. }
        ));
    }

    #[test]
    fn fn_no_args_for_zero_arity_func_is_unknown() {
        // We don't define any zero-arity functions; calling `pi()` is unknown.
        assert!(matches!(
            evaluate("pi()").unwrap_err(),
            ExpressionError::UnknownFunction(_)
        ));
    }

    // ---- composition with constants ----

    #[test]
    fn sin_pi_is_zero() {
        // sin takes degrees; we want sin(pi rad) = sin(180°). Convert manually.
        // But we also keep the constant available as the radian value, so this
        // tests that constants compose cleanly with operator math.
        let out = evaluate("sin(pi * 180 / pi)").unwrap();
        assert_close(out, 0.0);
    }

    #[test]
    fn sin_r_pi_is_zero() {
        // Radian variant: sin_r(pi) = 0 exactly (within FP noise). This exists
        // so callers combining trig with `pi`/`tau` get the expected result
        // without manually rescaling by 180/pi.
        let out = evaluate("sin_r(pi)").unwrap();
        assert_close(out, 0.0);
    }

    #[test]
    fn cos_r_pi_is_minus_one() {
        let out = evaluate("cos_r(pi)").unwrap();
        assert_close(out, -1.0);
    }

    #[test]
    fn tan_r_at_quarter_pi_is_one() {
        // Spot-check the radian variant: tan_r(π/4) = 1.
        let out = evaluate("tan_r(pi/4)").unwrap();
        assert_close(out, 1.0);
    }

    #[test]
    fn exp_ln_round_trip() {
        assert_close(evaluate("ln(exp(2.5))").unwrap(), 2.5);
    }
}
