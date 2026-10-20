package tideline_test

import (
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"os"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	tideline "github.com/dylanp12/tideline/sdk/go"
)

var counter atomic.Int64

func newRun(t *testing.T, c *tideline.Client) *tideline.Run {
	t.Helper()
	run, err := c.StartRun(ctx(t), tideline.NewRun{
		RunID:      fmt.Sprintf("go-%d-%d", os.Getpid(), counter.Add(1)),
		Agent:      tideline.Agent{Name: "underwriter", Version: "2.1.0"},
		SubjectRef: "applicant-4821",
		Labels:     map[string]string{"product": "personal-loan"},
	})
	if err != nil {
		t.Fatalf("start run: %v", err)
	}
	return run
}

func TestRecordsARunAndVerifiesIt(t *testing.T) {
	c := tideline.New(startServer(t), "")
	run := newRun(t, c)

	if _, err := run.Record(ctx(t), tideline.NewEvent{
		Kind:     tideline.ToolCall,
		Role:     "tool",
		Name:     "pull_credit_report",
		Content:  "score=690",
		Metadata: map[string]any{"bureau": "experian", "ms": 410},
	}); err != nil {
		t.Fatalf("record: %v", err)
	}
	if _, err := run.Complete(ctx(t)); err != nil {
		t.Fatalf("complete: %v", err)
	}

	events, chain, err := run.VerifiedEvents(ctx(t))
	if err != nil {
		t.Fatalf("verify: %v", err)
	}
	if !chain.Sealed || chain.Len != 3 {
		t.Errorf("chain = %+v", chain)
	}
	if events[1].Kind != tideline.ToolCall {
		t.Errorf("kind = %s", events[1].Kind)
	}
	// The metadata survived byte for byte, which is the only reason it verifies.
	if got := string(events[1].Metadata); got != `{"bureau":"experian","ms":410}` {
		t.Errorf("metadata = %s", got)
	}
}

func TestIdempotentRetryAppendsOnce(t *testing.T) {
	c := tideline.New(startServer(t), "")
	run := newRun(t, c)

	a, err := run.Record(ctx(t), tideline.NewEvent{Kind: tideline.Message, Content: "once"}, "k-1")
	if err != nil {
		t.Fatal(err)
	}
	b, err := run.Record(ctx(t), tideline.NewEvent{Kind: tideline.Message, Content: "once"}, "k-1")
	if err != nil {
		t.Fatal(err)
	}
	if a.Seq != b.Seq || a.Hash != b.Hash {
		t.Errorf("a=%+v b=%+v", a, b)
	}
	events, err := run.Events(ctx(t), 0, 0)
	if err != nil || len(events) != 2 {
		t.Errorf("events = %d, err = %v", len(events), err)
	}
}

func TestGateBlocksUntilAReviewerDecides(t *testing.T) {
	c := tideline.New(startServer(t), "")
	run := newRun(t, c)

	// A reviewer turns up a moment later, as one does.
	go func() {
		reviewer := c.Run(run.ID)
		for i := 0; i < 100; i++ {
			gates, err := reviewer.Approvals(ctx(t))
			if err == nil && len(gates) > 0 {
				_ = reviewer.Resolve(ctx(t), gates[0].Seq, tideline.Approved, "Jane Okafor", "within policy")
				return
			}
			time.Sleep(20 * time.Millisecond)
		}
	}()

	decision, err := run.Gate(ctx(t), "Approve EUR40,000 loan", time.Minute)
	if err != nil {
		t.Fatalf("gate: %v", err)
	}
	if !decision.Approved() || decision.Reviewer != "Jane Okafor" {
		t.Errorf("decision = %+v", decision)
	}
	// No control plane configured, so the server cannot vouch for who that was.
	if decision.Attested {
		t.Error("attested must be false without an authenticated reviewer")
	}
}

func TestRejectionIsAValueNotAnError(t *testing.T) {
	// An agent must be able to branch on a refusal.
	c := tideline.New(startServer(t), "")
	run := newRun(t, c)

	seq, err := run.OpenGate(ctx(t), "Approve EUR900,000", time.Minute)
	if err != nil {
		t.Fatal(err)
	}
	if err := run.Resolve(ctx(t), seq, tideline.Rejected, "Risk", "exceeds mandate"); err != nil {
		t.Fatal(err)
	}

	decision, err := run.AwaitGate(ctx(t), seq)
	if err != nil {
		t.Fatalf("await: %v", err)
	}
	if decision.Approved() || decision.Decision != tideline.Rejected {
		t.Errorf("decision = %+v", decision)
	}
}

