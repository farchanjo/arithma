//! Core math engine — shared infrastructure consumed by all tools.
//!
//! - [`expression`] — recursive-descent expression evaluator (port of Java `ExpressionEvaluator`)
//! - [`unit_registry`] — unit catalog + conversion engine (port of Java `UnitRegistry`)
//! - [`bigdecimal_ext`] — helpers matching Java `BigDecimal` semantics (scale, HALF_UP, plain-string output)

pub mod bigdecimal_ext;
pub mod expression;
pub mod expression_exact;
pub mod unit_registry;
