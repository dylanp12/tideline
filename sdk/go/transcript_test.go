package tideline_test

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	tideline "github.com/dylanp12/tideline/sdk/go"
)

type step struct {
	Name    string `json:"name"`
	Repeat  int    `json:"repeat"`
	Request struct {
		Method  string            `json:"method"`
		Path    string            `json:"path"`
		Headers map[string]string `json:"headers"`
		Body    json.RawMessage   `json:"body"`
	} `json:"request"`
	Expect struct {
		Status        *int                       `json:"status"`
		HeaderEq      map[string]string          `json:"header_eq"`
		JSONHas       []string                   `json:"json_has"`
		JSONEq        map[string]json.RawMessage `json:"json_eq"`
		ArrayLen      *int                       `json:"array_len"`
		ChainVerifies bool                       `json:"chain_verifies"`
	} `json:"expect"`
	Capture map[string]string `json:"capture"`
}

// substString replaces $VAR occurrences inside a string.
func substString(s string, vars map[string]any) string {
	for k, v := range vars {
		s = strings.ReplaceAll(s, "$"+k, fmt.Sprint(v))
	}
	return s
}

// substJSON replaces $VAR inside a JSON document. A string that is exactly
// "$VAR" takes the variable's JSON value, so a captured number stays a number.
func substJSON(raw json.RawMessage, vars map[string]any) json.RawMessage {
	if len(raw) == 0 {
		return raw
	}
	var value any
	if err := json.Unmarshal(raw, &value); err != nil {
		return raw
	}
	replaced, _ := json.Marshal(substValue(value, vars))
	return replaced
}

func substValue(v any, vars map[string]any) any {
	switch t := v.(type) {
	case string:
		if strings.HasPrefix(t, "$") {
			if replacement, ok := vars[t[1:]]; ok {
				return replacement
			}
		}
		return substString(t, vars)
	case []any:
		out := make([]any, len(t))
		for i, e := range t {
			out[i] = substValue(e, vars)
		}
		return out
	case map[string]any:
		out := make(map[string]any, len(t))
		for k, e := range t {
			out[k] = substValue(e, vars)
		}
		return out
	}
	return v
}

func TestTranscriptPasses(t *testing.T) {
	base := startServer(t)

	raw, err := os.ReadFile(filepath.Join("..", "..", "conformance", "transcript", "basic.json"))
	if err != nil {
		t.Fatalf("read transcript: %v", err)
	}
	var script struct {
		Steps []step `json:"steps"`
	}
	if err := json.Unmarshal(raw, &script); err != nil {
		t.Fatalf("parse transcript: %v", err)
	}

	vars := map[string]any{"RUN": fmt.Sprintf("go-conformance-%d", os.Getpid())}

	for _, s := range script.Steps {
		path := substString(s.Request.Path, vars)
		repeat := s.Repeat
		if repeat == 0 {
			repeat = 1
		}

		var status int
		var headers http.Header
		var text string
		for i := 0; i < repeat; i++ {
			var body io.Reader
			if len(s.Request.Body) > 0 {
				body = bytes.NewReader(substJSON(s.Request.Body, vars))
			}
			req, err := http.NewRequest(s.Request.Method, base+path, body)
			if err != nil {
				t.Fatalf("%s: %v", s.Name, err)
			}
			if body != nil {
				req.Header.Set("content-type", "application/json")
			}
			for k, v := range s.Request.Headers {
				req.Header.Set(k, v)
			}
			res, err := http.DefaultClient.Do(req)
			if err != nil {
				t.Fatalf("%s: %v", s.Name, err)
			}
			out, _ := io.ReadAll(res.Body)
			res.Body.Close()
			status, headers, text = res.StatusCode, res.Header, string(out)
		}

		if s.Expect.Status != nil && status != *s.Expect.Status {
			t.Fatalf("%s: status %d, want %d. Body: %s", s.Name, status, *s.Expect.Status, text)
		}
		for k, want := range s.Expect.HeaderEq {
			if got := headers.Get(k); got != want {
				t.Errorf("%s: header %s = %q, want %q", s.Name, k, got, want)
			}
		}

		needsBody := len(s.Expect.JSONHas) > 0 || len(s.Expect.JSONEq) > 0 ||
			s.Expect.ArrayLen != nil || s.Expect.ChainVerifies || len(s.Capture) > 0
		if !needsBody {
			continue
		}

		var body any
		if err := json.Unmarshal([]byte(text), &body); err != nil {
			t.Fatalf("%s: body is not JSON: %v (%s)", s.Name, err, text)
		}
		object, _ := body.(map[string]any)

		for _, k := range s.Expect.JSONHas {
			if _, ok := object[k]; !ok {
				t.Errorf("%s: missing key %s in %s", s.Name, k, text)
			}
		}
		for k, wantRaw := range s.Expect.JSONEq {
			var want any
			_ = json.Unmarshal(substJSON(wantRaw, vars), &want)
			if !reflect.DeepEqual(object[k], want) {
				t.Errorf("%s: %s = %v, want %v", s.Name, k, object[k], want)
			}
		}
		if s.Expect.ArrayLen != nil {
			list, ok := body.([]any)
			if !ok {
				t.Fatalf("%s: expected an array, got %s", s.Name, text)
			}
			if len(list) != *s.Expect.ArrayLen {
				t.Errorf("%s: array length %d, want %d", s.Name, len(list), *s.Expect.ArrayLen)
			}
		}
		if s.Expect.ChainVerifies {
			// From the response text, never from the parsed body.
			var events []tideline.Event
			if err := json.Unmarshal([]byte(text), &events); err != nil {
				t.Fatalf("%s: not a record: %v", s.Name, err)
			}
			chain, err := tideline.VerifyChain(events)
			if err != nil {
				t.Fatalf("%s: chain does not verify: %v", s.Name, err)
			}
			if !chain.Sealed {
				t.Errorf("%s: expected a sealed record", s.Name)
			}
		}
		for v, field := range s.Capture {
			vars[v] = object[field]
		}
	}
}
