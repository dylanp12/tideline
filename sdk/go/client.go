package tideline

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"
)

// Error is the server answering, and saying no.
type Error struct {
	Status int
	Body   string
}

func (e *Error) Error() string { return fmt.Sprintf("server returned %d: %s", e.Status, e.Body) }

// IsConflict reports whether the record's state forbids the request: a sealed
// run, a gate already decided.
func (e *Error) IsConflict() bool { return e.Status == http.StatusConflict }

// IsNotFound reports whether the run, event or gate does not exist.
func (e *Error) IsNotFound() bool { return e.Status == http.StatusNotFound }

// IsUnauthorized reports whether the credential was missing or refused.
func (e *Error) IsUnauthorized() bool { return e.Status == http.StatusUnauthorized }

// Client talks to one TLR/1 server.
type Client struct {
	Base string
	Key  string
	HTTP *http.Client
}

// New returns a client. Pass an empty key for an unauthenticated server.
func New(baseURL, key string) *Client {
	return &Client{
		Base: strings.TrimRight(baseURL, "/"),
		Key:  key,
		HTTP: &http.Client{Timeout: 30 * time.Second},
	}
}

func (c *Client) do(ctx context.Context, method, path string, body any, headers map[string]string) (*http.Response, error) {
	var reader io.Reader
	if body != nil {
		encoded, err := json.Marshal(body)
		if err != nil {
			return nil, err
		}
		reader = bytes.NewReader(encoded)
	}
	req, err := http.NewRequestWithContext(ctx, method, c.Base+path, reader)
	if err != nil {
		return nil, err
	}
	if body != nil {
		req.Header.Set("content-type", "application/json")
	}
	if c.Key != "" {
		req.Header.Set("authorization", "Bearer "+c.Key)
	}
	for k, v := range headers {
		req.Header.Set(k, v)
	}

	res, err := c.HTTP.Do(req)
	if err != nil {
		return nil, err
	}
	if res.StatusCode < 200 || res.StatusCode >= 300 {
		defer res.Body.Close()
		text, _ := io.ReadAll(res.Body)
		return nil, &Error{Status: res.StatusCode, Body: string(text)}
	}
	return res, nil
}

func (c *Client) json(ctx context.Context, method, path string, body any, out any, headers map[string]string) error {
	res, err := c.do(ctx, method, path, body, headers)
	if err != nil {
		return err
	}
	defer res.Body.Close()
	if out == nil {
		_, err = io.Copy(io.Discard, res.Body)
		return err
	}
	return json.NewDecoder(res.Body).Decode(out)
}

// NewRun is what opening a run requires.
type NewRun struct {
	RunID string `json:"run_id"`
	Agent Agent  `json:"agent"`
	// SubjectRef is an opaque reference to the subject of the decision. Never
	// personal data: it is not redactable and it appears in list queries.
	SubjectRef string            `json:"subject_ref,omitempty"`
	Labels     map[string]string `json:"labels,omitempty"`
}

// StartRun opens a run. Its envelope becomes the first link in the chain.
func (c *Client) StartRun(ctx context.Context, spec NewRun) (*Run, error) {
	var env Envelope
	if err := c.json(ctx, http.MethodPost, "/v1/runs", spec, &env, nil); err != nil {
		return nil, err
	}
	return &Run{client: c, ID: env.RunID}, nil
}

// Run returns a handle to a run that already exists. It contacts nothing.
func (c *Client) Run(runID string) *Run { return &Run{client: c, ID: runID} }

// RunPage is a page of run envelopes.
type RunPage struct {
	Runs       []Envelope `json:"runs"`
	NextCursor string     `json:"next_cursor,omitempty"`
}

// ListRuns lists runs, filtered by any of agent, label ("key=value"), since,
// cursor and limit.
func (c *Client) ListRuns(ctx context.Context, query map[string]string) (RunPage, error) {
	values := url.Values{}
	for k, v := range query {
		if v != "" {
			values.Set(k, v)
		}
	}
	path := "/v1/runs"
	if encoded := values.Encode(); encoded != "" {
		path += "?" + encoded
	}
	var page RunPage
	return page, c.json(ctx, http.MethodGet, path, nil, &page, nil)
}

// WellKnown describes what a server supports and which keys sign its checkpoints.
type WellKnown struct {
	ProtocolVersions []int    `json:"protocol_versions"`
	Capabilities     []string `json:"capabilities"`
	Keys             []struct {
		ID        string `json:"id"`
		Alg       string `json:"alg"`
		PublicKey string `json:"public_key"`
	} `json:"keys"`
}

// WellKnown fetches the server's discovery document.
func (c *Client) WellKnown(ctx context.Context) (WellKnown, error) {
	var doc WellKnown
	return doc, c.json(ctx, http.MethodGet, "/v1/.well-known/tideline", nil, &doc, nil)
}

// Run is a handle to one run.
type Run struct {
	client *Client
	ID     string
}

