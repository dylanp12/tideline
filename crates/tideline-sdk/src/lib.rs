//! Official Rust client for TLR/1 — the Tideline Record Protocol.
//!
//! ```no_run
//! use std::time::Duration;
//! use tideline_sdk::{Agent, NewEvent, Tideline};
//!
//! # async fn example() -> tideline_sdk::Result<()> {
//! let tl = Tideline::new("http://localhost:8080", None);
//! let run = tl
//!     .start_run("loan-4821", Agent::new("underwriter", "2.1.0"))
//!     .subject_ref("applicant-4821")
//!     .label("product", "personal-loan")
//!     .send()
//!     .await?;
//!
//! run.record(NewEvent::tool_call("pull_credit_report").content("score=690"))
//!     .await?;
//!
//! // Blocks until a human decides. Both the request and the decision are in
//! // the chain by the time this returns.
//! let decision = run.gate("Approve EUR40,000 loan", Duration::from_secs(7200)).await?;
//! if decision.is_approved() {
//!     run.record(NewEvent::decision("loan_approved").content("EUR40,000")).await?;
//! }
//! run.complete().await?;
//!
//! // Verification needs no server and no trust.
//! let events = run.verified_events().await?;
//! println!("{} events verified", events.len());
//! # Ok(())
//! # }
//! ```
//!
//! For verification alone — an auditor checking an exported record — depend on
//! [`tideline_proto`] instead. It has no I/O and cannot open a socket.

pub mod client;
pub mod error;
pub mod event;
pub mod run;

pub use client::{RunPage, RunQuery, StartRun, Tideline};
pub use error::{Error, Result};
pub use event::NewEvent;
pub use run::{Appended, Decision, Gate, OpenedGate, Resolution, Run};

// Re-exported so a caller needs one dependency, not two.
pub use tideline_proto::{
    verify_chain, Agent, ChainError, ChainOk, Checkpoint, EventKind, Hash, RunEnvelope, RunEvent,
};
