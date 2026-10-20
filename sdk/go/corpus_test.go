package tideline_test

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"testing"

	tideline "github.com/dylanp12/tideline/sdk/go"
)

func corpusPath(name string) string {
	return filepath.Join("..", "..", "conformance", "corpus", name)
}

type eventVector struct {
	Name        string `json:"name"`
	EventJSON   string `json:"event_json"`
	PrevHash    string `json:"prev_hash"`
	CanonSHA256 string `json:"canon_sha256"`
	Hash        string `json:"hash"`
}

func TestEventVectorsReproduce(t *testing.T) {
	raw, err := os.ReadFile(corpusPath("events.json"))
	if err != nil {
		t.Fatalf("read corpus: %v", err)
	}
	var corpus struct {
		Vectors []eventVector `json:"vectors"`
	}
	if err := json.Unmarshal(raw, &corpus); err != nil {
		t.Fatalf("parse corpus: %v", err)
	}
	if len(corpus.Vectors) < 8 {
		t.Fatalf("corpus shrank: %d vectors", len(corpus.Vectors))
	}

	for _, v := range corpus.Vectors {
		t.Run(v.Name, func(t *testing.T) {
			var e tideline.Event
			// From the vector's source text. json.RawMessage keeps the metadata
			// bytes exactly, which is why Go needs no span reader.
			if err := json.Unmarshal([]byte(v.EventJSON), &e); err != nil {
				t.Fatalf("parse event: %v", err)
			}
			canon := sha256.Sum256(e.Canon())
			if got := hex.EncodeToString(canon[:]); got != v.CanonSHA256 {
				t.Errorf("canon\n got %s\nwant %s", got, v.CanonSHA256)
			}
			got, err := e.ComputeHash(v.PrevHash)
			if err != nil {
				t.Fatalf("hash: %v", err)
			}
			if got != v.Hash {
				t.Errorf("hash\n got %s\nwant %s", got, v.Hash)
			}
		})
	}
}

func TestChainCasesReachTheirVerdict(t *testing.T) {
	raw, err := os.ReadFile(corpusPath("chains.json"))
	if err != nil {
		t.Fatalf("read corpus: %v", err)
	}
	var corpus struct {
		Cases []struct {
			Name       string `json:"name"`
			EventsJSON string `json:"events_json"`
			Expect     string `json:"expect"`
			AtSeq      *int64 `json:"at_seq"`
		} `json:"cases"`
	}
	if err := json.Unmarshal(raw, &corpus); err != nil {
		t.Fatalf("parse corpus: %v", err)
	}

	for _, c := range corpus.Cases {
		t.Run(c.Name, func(t *testing.T) {
			var events []tideline.Event
			if err := json.Unmarshal([]byte(c.EventsJSON), &events); err != nil {
				t.Fatalf("parse events: %v", err)
			}
			_, err := tideline.VerifyChain(events)
			if c.Expect == "ok" {
				if err != nil {
					t.Fatalf("should verify, got %v", err)
				}
				return
			}
			var chainErr *tideline.ChainError
			if !errors.As(err, &chainErr) {
				t.Fatalf("expected a ChainError, got %v", err)
			}
			if chainErr.Reason != c.Expect {
				t.Errorf("reason: got %q, want %q", chainErr.Reason, c.Expect)
			}
			if c.AtSeq != nil && chainErr.Seq != *c.AtSeq {
				t.Errorf("seq: got %d, want %d", chainErr.Seq, *c.AtSeq)
			}
		})
	}
}

func TestRawMessagePreservesMetadataBytes(t *testing.T) {
	// The property the whole Go SDK rests on: no span reader is needed because
	// json.RawMessage hands back the original bytes, spacing included.
	const src = `{"seq":1,"ts":2,"kind":"model_call","metadata":{ "z" : 1 , "a" : 2 }}`
	var e tideline.Event
	if err := json.Unmarshal([]byte(src), &e); err != nil {
		t.Fatal(err)
	}
	if got, want := string(e.Metadata), `{ "z" : 1 , "a" : 2 }`; got != want {
		t.Errorf("metadata\n got %s\nwant %s", got, want)
	}
}

func TestKeyOrderChangesTheHash(t *testing.T) {
	// Why a client must never re-serialise metadata: these two events are equal
	// as JSON documents and different as records.
	parse := func(src string) tideline.Event {
		var e tideline.Event
		if err := json.Unmarshal([]byte(src), &e); err != nil {
			t.Fatal(err)
		}
		return e
	}
	a := parse(`{"seq":1,"ts":2,"kind":"model_call","metadata":{"z":1,"a":2}}`)
	b := parse(`{"seq":1,"ts":2,"kind":"model_call","metadata":{"a":2,"z":1}}`)

	ha, _ := a.ComputeHash(tideline.ZeroHash)
	hb, _ := b.ComputeHash(tideline.ZeroHash)
	if ha == hb {
		t.Error("key order must change the hash")
	}
}

func TestUnknownKindIsInvalid(t *testing.T) {
	if tideline.Kind("wire_transfer").Valid() {
		t.Error("an unknown kind must not validate")
	}
	for _, k := range tideline.Kinds {
		if !k.Valid() {
			t.Errorf("%s should be valid", k)
		}
	}
}
