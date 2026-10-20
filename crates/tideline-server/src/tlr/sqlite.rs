//! Single-node TLR/1 store on embedded SQLite.
//!
//! One connection, WAL, and a busy timeout — the previous record store opened
//! the same file twice with neither, so concurrent writes returned
//! `SQLITE_BUSY` and every statement's `.expect` turned that into a panic
//! (audit finding 3). Every write runs inside one `BEGIN IMMEDIATE`
//! transaction, because `prev_hash` must be read and the successor written
//! under the same lock or the chain forks.

use super::store::*;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use serde_json::value::RawValue;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tideline_proto::{
    canon::REDACTABLE, Agent, Checkpoint, EventCore, EventKind, Hash, RunEnvelope, RunEvent, ZERO,
};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS tlr_runs (
  ns            TEXT    NOT NULL,
  run_id        TEXT    NOT NULL,
  started_ts    INTEGER NOT NULL,
  ended_ts      INTEGER,
  agent_name    TEXT    NOT NULL,
  agent_version TEXT    NOT NULL,
  subject_ref   TEXT,
  labels        TEXT    NOT NULL DEFAULT '{}',
  head_seq      INTEGER NOT NULL,
  head_hash     BLOB    NOT NULL,
  PRIMARY KEY (ns, run_id)
);
CREATE TABLE IF NOT EXISTS tlr_events (
  ns        TEXT    NOT NULL,
  run_id    TEXT    NOT NULL,
  seq       INTEGER NOT NULL,
  ts        INTEGER NOT NULL,
  kind      TEXT    NOT NULL,
  role      TEXT,
  name      TEXT,
  content   TEXT,
  metadata  TEXT,
  redacted  TEXT,
  prev_hash BLOB    NOT NULL,
  hash      BLOB    NOT NULL,
  PRIMARY KEY (ns, run_id, seq)
);
CREATE TABLE IF NOT EXISTS tlr_idem (
  ns        TEXT    NOT NULL,
  run_id    TEXT    NOT NULL,
  key       TEXT    NOT NULL,
  seq       INTEGER NOT NULL,
  ts        INTEGER NOT NULL,
  prev_hash BLOB    NOT NULL,
  hash      BLOB    NOT NULL,
  PRIMARY KEY (ns, run_id, key)
);
CREATE TABLE IF NOT EXISTS tlr_checkpoints (
  ns        TEXT    NOT NULL,
  run_id    TEXT    NOT NULL,
  seq       INTEGER NOT NULL,
  head_hash BLOB    NOT NULL,
  ts        INTEGER NOT NULL,
  key_id    TEXT    NOT NULL,
  sig       TEXT    NOT NULL,
  PRIMARY KEY (ns, run_id, seq)
);
CREATE INDEX IF NOT EXISTS tlr_runs_started ON tlr_runs (ns, started_ts DESC);
CREATE INDEX IF NOT EXISTS tlr_events_kind  ON tlr_events (ns, run_id, kind);
"#;

/// The envelope as written into the `run_started` event's metadata.
///
/// Deliberately excludes `head_seq` and `head_hash`: those describe the chain
/// this event is the first link of, so including them would be circular.
#[derive(Serialize)]
struct EnvelopeMeta<'a> {
    run_id: &'a str,
    agent: &'a Agent,
    #[serde(skip_serializing_if = "Option::is_none")]
    subject_ref: Option<&'a str>,
    labels: &'a BTreeMap<String, String>,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn blob(h: &Hash) -> Vec<u8> {
    h.0.to_vec()
}

fn unblob(v: Vec<u8>) -> StoreResult<Hash> {
    let arr: [u8; 32] = v
        .try_into()
        .map_err(|_| StoreError::Backend("hash column is not 32 bytes".into()))?;
    Ok(Hash(arr))
}

pub struct SqliteTlrStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteTlrStore {
    pub fn open(path: &str) -> StoreResult<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn in_memory() -> StoreResult<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> StoreResult<Self> {
        // WAL lets readers proceed during a write; the busy timeout turns a
        // contended write into a wait rather than an immediate SQLITE_BUSY.
        // An in-memory database cannot use WAL, so ignore that failure.
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Run `f` on the blocking pool with the connection locked.
    async fn with_conn<T, F>(&self, f: F) -> StoreResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> StoreResult<T> + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|_| StoreError::Backend("record store lock poisoned".into()))?;
            f(&mut guard)
        })
        .await
        .map_err(|e| StoreError::Backend(format!("blocking task: {e}")))?
    }
}