func (r *Run) path(suffix string) string {
	return "/v1/runs/" + url.PathEscape(r.ID) + suffix
}

// Envelope returns the run's identity and current head.
func (r *Run) Envelope(ctx context.Context) (Envelope, error) {
	var env Envelope
	return env, r.client.json(ctx, http.MethodGet, r.path(""), nil, &env, nil)
}

// NewEvent is an event on its way to the record. Seq, TS and both hashes belong
// to the server; nothing here can set them.
type NewEvent struct {
	Kind     Kind   `json:"kind"`
	Role     string `json:"role,omitempty"`
	Name     string `json:"name,omitempty"`
	Content  string `json:"content,omitempty"`
	Metadata any    `json:"metadata,omitempty"`
}

// Record appends an event.
//
// Pass an idempotency key on anything you might retry. Without one, a retry
// after a timeout puts a duplicate into evidence permanently, and it verifies:
// the chain proves a record was not altered afterwards, not that it was right
// when written.
func (r *Run) Record(ctx context.Context, event NewEvent, idempotencyKey ...string) (Appended, error) {
	headers := map[string]string{}
	if len(idempotencyKey) > 0 && idempotencyKey[0] != "" {
		headers["idempotency-key"] = idempotencyKey[0]
	}
	var out Appended
	return out, r.client.json(ctx, http.MethodPost, r.path("/events"), event, &out, headers)
}

// Decision is how a gate ended.
type Decision string

const (
	Approved Decision = "approved"
	Rejected Decision = "rejected"
	// Expired means nobody decided in time. Recorded like any other outcome: a
	// record that simply stops is indistinguishable from a truncated one.
	Expired Decision = "expired"
)

// Gate is one approval gate, as the server projects it from the chain.
type Gate struct {
	Seq       uint64   `json:"seq"`
	Action    string   `json:"action"`
	ExpiresAt uint64   `json:"expires_at"`
	State     string   `json:"state"`
	Decision  Decision `json:"decision,omitempty"`
	Reviewer  string   `json:"reviewer,omitempty"`
	// ReviewerPrincipal is the authenticated identity, when the server has one.
	ReviewerPrincipal string `json:"reviewer_principal,omitempty"`
	// Attested reports whether the server authenticated the reviewer, rather
	// than taking their word for who they are.
	Attested bool   `json:"attested"`
	Note     string `json:"note,omitempty"`
}

// IsPending reports whether the gate is still awaiting a decision.
func (g Gate) IsPending() bool { return g.State == "pending" }

// Resolution is a gate's outcome once a person has decided.
type Resolution struct {
	Seq      uint64
	Decision Decision
	Reviewer string
	Note     string
	Attested bool
}

// Approved reports whether the action may proceed.
func (r Resolution) Approved() bool { return r.Decision == Approved }

type openedGate struct {
	Seq       uint64 `json:"seq"`
	ExpiresAt uint64 `json:"expires_at"`
}

// OpenGate opens a gate without waiting on it.
func (r *Run) OpenGate(ctx context.Context, action string, expiresIn time.Duration) (uint64, error) {
	body := map[string]any{"action": action}
	if expiresIn > 0 {
		body["expires_in"] = int64(expiresIn.Seconds())
	}
	var out openedGate
	err := r.client.json(ctx, http.MethodPost, r.path("/approvals"), body, &out, nil)
	return out.Seq, err
}

// Gate opens a gate and blocks until a person decides, or it expires.
//
// This is what puts oversight in the agent's path rather than beside it: when
// it returns, the request and the decision are both already in the chain.
//
// It polls rather than holding a stream open. A gate can sit for hours, and a
// poll survives a proxy idle timeout, a NAT rebind, and a process restart where
// a long-lived connection quietly does not. Watch is the tool for a live
// oversight view; this is the tool for a gate.
//
// A rejection returns a Resolution rather than an error: an agent must be able
// to branch on a refusal without treating the normal case as a failure.
func (r *Run) Gate(ctx context.Context, action string, expiresIn time.Duration) (Resolution, error) {
	seq, err := r.OpenGate(ctx, action, expiresIn)
	if err != nil {
		return Resolution{}, err
	}
	return r.AwaitGate(ctx, seq)
}

// AwaitGate blocks until gate seq is decided.
func (r *Run) AwaitGate(ctx context.Context, seq uint64) (Resolution, error) {
	delay := 250 * time.Millisecond
	for {
		gate, err := r.Approval(ctx, seq)
		if err != nil {
			return Resolution{}, err
		}
		if !gate.IsPending() {
			if gate.Decision == "" {
				return Resolution{}, fmt.Errorf("gate %d is resolved but carries no decision", seq)
			}
			return Resolution{
				Seq:      seq,
				Decision: gate.Decision,
				Reviewer: gate.Reviewer,
				Note:     gate.Note,
				Attested: gate.Attested,
			}, nil
		}
		select {
		case <-ctx.Done():
			return Resolution{}, ctx.Err()
		case <-time.After(delay):
		}
		// Back off to five seconds: nobody answers faster than that, and a tight
		// loop on a multi-hour gate is rude.
		if delay *= 2; delay > 5*time.Second {
			delay = 5 * time.Second
		}
	}
}

