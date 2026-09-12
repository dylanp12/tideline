package tideline_test

import (
	"context"
	"math/rand"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"
)

// startServer runs the reference server on a free port.
//
// The SDK is tested against the real server, not a mock. A mock would agree
// with whatever the SDK believes, which is exactly the belief under test.
func startServer(t *testing.T) string {
	t.Helper()

	bin, err := filepath.Abs(filepath.Join("..", "..", "target", "debug", "tideline-server"))
	if err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(bin); err != nil {
		t.Skipf("%s is missing — run `cargo build --bin tideline-server` first", bin)
	}

	port := 9000 + rand.Intn(900)
	cmd := exec.Command(bin)
	// TIDELINE_EPHEMERAL_RECORDS: the server refuses an in-memory record store
	// unless told that is intended, because losing the record is catastrophic in
	// production — and exactly what a test wants.
	cmd.Env = append(os.Environ(),
		"TIDELINE_PORT="+itoa(port),
		"RUST_LOG=warn",
		"TIDELINE_EPHEMERAL_RECORDS=1",
	)
	if err := cmd.Start(); err != nil {
		t.Fatalf("start server: %v", err)
	}
	t.Cleanup(func() {
		_ = cmd.Process.Kill()
		_, _ = cmd.Process.Wait()
	})

	base := "http://127.0.0.1:" + itoa(port)
	for i := 0; i < 200; i++ {
		res, err := http.Get(base + "/health")
		if err == nil {
			res.Body.Close()
			if res.StatusCode == 200 {
				return base
			}
		}
		time.Sleep(25 * time.Millisecond)
	}
	t.Fatalf("server did not become healthy on %s", base)
	return ""
}

func itoa(n int) string {
	if n == 0 {
		return "0"
	}
	var b [8]byte
	i := len(b)
	for n > 0 {
		i--
		b[i] = byte('0' + n%10)
		n /= 10
	}
	return string(b[i:])
}

func ctx(t *testing.T) context.Context {
	c, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	t.Cleanup(cancel)
	return c
}
