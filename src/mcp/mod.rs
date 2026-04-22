//! MCP transport helpers and response formatting.
//!
//! The [`message`] submodule is the single source of truth for the wire format
//! emitted by every tool: a compact markdown envelope of the form
//! `TOOL_NAME: STATUS [| KEY: value ...]` (inline) or `KEY: value` per line
//! (block), with errors as a three-line block starting with
//! `TOOL_NAME: ERROR`.

pub mod message;