/// Read one event row. Column order must match `EVENT_COLS`.
const EVENT_COLS: &str = "seq, ts, kind, role, name, content, metadata, redacted, prev_hash, hash";

fn row_to_event(r: &rusqlite::Row) -> rusqlite::Result<RunEvent> {
    let seq: i64 = r.get(0)?;
    let ts: i64 = r.get(1)?;
    let kind_s: String = r.get(2)?;
    let metadata: Option<String> = r.get(6)?;
    let redacted: Option<String> = r.get(7)?;
    let prev: Vec<u8> = r.get(8)?;
    let hash: Vec<u8> = r.get(9)?;
    Ok(RunEvent {
        seq: seq as u64,
        ts: ts as u64,
        // The column is written from `EventKind::as_str`, so an unparseable
        // value means the database was edited outside the server.
        kind: EventKind::parse(&kind_s).unwrap_or(EventKind::Message),
        role: r.get(3)?,
        name: r.get(4)?,
        content: r.get(5)?,
        metadata: metadata.and_then(|s| RawValue::from_string(s).ok()),
        redacted: redacted.and_then(|s| serde_json::from_str(&s).ok()),
        prev_hash: Hash(prev.try_into().unwrap_or([0u8; 32])),
        hash: Hash(hash.try_into().unwrap_or([0u8; 32])),
    })
}

