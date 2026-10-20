//! TLR/1 — the tamper-evident record protocol.
//!
//! `store` is the interface, `sqlite` the single-node implementation, `routes`
//! the `/v1` HTTP binding. Approvals are a projection over the chain
//! (`approvals`), not a separate table, so the record and the oversight trail
//! cannot disagree.

pub mod approvals;
pub mod auth;
pub mod keys;
pub mod metrics;
pub mod routes;
pub mod sqlite;
pub mod store;
