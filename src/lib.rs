//! Shared crate code: tuning constants/helpers and the network protocol.
//!
//! Everything in here is used by BOTH the server (`src/bin/server.rs`) and the
//! client (`src/main.rs`). The two sides must agree on these values exactly,
//! otherwise movement desyncs and snapshots fail to decode.

pub mod config;
pub mod protocol;