/// The head of a run, or the reason it cannot be appended to.
fn head_of(tx: &rusqlite::Transaction, ns: &str, run_id: &str) -> StoreResult<(u64, Hash)> {
    let row: Option<(i64, Vec<u8>, Option<i64>)> = tx
        .query_row(
            "SELECT head_seq, head_hash, ended_ts FROM tlr_runs WHERE ns = ?1 AND run_id = ?2",
            rusqlite::params![ns, run_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    match row {
        None => Err(StoreError::NotFound),
        Some((_, _, Some(_))) => Err(StoreError::Conflict("run is sealed".into())),
        Some((seq, hash, None)) => Ok((seq as u64, unblob(hash)?)),
    }
}

/// Insert one already-hashed event and advance the run head.
#[allow(clippy::too_many_arguments)]
fn write_event(
    tx: &rusqlite::Transaction,
    ns: &str,
    run_id: &str,
    seq: u64,
    ts: u64,
    kind: EventKind,
    role: Option<&str>,
    name: Option<&str>,
    content: Option<&str>,
    metadata: Option<&str>,
    prev: &Hash,
    hash: &Hash,
) -> StoreResult<()> {
    tx.execute(
        &format!(
            "INSERT INTO tlr_events (ns, run_id, {EVENT_COLS}) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, ?11)"
        ),
        rusqlite::params![
            ns,
            run_id,
            seq as i64,
            ts as i64,
            kind.as_str(),
            role,
            name,
            content,
            metadata,
            blob(prev),
            blob(hash),
        ],
    )?;
    tx.execute(
        "UPDATE tlr_runs SET head_seq = ?3, head_hash = ?4 WHERE ns = ?1 AND run_id = ?2",
        rusqlite::params![ns, run_id, seq as i64, blob(hash)],
    )?;
    Ok(())
}

/// Append `event` to `run_id`, hashing it against the current head.
#[allow(clippy::too_many_arguments)]
fn append_in_tx(
    tx: &rusqlite::Transaction,
    ns: &str,
    run_id: &str,
    kind: EventKind,
    role: Option<&str>,
    name: Option<&str>,
    content: Option<&str>,
    metadata: Option<&str>,
) -> StoreResult<AppendResult> {
    let (head_seq, head_hash) = head_of(tx, ns, run_id)?;
    let seq = head_seq + 1;
    let ts = now_ms();
    let core = EventCore {
        seq,
        ts,
        kind,
        role,
        name,
        content,
        metadata_raw: metadata,
        redacted: None,
    };
    let hash = core.hash(&head_hash);
    write_event(
        tx, ns, run_id, seq, ts, kind, role, name, content, metadata, &head_hash, &hash,
    )?;
    Ok(AppendResult {
        seq,
        ts,
        prev_hash: head_hash,
        hash,
    })
}

#[async_trait::async_trait]
impl TlrStore for SqliteTlrStore {
    async fn create_run(&self, ns: &str, new: NewRun) -> StoreResult<RunEnvelope> {
        let ns = ns.to_string();
        self.with_conn(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let exists: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM tlr_runs WHERE ns = ?1 AND run_id = ?2",
                    rusqlite::params![&ns, &new.run_id],
                    |r| r.get(0),
                )
                .optional()?;
            if exists.is_some() {
                return Err(StoreError::Conflict(format!(
                    "run {} already exists",
                    new.run_id
                )));
            }

            let ts = now_ms();
            let meta = serde_json::to_string(&EnvelopeMeta {
                run_id: &new.run_id,
                agent: &new.agent,
                subject_ref: new.subject_ref.as_deref(),
                labels: &new.labels,
            })?;
            let core = EventCore {
                seq: 0,
                ts,
                kind: EventKind::RunStarted,
                role: None,
                name: None,
                content: None,
                metadata_raw: Some(&meta),
                redacted: None,
            };
            let hash = core.hash(&ZERO);

            tx.execute(
                "INSERT INTO tlr_runs \
                   (ns, run_id, started_ts, ended_ts, agent_name, agent_version, \
                    subject_ref, labels, head_seq, head_hash) \
                 VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, 0, ?8)",
                rusqlite::params![
                    &ns,
                    &new.run_id,
                    ts as i64,
                    &new.agent.name,
                    &new.agent.version,
                    new.subject_ref.as_deref(),
                    serde_json::to_string(&new.labels)?,
                    blob(&hash),
                ],
            )?;
            write_event(
                &tx,
                &ns,
                &new.run_id,
                0,
                ts,
                EventKind::RunStarted,
                None,
                None,
                None,
                Some(&meta),
                &ZERO,
                &hash,
            )?;
            tx.commit()?;

            Ok(RunEnvelope {
                run_id: new.run_id,
                started_ts: ts,
                ended_ts: None,
                agent: new.agent,
                subject_ref: new.subject_ref,
                labels: new.labels,
                head_seq: 0,
                head_hash: hash,
            })
        })
        .await
    }

    async fn get_run(&self, ns: &str, run_id: &str) -> StoreResult<RunEnvelope> {
        let (ns, run_id) = (ns.to_string(), run_id.to_string());
        self.with_conn(move |c| {
            c.query_row(
                "SELECT run_id, started_ts, ended_ts, agent_name, agent_version, \
                        subject_ref, labels, head_seq, head_hash \
                 FROM tlr_runs WHERE ns = ?1 AND run_id = ?2",
                rusqlite::params![ns, run_id],
                row_to_envelope,
            )
            .optional()?
            .ok_or(StoreError::NotFound)?
        })
        .await
    }

    async fn list_runs(&self, ns: &str, q: RunQuery) -> StoreResult<RunPage> {
        let ns = ns.to_string();
        self.with_conn(move |c| {
            let limit = q.limit.unwrap_or(50).clamp(1, 500);
            let mut sql = String::from(
                "SELECT run_id, started_ts, ended_ts, agent_name, agent_version, \
                        subject_ref, labels, head_seq, head_hash \
                 FROM tlr_runs WHERE ns = ?1",
            );
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(ns.clone())];

            if let Some(agent) = &q.agent {
                params.push(Box::new(agent.clone()));
                sql.push_str(&format!(" AND agent_name = ?{}", params.len()));
            }
            if let Some(label) = &q.label {
                let (k, v) = label
                    .split_once('=')
                    .ok_or_else(|| StoreError::Invalid("label filter must be key=value".into()))?;
                params.push(Box::new(format!("$.{k}")));
                let kp = params.len();
                params.push(Box::new(v.to_string()));
                sql.push_str(&format!(
                    " AND json_extract(labels, ?{kp}) = ?{}",
                    params.len()
                ));
            }
            if let Some(since) = q.since {
                params.push(Box::new(since as i64));
                sql.push_str(&format!(" AND started_ts >= ?{}", params.len()));
            }
            if let Some(cursor) = &q.cursor {
                let (ts, id) = cursor
                    .split_once(':')
                    .ok_or_else(|| StoreError::Invalid("malformed cursor".into()))?;
                let ts: i64 = ts
                    .parse()
                    .map_err(|_| StoreError::Invalid("malformed cursor".into()))?;
                params.push(Box::new(ts));
                let tp = params.len();
                params.push(Box::new(id.to_string()));
                sql.push_str(&format!(
                    " AND (started_ts < ?{tp} OR (started_ts = ?{tp} AND run_id > ?{}))",
                    params.len()
                ));
            }
            // One past the page, to learn whether another page exists.
            sql.push_str(&format!(
                " ORDER BY started_ts DESC, run_id ASC LIMIT {}",
                limit + 1
            ));

            let mut stmt = c.prepare(&sql)?;
            let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
            let rows = stmt.query_map(refs.as_slice(), row_to_envelope)?;
            let mut runs: Vec<RunEnvelope> = Vec::new();
            for r in rows {
                runs.push(r??);
            }

            let next_cursor = if runs.len() > limit as usize {
                runs.truncate(limit as usize);
                runs.last()
                    .map(|r| format!("{}:{}", r.started_ts, r.run_id))
            } else {
                None
            };
            Ok(RunPage { runs, next_cursor })
        })
        .await
    }

    async fn append(
        &self,
        ns: &str,
        run_id: &str,
        event: NewEvent,
        idempotency_key: Option<&str>,
    ) -> StoreResult<AppendResult> {
        let (ns, run_id) = (ns.to_string(), run_id.to_string());
        let idem = idempotency_key.map(str::to_string);
        self.with_conn(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;

            // A retry after a timeout must not put a phantom event into
            // evidence: it would verify, because the chain proves integrity,
            // not correctness at the time of writing.
            if let Some(key) = &idem {
                let prior: Option<(i64, i64, Vec<u8>, Vec<u8>)> = tx
                    .query_row(
                        "SELECT seq, ts, prev_hash, hash FROM tlr_idem \
                         WHERE ns = ?1 AND run_id = ?2 AND key = ?3",
                        rusqlite::params![&ns, &run_id, key],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .optional()?;
                if let Some((seq, ts, prev, hash)) = prior {
                    return Ok(AppendResult {
                        seq: seq as u64,
                        ts: ts as u64,
                        prev_hash: unblob(prev)?,
                        hash: unblob(hash)?,
                    });
                }
            }

            let meta = event.metadata.as_deref().map(|r| r.get().to_string());
            let res = append_in_tx(
                &tx,
                &ns,
                &run_id,
                event.kind,
                event.role.as_deref(),
                event.name.as_deref(),
                event.content.as_deref(),
                meta.as_deref(),
            )?;

            if let Some(key) = &idem {
                tx.execute(
                    "INSERT INTO tlr_idem (ns, run_id, key, seq, ts, prev_hash, hash) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        &ns,
                        &run_id,
                        key,
                        res.seq as i64,
                        res.ts as i64,
                        blob(&res.prev_hash),
                        blob(&res.hash),
                    ],
                )?;
            }
            tx.commit()?;
            Ok(res)
        })
        .await
    }

    async fn events(
        &self,
        ns: &str,
        run_id: &str,
        from: u64,
        limit: u32,
    ) -> StoreResult<Vec<RunEvent>> {
        let (ns, run_id) = (ns.to_string(), run_id.to_string());
        self.with_conn(move |c| {
            let limit = limit.clamp(1, 5000);
            let mut stmt = c.prepare(&format!(
                "SELECT {EVENT_COLS} FROM tlr_events \
                 WHERE ns = ?1 AND run_id = ?2 AND seq >= ?3 ORDER BY seq LIMIT ?4"
            ))?;
            let rows = stmt.query_map(
                rusqlite::params![ns, run_id, from as i64, limit],
                row_to_event,
            )?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
        .await
    }

    async fn seal(&self, ns: &str, run_id: &str) -> StoreResult<AppendResult> {
        let (ns, run_id) = (ns.to_string(), run_id.to_string());
        self.with_conn(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (head_seq, _) = head_of(&tx, &ns, &run_id)?;
            // The count lets a verifier detect truncation of a sealed run from
            // the record alone, without reference to a checkpoint.
            let meta = serde_json::json!({ "event_count": head_seq + 2 }).to_string();
            let res = append_in_tx(
                &tx,
                &ns,
                &run_id,
                EventKind::RunFinished,
                None,
                None,
                None,
                Some(&meta),
            )?;
            tx.execute(
                "UPDATE tlr_runs SET ended_ts = ?3 WHERE ns = ?1 AND run_id = ?2",
                rusqlite::params![&ns, &run_id, res.ts as i64],
            )?;
            tx.commit()?;
            Ok(res)
        })
        .await
    }

    async fn redact(
        &self,
        ns: &str,
        run_id: &str,
        target_seq: u64,
        fields: &[String],
        authority: &str,
    ) -> StoreResult<AppendResult> {
        let (ns, run_id) = (ns.to_string(), run_id.to_string());
        let fields = fields.to_vec();
        let authority = authority.to_string();
        self.with_conn(move |c| {
            for f in &fields {
                if !REDACTABLE.contains(&f.as_str()) {
                    return Err(StoreError::Invalid(format!(
                        "{f} is structural and cannot be redacted"
                    )));
                }
            }
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;

            let row: Option<(
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
            )> = tx
                .query_row(
                    "SELECT role, name, content, metadata, redacted FROM tlr_events \
                     WHERE ns = ?1 AND run_id = ?2 AND seq = ?3",
                    rusqlite::params![&ns, &run_id, target_seq as i64],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .optional()?;
            let (role, name, content, metadata, redacted) = row.ok_or(StoreError::NotFound)?;

            let mut kept: BTreeMap<String, Hash> = redacted
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();

            for f in &fields {
                let present = match f.as_str() {
                    "role" => role.as_deref(),
                    "name" => name.as_deref(),
                    "content" => content.as_deref(),
                    "metadata" => metadata.as_deref(),
                    _ => None,
                };
                // Already erased: skip, so a repeat redaction is a no-op.
                if let Some(v) = present {
                    kept.insert(f.clone(), Hash::of(v.as_bytes()));
                    tx.execute(
                        &format!(
                            "UPDATE tlr_events SET {f} = NULL \
                             WHERE ns = ?1 AND run_id = ?2 AND seq = ?3"
                        ),
                        rusqlite::params![&ns, &run_id, target_seq as i64],
                    )?;
                }
            }
            tx.execute(
                "UPDATE tlr_events SET redacted = ?4 WHERE ns = ?1 AND run_id = ?2 AND seq = ?3",
                rusqlite::params![
                    &ns,
                    &run_id,
                    target_seq as i64,
                    serde_json::to_string(&kept)?
                ],
            )?;

            let meta = serde_json::json!({
                "target_seq": target_seq,
                "fields": fields,
                "authority": authority,
            })
            .to_string();
            let res = append_in_tx(
                &tx,
                &ns,
                &run_id,
                EventKind::Redaction,
                None,
                None,
                None,
                Some(&meta),
            )?;
            tx.commit()?;
            Ok(res)
        })
        .await
    }

    async fn latest_checkpoint(&self, ns: &str, run_id: &str) -> StoreResult<Option<Checkpoint>> {
        let (ns, run_id) = (ns.to_string(), run_id.to_string());
        self.with_conn(move |c| {
            let cp = c
                .query_row(
                    "SELECT seq, head_hash, ts, key_id, sig FROM tlr_checkpoints \
                     WHERE ns = ?1 AND run_id = ?2 ORDER BY seq DESC LIMIT 1",
                    rusqlite::params![&ns, &run_id],
                    |r| {
                        let seq: i64 = r.get(0)?;
                        let head: Vec<u8> = r.get(1)?;
                        let ts: i64 = r.get(2)?;
                        Ok((
                            seq,
                            head,
                            ts,
                            r.get::<_, String>(3)?,
                            r.get::<_, String>(4)?,
                        ))
                    },
                )
                .optional()?;
            match cp {
                None => Ok(None),
                Some((seq, head, ts, key_id, sig)) => Ok(Some(Checkpoint {
                    run_id,
                    seq: seq as u64,
                    head_hash: unblob(head)?,
                    ts: ts as u64,
                    key_id,
                    sig,
                })),
            }
        })
        .await
    }

    async fn put_checkpoint(&self, ns: &str, cp: &Checkpoint) -> StoreResult<()> {
        let ns = ns.to_string();
        let cp = cp.clone();
        self.with_conn(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO tlr_checkpoints \
                   (ns, run_id, seq, head_hash, ts, key_id, sig) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    ns,
                    cp.run_id,
                    cp.seq as i64,
                    blob(&cp.head_hash),
                    cp.ts as i64,
                    cp.key_id,
                    cp.sig,
                ],
            )?;
            Ok(())
        })
        .await
    }
}

fn row_to_envelope(r: &rusqlite::Row) -> rusqlite::Result<StoreResult<RunEnvelope>> {
    let started: i64 = r.get(1)?;
    let ended: Option<i64> = r.get(2)?;
    let labels: String = r.get(6)?;
    let head_seq: i64 = r.get(7)?;
    let head: Vec<u8> = r.get(8)?;
    Ok((|| {
        Ok(RunEnvelope {
            run_id: r.get(0)?,
            started_ts: started as u64,
            ended_ts: ended.map(|v| v as u64),
            agent: Agent {
                name: r.get(3)?,
                version: r.get(4)?,
            },
            subject_ref: r.get(5)?,
            labels: serde_json::from_str(&labels).unwrap_or_default(),
            head_seq: head_seq as u64,
            head_hash: unblob(head)?,
        })
    })())
}
