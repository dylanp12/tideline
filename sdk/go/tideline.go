// Package tideline is the official Go client for TLR/1 — the Tideline Record
// Protocol.
//
// Record what an AI agent did, gate its high-risk actions on human approval,
// and hand anyone a record they can verify without trusting you. EU AI Act
// Articles 12 and 14.
//
//	tl := tideline.New("http://localhost:8080", "")
//	run, err := tl.StartRun(ctx, tideline.NewRun{
//		RunID: "loan-4821",
//		Agent: tideline.Agent{Name: "underwriter", Version: "2.1.0"},
//	})
//
//	run.Record(ctx, tideline.Event{Kind: tideline.ToolCall, Name: "pull_credit_report"})
//
//	// Blocks until a person decides. Both halves are in the chain by then.
//	decision, err := run.Gate(ctx, "Approve EUR40,000 loan", 2*time.Hour)
//
// VerifyChain needs no server and no network.
package tideline

import "encoding/json"

// Kind is the kind of an event. These wire strings are hashed into canon, so
// they are part of the protocol and never change.
type Kind string

const (
	RunStarted  Kind = "run_started"
	Message     Kind = "message"
	ModelCall   Kind = "model_call"
	ToolCall    Kind = "tool_call"
	DecisionEvt Kind = "decision"
	Redaction   Kind = "redaction"
	RunFinished Kind = "run_finished"
)

// Kinds is every valid event kind.
var Kinds = []Kind{RunStarted, Message, ModelCall, ToolCall, DecisionEvt, Redaction, RunFinished}

// Valid reports whether k is a kind this protocol version defines. An unknown
// kind must be rejected rather than defaulted, or a writer could mislabel an
// event unnoticed.
func (k Kind) Valid() bool {
	for _, v := range Kinds {
		if v == k {
			return true
		}
	}
	return false
}

// Redactable lists the fields a redaction may erase. seq, ts and kind are
// structural: erasing them would destroy the record's shape, not its contents.
var Redactable = []string{"role", "name", "content", "metadata"}

// Event is one event on the record.
//
// Metadata is json.RawMessage on purpose: canon hashes the exact bytes the
// server sent, and any round trip through a decoded value would reorder keys
// and change the digest.
type Event struct {
	Seq      uint64          `json:"seq"`
	TS       uint64          `json:"ts"`
	Kind     Kind            `json:"kind"`
	Role     *string         `json:"role,omitempty"`
	Name     *string         `json:"name,omitempty"`
	Content  *string         `json:"content,omitempty"`
	Metadata json.RawMessage `json:"metadata,omitempty"`
	// Redacted holds the retained digests of erased fields, keyed by field name.
	Redacted map[string]string `json:"redacted,omitempty"`
	PrevHash string            `json:"prev_hash"`
	Hash     string            `json:"hash"`
}

// Agent identifies what produced a run.
type Agent struct {
	Name    string `json:"name"`
	Version string `json:"version"`
}

// Envelope is a run's identity, committed by the chain as the metadata of its
// run_started event.
type Envelope struct {
	RunID      string            `json:"run_id"`
	StartedTS  uint64            `json:"started_ts"`
	EndedTS    *uint64           `json:"ended_ts,omitempty"`
	Agent      Agent             `json:"agent"`
	SubjectRef *string           `json:"subject_ref,omitempty"`
	Labels     map[string]string `json:"labels,omitempty"`
	HeadSeq    uint64            `json:"head_seq"`
	HeadHash   string            `json:"head_hash"`
}

// Checkpoint is a server's signed assertion that a run's head was HeadHash at
// Seq. It closes the gap a hash chain leaves open: a chain cannot detect a
// truncated tail, because a valid prefix is a valid chain.
type Checkpoint struct {
	RunID    string `json:"run_id"`
	Seq      uint64 `json:"seq"`
	HeadHash string `json:"head_hash"`
	TS       uint64 `json:"ts"`
	KeyID    string `json:"key_id"`
	Sig      string `json:"sig"`
}

// Appended is what the server assigns when an event lands.
type Appended struct {
	Seq      uint64 `json:"seq"`
	TS       uint64 `json:"ts"`
	PrevHash string `json:"prev_hash"`
	Hash     string `json:"hash"`
}
