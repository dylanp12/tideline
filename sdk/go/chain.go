package tideline

import "fmt"

// ChainError is a record that does not verify, and where.
type ChainError struct {
	// Reason is one of: empty, not_run_started, seq_gap, prev_mismatch,
	// hash_mismatch. Shared with every other TLR/1 implementation.
	Reason string
	// Seq is the event at fault, or -1 when the record as a whole is at fault.
	Seq     int64
	Message string
}

func (e *ChainError) Error() string { return e.Message }

// ChainOK is what a valid record tells you.
type ChainOK struct {
	Len      int
	HeadSeq  uint64
	HeadHash string
	// Sealed is true when the record ends in run_finished.
	Sealed bool
}

// VerifyChain verifies an ordered run record end to end.
//
// It detects edited fields, inserted events, deleted events, and rewritten
// links, and names the event at fault. It cannot detect truncation of the tail —
// a valid prefix is a valid chain — which is what checkpoints are for. Check
// Sealed, and a checkpoint, before calling a record complete.
func VerifyChain(events []Event) (ChainOK, error) {
	if len(events) == 0 {
		return ChainOK{}, &ChainError{Reason: "empty", Seq: -1, Message: "the record is empty"}
	}
	if events[0].Kind != RunStarted {
		return ChainOK{}, &ChainError{
			Reason:  "not_run_started",
			Seq:     0,
			Message: fmt.Sprintf("a record must open with run_started, got %s", events[0].Kind),
		}
	}

	prev := ZeroHash
	for i := range events {
		e := &events[i]
		if e.Seq != uint64(i) {
			return ChainOK{}, &ChainError{
				Reason:  "seq_gap",
				Seq:     int64(i),
				Message: fmt.Sprintf("expected seq %d, found %d", i, e.Seq),
			}
		}
		if e.PrevHash != prev {
			return ChainOK{}, &ChainError{
				Reason:  "prev_mismatch",
				Seq:     int64(e.Seq),
				Message: fmt.Sprintf("event %d points at %s…, expected %s…", e.Seq, e.PrevHash[:16], prev[:16]),
			}
		}
		computed, err := e.ComputeHash(prev)
		if err != nil {
			return ChainOK{}, &ChainError{Reason: "prev_mismatch", Seq: int64(e.Seq), Message: err.Error()}
		}
		if computed != e.Hash {
			return ChainOK{}, &ChainError{
				Reason: "hash_mismatch",
				Seq:    int64(e.Seq),
				Message: fmt.Sprintf("event %d was altered: it hashes to %s…, but claims %s…",
					e.Seq, computed[:16], e.Hash[:16]),
			}
		}
		prev = computed
	}

	last := events[len(events)-1]
	return ChainOK{
		Len:      len(events),
		HeadSeq:  last.Seq,
		HeadHash: last.Hash,
		Sealed:   last.Kind == RunFinished,
	}, nil
}
