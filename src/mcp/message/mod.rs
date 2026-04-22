//! Response envelope shared by every tool.
//!
//! Two entry points are exposed:
//! * [`Response`] — fluent builder that auto-picks inline vs. block layout.
//! * [`error`] / [`error_with_detail`] — three-line error envelope.
//!
//! Tools call the builder once per invocation and return the final `String`.
//! The server layer does no post-processing.

pub mod builder;
pub mod expression_error;
pub mod helpers;

pub use builder::{ErrorCode, Response, error, error_with_detail};
pub use expression_error::expression_error_envelope;