func TestSecondResolutionIsAConflict(t *testing.T) {
	c := tideline.New(startServer(t), "")
	run := newRun(t, c)

	seq, err := run.OpenGate(ctx(t), "act", time.Minute)
	if err != nil {
		t.Fatal(err)
	}
	if err := run.Resolve(ctx(t), seq, tideline.Approved, "", ""); err != nil {
		t.Fatal(err)
	}

	err = run.Resolve(ctx(t), seq, tideline.Rejected, "", "")
	var apiErr *tideline.Error
	if !errors.As(err, &apiErr) || !apiErr.IsConflict() {
		t.Fatalf("expected a conflict, got %v", err)
	}
}

func TestTamperingIsDetectedAndLocated(t *testing.T) {
	c := tideline.New(startServer(t), "")
	run := newRun(t, c)
	for i := 0; i < 4; i++ {
		if _, err := run.Record(ctx(t), tideline.NewEvent{
			Kind: tideline.Message, Content: fmt.Sprintf("m%d", i),
		}); err != nil {
			t.Fatal(err)
		}
	}

	events, err := run.Events(ctx(t), 0, 0)
	if err != nil {
		t.Fatal(err)
	}
	altered := "altered after the fact"
	events[3].Content = &altered

	_, err = tideline.VerifyChain(events)
	var chainErr *tideline.ChainError
	if !errors.As(err, &chainErr) {
		t.Fatalf("expected a ChainError, got %v", err)
	}
	if chainErr.Reason != "hash_mismatch" || chainErr.Seq != 3 {
		t.Errorf("err = %+v", chainErr)
	}
}

