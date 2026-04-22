//! Core math engine — shared infrastructure consumed by all tools.
//!
//! - [`expression`] — recursive-descent expression evaluator with proper precedence
//! - [`unit_registry`] — 21 categories, 118 units with DECIMAL128 conversion factors
//! - [`bigdecimal_ext`] — arbitrary-precision helpers (DECIMAL128 semantics, HALF_UP rounding)

pub mod bigdecimal_ext;
pub mod expression;
pub mod expression_exact;
pub mod unit_registry;
