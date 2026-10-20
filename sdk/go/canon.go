package tideline

import (
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"fmt"
)

// CanonLen is the fixed width of a canonical event: a 5-byte domain tag, two
// big-endian uint64s, and five 32-byte field digests.
const CanonLen = 181

var domain = []byte("tlr1\n")

var zero [32]byte

// ZeroHash is the prev_hash of event 0, and the digest of a field that was
// never set.
const ZeroHash = "0000000000000000000000000000000000000000000000000000000000000000"

func sha(b []byte) [32]byte { return sha256.Sum256(b) }

// digest resolves one field: present → SHA-256 of the value; erased → the
// retained digest; never set → zero.
//
// A present value always wins, so a forged Redacted map cannot restate the hash
// of an event that still carries its content.
func (e *Event) digest(field string, value *string) [32]byte {
	if value != nil {
		return sha([]byte(*value))
	}
	if kept, ok := e.Redacted[field]; ok && kept != "" {
		raw, err := hex.DecodeString(kept)
		if err == nil && len(raw) == 32 {
			var out [32]byte
			copy(out[:], raw)
			return out
		}
	}
	return zero
}

// Canon returns the canonical 181-byte encoding of the event.
func (e *Event) Canon() []byte {
	out := make([]byte, 0, CanonLen)
	out = append(out, domain...)
	out = binary.BigEndian.AppendUint64(out, e.Seq)
	out = binary.BigEndian.AppendUint64(out, e.TS)

	k := sha([]byte(e.Kind))
	out = append(out, k[:]...)

	var metadata *string
	if len(e.Metadata) > 0 && string(e.Metadata) != "null" {
		s := string(e.Metadata)
		metadata = &s
	}
	for _, f := range []struct {
		name  string
		value *string
	}{
		{"role", e.Role},
		{"name", e.Name},
		{"content", e.Content},
		{"metadata", metadata},
	} {
		d := e.digest(f.name, f.value)
		out = append(out, d[:]...)
	}

	if len(out) != CanonLen {
		panic(fmt.Sprintf("canon is %d bytes, expected %d", len(out), CanonLen))
	}
	return out
}

// ComputeHash returns the event's hash, committing to its predecessor.
func (e *Event) ComputeHash(prevHashHex string) (string, error) {
	prev, err := hex.DecodeString(prevHashHex)
	if err != nil || len(prev) != 32 {
		return "", fmt.Errorf("prev_hash must be 64 hex characters, got %q", prevHashHex)
	}
	h := sha256.New()
	h.Write(e.Canon())
	h.Write(prev)
	return hex.EncodeToString(h.Sum(nil)), nil
}