func TestRedactionKeepsTheRecordVerifiable(t *testing.T) {
	c := tideline.New(startServer(t), "")
	run := newRun(t, c)

	if _, err := run.Record(ctx(t), tideline.NewEvent{
		Kind: tideline.ToolCall, Name: "kyc", Content: "applicant dossier",
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := run.Redact(ctx(t), 1, []string{"content"}, "GDPR Art 17 request #55"); err != nil {
		t.Fatalf("redact: %v", err)
	}

	events, _, err := run.VerifiedEvents(ctx(t))
	if err != nil {
		t.Fatalf("verify after redaction: %v", err)
	}
	if events[1].Content != nil {
		t.Error("plaintext should be erased")
	}
	if _, ok := events[1].Redacted["content"]; !ok {
		t.Error("the digest that commits to it should be retained")
	}
}

func TestCheckpointOnSealMatchesThePublishedKey(t *testing.T) {
	c := tideline.New(startServer(t), "")
	run := newRun(t, c)

	cp, err := run.Checkpoint(ctx(t))
	if err != nil || cp != nil {
		t.Fatalf("no checkpoint expected before seal: %v %v", cp, err)
	}
	if _, err := run.Complete(ctx(t)); err != nil {
		t.Fatal(err)
	}

	cp, err = run.Checkpoint(ctx(t))
	if err != nil || cp == nil {
		t.Fatalf("checkpoint after seal: %v %v", cp, err)
	}
	doc, err := c.WellKnown(ctx(t))
	if err != nil {
		t.Fatal(err)
	}
	if cp.KeyID != doc.Keys[0].ID {
		t.Errorf("key id: %s vs %s", cp.KeyID, doc.Keys[0].ID)
	}
}

func TestWatchStreamsTheRecord(t *testing.T) {
	c := tideline.New(startServer(t), "")
	run := newRun(t, c)
	if _, err := run.Record(ctx(t), tideline.NewEvent{Kind: tideline.Message, Content: "first"}); err != nil {
		t.Fatal(err)
	}

	events, errs := run.Watch(ctx(t), 0)
	var seen []tideline.Event
	timeout := time.After(10 * time.Second)
	for len(seen) < 2 {
		select {
		case e, ok := <-events:
			if !ok {
				t.Fatal("stream closed early")
			}
			seen = append(seen, e)
		case err := <-errs:
			t.Fatalf("watch: %v", err)
		case <-timeout:
			t.Fatal("watch timed out")
		}
	}
	if seen[0].Kind != tideline.RunStarted || *seen[1].Content != "first" {
		t.Errorf("seen = %+v", seen)
	}
}

func TestListRunsFilters(t *testing.T) {
	c := tideline.New(startServer(t), "")
	run := newRun(t, c)

	page, err := c.ListRuns(ctx(t), map[string]string{"agent": "underwriter"})
	if err != nil {
		t.Fatal(err)
	}
	found := false
	for _, e := range page.Runs {
		if e.RunID == run.ID {
			found = true
		}
	}
	if !found {
		t.Errorf("run %s not listed", run.ID)
	}

	none, err := c.ListRuns(ctx(t), map[string]string{"agent": "no-such-agent"})
	if err != nil || len(none.Runs) != 0 {
		t.Errorf("expected no runs, got %d (%v)", len(none.Runs), err)
	}
}

func TestUnknownRunIsNotFound(t *testing.T) {
	c := tideline.New(startServer(t), "")
	_, err := c.Run("nope-nope").Envelope(ctx(t))
	var apiErr *tideline.Error
	if !errors.As(err, &apiErr) || !apiErr.IsNotFound() {
		t.Fatalf("expected not found, got %v", err)
	}
}

func TestGoAlwaysSendsCompactMetadata(t *testing.T) {
	// encoding/json compacts the output of a Marshaler, so a Go client cannot
	// send metadata with insignificant whitespace even when handed a
	// json.RawMessage containing it.
	//
	// This is harmless, and worth pinning so nobody mistakes it for a bug when
	// comparing a Go-written record with one written from TypeScript or Python.
	// The server hashes the bytes that arrive; what a client sent before its own
	// encoder touched it is not part of the protocol.
	c := tideline.New(startServer(t), "")
	run := newRun(t, c)

	for _, spacing := range []string{`{"z":1,"a":2}`, `{ "z" : 1 , "a" : 2 }`} {
		var raw json.RawMessage = []byte(spacing)
		if _, err := run.Record(ctx(t), tideline.NewEvent{
			Kind: tideline.ModelCall, Metadata: raw,
		}); err != nil {
			t.Fatalf("record %s: %v", spacing, err)
		}
	}

	events, chain, err := run.VerifiedEvents(ctx(t))
	if err != nil {
		t.Fatalf("verify: %v", err)
	}
	if chain.Len != 3 {
		t.Fatalf("len = %d", chain.Len)
	}
	// Both arrived compacted, and both verify — which is the property that
	// matters. Verification is against what the server received.
	for i := 1; i <= 2; i++ {
		if got := string(events[i].Metadata); got != `{"z":1,"a":2}` {
			t.Errorf("event %d metadata = %s", i, got)
		}
	}
}

func TestARecordFromAnyClientVerifies(t *testing.T) {
	// The protocol commits to bytes, not to a normalisation of them. A record
	// written with whitespace by another client still verifies here, which is
	// what makes cross-language verification meaningful at all.
	c := tideline.New(startServer(t), "")
	run := newRun(t, c)

	// Bypass the typed path to send exactly these bytes, as a Python or
	// TypeScript client would.
	for _, body := range []string{
		`{"kind":"model_call","metadata":{"z":1,"a":2}}`,
		`{"kind":"model_call","metadata":{ "z" : 1 , "a" : 2 }}`,
	} {
		req, err := http.NewRequest(http.MethodPost,
			c.Base+"/v1/runs/"+run.ID+"/events", strings.NewReader(body))
		if err != nil {
			t.Fatal(err)
		}
		req.Header.Set("content-type", "application/json")
		res, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		res.Body.Close()
		if res.StatusCode != 200 {
			t.Fatalf("status %d for %s", res.StatusCode, body)
		}
	}

	events, chain, err := run.VerifiedEvents(ctx(t))
	if err != nil {
		t.Fatalf("a record with whitespace must still verify: %v", err)
	}
	if chain.Len != 3 {
		t.Fatalf("len = %d", chain.Len)
	}
	if got := string(events[2].Metadata); got != `{ "z" : 1 , "a" : 2 }` {
		t.Errorf("whitespace should survive to the verifier, got %s", got)
	}
}
