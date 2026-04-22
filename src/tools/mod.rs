//! Tool implementations — 87 expert calculators across 15 categories.
//!
//! All tools return `String` so errors can be embedded as `"Error: ..."`
//! messages without protocol-level exceptions crossing the MCP boundary.

pub mod analog_electronics;
pub mod basic;
pub mod calculus;
pub mod cooking;
pub mod datetime;
pub mod digital_electronics;
pub mod financial;
pub mod graphing;
pub mod measure_reference;
pub mod network;
pub mod printing;
pub mod programmable;
pub mod scientific;
pub mod unit_converter;
pub mod vector;
