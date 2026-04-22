//! Canonical mapping from [`ExpressionError`] to the tool-scoped response
//! envelope. Every tool that surfaces expression errors (`programmable`,
//! `calculus`, `graphing`) delegates here so the REASON text and DETAIL shape
//! stay identical across the API.

use super::{ErrorCode, error, error_with_detail};
use crate::engine::expression::ExpressionError;

/// Produce the envelope string for `err`, scoped to the given tool name.
#[must_use]
pub fn expression_error_envelope(tool: &str, err: &ExpressionError) -> String {
    let (code, reason, detail): (ErrorCode, &str, Option<String>) = match err {
        ExpressionError::Empty => (
            ErrorCode::InvalidInput,
            "expression must not be blank",
            None,
        ),
        ExpressionError::UnexpectedChar { pos, ch } => (
            ErrorCode::ParseError,
            "unexpected character in expression",
            Some(format!("pos={pos}, char={ch}")),
        ),
        ExpressionError::UnexpectedEnd => (
            ErrorCode::ParseError,
            "unexpected end of expression",
            None,
        ),
        ExpressionError::InvalidNumber(token) => (
            ErrorCode::ParseError,
            "invalid number literal",
            Some(format!("token={token}")),
        ),
        ExpressionError::UnknownVariable(name) => (
            ErrorCode::UnknownVariable,
            "expression references an unknown variable",
            Some(format!("name={name}")),
        ),
        ExpressionError::UnknownFunction(name) => (
            ErrorCode::UnknownFunction,
            "expression calls an unknown function",
            Some(format!("name={name}")),
        ),
        ExpressionError::ExpectedCloseParen { pos } => (
            ErrorCode::ParseError,
            "missing closing parenthesis",
            Some(format!("pos={pos}")),
        ),
        ExpressionError::DivisionByZero => (
            ErrorCode::DivisionByZero,
            "cannot divide or take modulo by zero",
            None,
        ),
        ExpressionError::DomainError { op, value } => (
            ErrorCode::DomainError,
            "operation is undefined for this input",
            Some(format!("op={op}, value={value}")),
        ),
    };
    match detail {
        Some(d) => error_with_detail(tool, code, reason, &d),
        None => error(tool, code, reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_variable_renders_with_detail() {
        let err = ExpressionError::UnknownVariable("foo".into());
        assert_eq!(
            expression_error_envelope("EVALUATE", &err),
            "EVALUATE: ERROR\n\
             REASON: [UNKNOWN_VARIABLE] expression references an unknown variable\n\
             DETAIL: name=foo"
        );
    }

    #[test]
    fn domain_error_maps_to_domain_error_code() {
        let err = ExpressionError::DomainError {
            op: "log".into(),
            value: "0".into(),
        };
        assert_eq!(
            expression_error_envelope("EVALUATE_EXACT", &err),
            "EVALUATE_EXACT: ERROR\n\
             REASON: [DOMAIN_ERROR] operation is undefined for this input\n\
             DETAIL: op=log, value=0"
        );
    }

    #[test]
    fn division_by_zero_has_no_detail() {
        assert_eq!(
            expression_error_envelope("EVALUATE_EXACT", &ExpressionError::DivisionByZero),
            "EVALUATE_EXACT: ERROR\n\
             REASON: [DIVISION_BY_ZERO] cannot divide or take modulo by zero"
        );
    }
}
