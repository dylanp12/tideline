use crate::{EventKind, Hash, RunEvent, ZERO};

/// What a valid chain tells you.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainOk {
    pub len: usize,
    pub head_seq: u64,
    pub head_hash: Hash,
    /// True when the chain ends in `run_finished`.
    pub sealed: bool,
}

/// Why a chain is not valid. Each variant names the event at fault, so a
/// verifier can report where a record was altered rather than only that it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainError {
    Empty,
    NotRunStarted,
    SeqGap {
        expected: u64,
        found: u64,
    },
    PrevMismatch {
        seq: u64,
        expected: Hash,
        found: Hash,
    },
    HashMismatch {
        seq: u64,
        expected: Hash,
        computed: Hash,
    },
}

/// Verify an ordered run record end to end.
///
/// Detects edited fields, inserted events, deleted events, and rewritten links.
/// It cannot detect truncation of the tail — a valid prefix is a valid chain —
/// which is what signed checkpoints are for.
pub fn verify_chain(events: &[RunEvent]) -> Result<ChainOk, ChainError> {
    let first = events.first().ok_or(ChainError::Empty)?;
    if first.kind != EventKind::RunStarted {
        return Err(ChainError::NotRunStarted);
    }

    let mut prev = ZERO;
    for (i, e) in events.iter().enumerate() {
        let expected_seq = i as u64;
        if e.seq != expected_seq {
            return Err(ChainError::SeqGap {
                expected: expected_seq,
                found: e.seq,
            });
        }
        if e.prev_hash != prev {
            return Err(ChainError::PrevMismatch {
                seq: e.seq,
                expected: prev,
                found: e.prev_hash,
            });
        }
        let computed = e.core().hash(&prev);
        if computed != e.hash {
            return Err(ChainError::HashMismatch {
                seq: e.seq,
                expected: e.hash,
                computed,
            });
        }
        prev = computed;
    }

    let last = events.last().expect("non-empty, checked above");
    Ok(ChainOk {
        len: events.len(),
        head_seq: last.seq,
        head_hash: last.hash,
        sealed: last.kind == EventKind::RunFinished,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventKind, Hash, RunEvent, ZERO};

    /// Build a valid chain of `n` events, the first `run_started`.
    fn chain(n: u64) -> Vec<RunEvent> {
        let mut out: Vec<RunEvent> = Vec::new();
        let mut prev = ZERO;
        for seq in 0..n {
            let mut e = RunEvent {
                seq,
                ts: 1_757_635_200_000 + seq,
                kind: if seq == 0 {
                    EventKind::RunStarted
                } else {
                    EventKind::Message
                },
                content: Some(format!("event {seq}")),
                prev_hash: prev,
                ..Default::default()
            };
            e.hash = e.core().hash(&prev);
            prev = e.hash;
            out.push(e);
        }
        out
    }

    #[test]
    fn accepts_a_well_formed_chain() {
        let events = chain(4);
        let ok = verify_chain(&events).unwrap();
        assert_eq!(ok.len, 4);
        assert_eq!(ok.head_seq, 3);
        assert_eq!(ok.head_hash, events[3].hash);
        assert!(!ok.sealed);
    }

    #[test]
    fn reports_a_sealed_chain() {
        let mut events = chain(2);
        let mut last = RunEvent {
            seq: 2,
            ts: 1_757_635_200_002,
            kind: EventKind::RunFinished,
            content: Some("2".into()),
            prev_hash: events[1].hash,
            ..Default::default()
        };
        last.hash = last.core().hash(&events[1].hash);
        events.push(last);
        assert!(verify_chain(&events).unwrap().sealed);
    }

    #[test]
    fn rejects_an_empty_chain() {
        assert_eq!(verify_chain(&[]).unwrap_err(), ChainError::Empty);
    }

    #[test]
    fn rejects_a_chain_that_does_not_open_with_run_started() {
        let mut events = chain(2);
        events[0].kind = EventKind::Message;
        events[0].hash = events[0].core().hash(&ZERO);
        assert_eq!(
            verify_chain(&events).unwrap_err(),
            ChainError::NotRunStarted
        );
    }

    #[test]
    fn rejects_a_nonzero_opening_prev_hash() {
        let mut events = chain(1);
        events[0].prev_hash = Hash::of(b"forged");
        events[0].hash = events[0].core().hash(&events[0].prev_hash);
        assert_eq!(
            verify_chain(&events).unwrap_err(),
            ChainError::PrevMismatch {
                seq: 0,
                expected: ZERO,
                found: Hash::of(b"forged")
            }
        );
    }

    #[test]
    fn detects_a_missing_event() {
        // Deleting from the middle: the classic tampering attempt.
        let mut events = chain(4);
        events.remove(2);
        assert_eq!(
            verify_chain(&events).unwrap_err(),
            ChainError::SeqGap {
                expected: 2,
                found: 3
            }
        );
    }

    #[test]
    fn detects_edited_content() {
        let mut events = chain(3);
        events[1].content = Some("tampered".into());
        match verify_chain(&events).unwrap_err() {
            ChainError::HashMismatch { seq, .. } => assert_eq!(seq, 1),
            other => panic!("expected HashMismatch, got {other:?}"),
        }
    }

    #[test]
    fn detects_a_rewritten_link() {
        let mut events = chain(3);
        events[2].prev_hash = Hash::of(b"elsewhere");
        events[2].hash = events[2].core().hash(&events[2].prev_hash);
        match verify_chain(&events).unwrap_err() {
            ChainError::PrevMismatch { seq, .. } => assert_eq!(seq, 2),
            other => panic!("expected PrevMismatch, got {other:?}"),
        }
    }

    #[test]
    fn accepts_a_chain_containing_a_redaction() {
        // Erasing content keeps the digest, so the chain still verifies.
        let mut events = chain(3);
        let original = Hash::of(b"event 1");
        events[1].content = None;
        events[1].redacted = Some([("content".to_string(), original)].into_iter().collect());
        assert!(verify_chain(&events).is_ok());
    }

    #[test]
    fn a_truncated_chain_still_verifies() {
        // Documents the known limit that signed checkpoints exist to close.
        let mut events = chain(5);
        events.truncate(3);
        assert!(verify_chain(&events).is_ok());
    }
}
