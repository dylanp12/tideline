//! TLR/1 — the Tideline Record Protocol.
//!
//! Wire types, the canonical form, chain verification, and signed checkpoints.
//! This crate performs no I/O: an auditor verifying an exported record should
//! compile nothing that can open a socket.

pub mod canon;
pub mod chain;
pub mod checkpoint;
pub mod event;
pub mod hash;
pub mod kind;

pub use canon::{EventCore, CANON_LEN};
pub use chain::{verify_chain, ChainError, ChainOk};
pub use checkpoint::{Checkpoint, CheckpointError, CHECKPOINT_CANON_LEN};
pub use event::{Agent, RunEnvelope, RunEvent};
pub use hash::{Hash, ZERO};
pub use kind::EventKind;
