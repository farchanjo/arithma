//! `math-calc` — pure-Rust MCP server porting the Spring AI math-calculator.
//!
//! Public surface is intentionally narrow: the server binary consumes this crate.

pub mod engine;
pub mod mcp;
pub mod server;
pub mod tools;