// Approvals returns the gates still awaiting a decision — the reviewer's queue.
func (r *Run) Approvals(ctx context.Context) ([]Gate, error) {
	var gates []Gate
	return gates, r.client.json(ctx, http.MethodGet, r.path("/approvals"), nil, &gates, nil)
}

// Approval returns one gate's current state.
func (r *Run) Approval(ctx context.Context, seq uint64) (Gate, error) {
	var gate Gate
	path := r.path("/approvals/" + strconv.FormatUint(seq, 10))
	return gate, r.client.json(ctx, http.MethodGet, path, nil, &gate, nil)
}

// Resolve records a person's decision on a gate.
func (r *Run) Resolve(ctx context.Context, seq uint64, decision Decision, reviewer, note string) error {
	path := r.path("/approvals/" + strconv.FormatUint(seq, 10) + "/resolve")
	body := map[string]any{"decision": string(decision), "reviewer": reviewer, "note": note}
	return r.client.json(ctx, http.MethodPost, path, body, nil, nil)
}

// Events returns the record from offset.
func (r *Run) Events(ctx context.Context, from uint64, limit int) ([]Event, error) {
	if limit <= 0 {
		limit = 5000
	}
	path := fmt.Sprintf("%s?from=%d&limit=%d", r.path("/events"), from, limit)
	var events []Event
	return events, r.client.json(ctx, http.MethodGet, path, nil, &events, nil)
}

// VerifiedEvents fetches the record and verifies it in one step.
func (r *Run) VerifiedEvents(ctx context.Context) ([]Event, ChainOK, error) {
	events, err := r.Events(ctx, 0, 0)
	if err != nil {
		return nil, ChainOK{}, err
	}
	ok, err := VerifyChain(events)
	return events, ok, err
}

// Watch follows the record live: history first, then events as they land. The
// channel closes when the stream ends or ctx is cancelled.
func (r *Run) Watch(ctx context.Context, from uint64) (<-chan Event, <-chan error) {
	events := make(chan Event)
	errs := make(chan error, 1)

	go func() {
		defer close(events)
		defer close(errs)

		path := fmt.Sprintf("%s?from=%d", r.path("/watch"), from)
		res, err := r.client.do(ctx, http.MethodGet, path, nil, map[string]string{"accept": "text/event-stream"})
		if err != nil {
			errs <- err
			return
		}
		defer res.Body.Close()

		scanner := bufio.NewScanner(res.Body)
		scanner.Buffer(make([]byte, 0, 64*1024), 16*1024*1024)
		var data strings.Builder
		for scanner.Scan() {
			line := scanner.Text()
			if line != "" {
				if rest, ok := strings.CutPrefix(line, "data:"); ok {
					data.WriteString(strings.TrimLeft(rest, " "))
				} else if name, ok := strings.CutPrefix(line, "event:"); ok {
					if n := strings.TrimSpace(name); n == "done" || n == "stream_error" {
						return
					}
				}
				continue
			}
			if data.Len() > 0 {
				var e Event
				// From the frame text, so the metadata bytes survive.
				if err := json.Unmarshal([]byte(data.String()), &e); err != nil {
					errs <- err
					return
				}
				select {
				case events <- e:
				case <-ctx.Done():
					return
				}
				data.Reset()
			}
		}
		if err := scanner.Err(); err != nil && ctx.Err() == nil {
			errs <- err
		}
	}()

	return events, errs
}

// Complete seals the run and writes a checkpoint.
func (r *Run) Complete(ctx context.Context) (Appended, error) {
	var out Appended
	return out, r.client.json(ctx, http.MethodPost, r.path("/complete"), nil, &out, nil)
}

// Checkpoint returns the latest signed checkpoint, or nil if none exists yet.
func (r *Run) Checkpoint(ctx context.Context) (*Checkpoint, error) {
	var cp Checkpoint
	err := r.client.json(ctx, http.MethodGet, r.path("/checkpoint"), nil, &cp, nil)
	if err != nil {
		var apiErr *Error
		if ok := asError(err, &apiErr); ok && apiErr.IsNotFound() {
			return nil, nil
		}
		return nil, err
	}
	return &cp, nil
}

// Redact erases fields of an earlier event, keeping the chain intact.
func (r *Run) Redact(ctx context.Context, targetSeq uint64, fields []string, authority string) (Appended, error) {
	body := map[string]any{"target_seq": targetSeq, "fields": fields, "authority": authority}
	var out Appended
	return out, r.client.json(ctx, http.MethodPost, r.path("/redactions"), body, &out, nil)
}

func asError(err error, target **Error) bool {
	if e, ok := err.(*Error); ok {
		*target = e
		return true
	}
	return false
}
