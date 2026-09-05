//! lsp-det: a transparent proxy that is the reference implementation of the server state
//! protocol (docs/spec/server-state.md). The upstream side stands in for the language server,
//! the downstream side for the client.
//!
//! The binary itself is a thin layer that only calls [`proxy::run`]. The modules are exposed
//! as a library so that the conformance test suite (`tests/`) shares the framing and the
//! `ServerState` type definitions. The suite has to be written so that it can also be run
//! against real servers, and if the types were defined twice, the very yardstick that measures
//! "does this conform to the spec" would drift.

pub mod adapter;
pub mod cli;
pub mod documents;
pub mod framing;
pub mod gate;
pub mod initialize;
pub mod peek;
pub mod process;
pub mod proxy;
pub mod state;
pub mod tracker;
pub mod uri;
pub mod watched_files;
