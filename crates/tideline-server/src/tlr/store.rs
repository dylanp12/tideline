//! Storage interface for TLR/1 records.
//!
//! Every method returns `Result`. The previous record store ended each statement
//! in `.expect`, so a `SQLITE_BUSY` panicked the task instead of failing the
//! request — audit finding 3.

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::collections::BTreeMap;
use tideline_proto::{Checkpoint, EventKind, Hash, RunEnvelope, RunEvent};

/// What a client supplies when opening a run.
#[derive(Clone, Debug, Deserialize)]
pub struct NewRun {
    pub run_id: String,
    pub agent: tideline_proto::Agent,
    #[serde(default)]
    pub subject_ref: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// What a client supplies when appending. `seq`, `ts`, and both hashes are the
/// server's to assign; a client-supplied value is ignored.
#[derive(Clone, Debug, Deserialize)]
pub struct NewEvent {
    pub kind: EventKind,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub metadata: Option<Box<RawValue>>,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct AppendResult {
    pub seq: u64,
    pub ts: u64,
    pub prev_hash: Hash,
    pub hash: Hash,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct RunQuery {
    #[serde(default)]
    pub agent: Option<String>,
    /// `key=value`, matched against the run's labels.
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub since: Option<u64>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunPage {
    pub runs: Vec<RunEnvelope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    NotFound,
    /// The request contradicts the record's state: a sealed run, a resolved
    /// gate, a duplicate run id.
    Conflict(String),
    /// Bad input that storage rejected, e.g. redacting a structural field.
    Invalid(String),
    Backend(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::NotFound => write!(f, "not found"),
            StoreError::Conflict(m) => write!(f, "conflict: {m}"),
            StoreError::Invalid(m) => write!(f, "invalid: {m}"),
            StoreError::Backend(m) => write!(f, "backend: {m}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Backend(e.to_string())
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError::Backend(e.to_string())
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

/// A TLR/1 record store.
///
/// `ns` is the tenant namespace, always derived from the authenticated
/// credential and never from a request parameter — audit finding 1.
#[async_trait::async_trait]
pub trait TlrStore: Send + Sync + 'static {
    async fn create_run(&self, ns: &str, new: NewRun) -> StoreResult<RunEnvelope>;
    async fn get_run(&self, ns: &str, run_id: &str) -> StoreResult<RunEnvelope>;
    async fn list_runs(&self, ns: &str, q: RunQuery) -> StoreResult<RunPage>;

    /// Append one event, assigning `seq`, `ts`, `prev_hash`, and `hash` under a
    /// per-run lock. When `idempotency_key` repeats, returns the original
    /// result without appending again.
    async fn append(
        &self,
        ns: &str,
        run_id: &str,
        event: NewEvent,
        idempotency_key: Option<&str>,
    ) -> StoreResult<AppendResult>;

    async fn events(
        &self,
        ns: &str,
        run_id: &str,
        from: u64,
        limit: u32,
    ) -> StoreResult<Vec<RunEvent>>;

    /// Write the terminal `run_finished` event and seal the run.
    async fn seal(&self, ns: &str, run_id: &str) -> StoreResult<AppendResult>;

    /// Erase fields of `target_seq`, moving their digests into `redacted`.
    async fn redact(
        &self,
        ns: &str,
        run_id: &str,
        target_seq: u64,
        fields: &[String],
        authority: &str,
    ) -> StoreResult<AppendResult>;

    async fn latest_checkpoint(&self, ns: &str, run_id: &str) -> StoreResult<Option<Checkpoint>>;
    async fn put_checkpoint(&self, ns: &str, cp: &Checkpoint) -> StoreResult<()>;
}
