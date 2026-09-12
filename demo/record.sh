#!/usr/bin/env bash
# Regenerate demo/tideline.gif — a real capture of the engine resuming a dropped
# stream. Boots the in-memory engine on :8080, records the scripted demo with
# asciinema, and renders the GIF with agg.
#
# Requires: cargo, asciinema (pipx install asciinema), agg
# (cargo install --git https://github.com/asciinema/agg), and a free :8080.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
PORT=8080
CAST="$(mktemp --suffix=.cast)"

cargo build --manifest-path "$REPO/Cargo.toml" >/dev/null 2>&1
fuser -k ${PORT}/tcp 2>/dev/null; sleep 0.4
( cd "$REPO" && exec ./target/debug/tideline ) >/tmp/tideline-demo-engine.log 2>&1 &
EPID=$!
for _ in $(seq 1 40); do curl -s -o /dev/null http://127.0.0.1:$PORT/health && break; sleep 0.2; done

asciinema rec "$CAST" --overwrite --cols 82 --rows 18 -c "bash '$HERE/demo.sh'"

kill "$EPID" 2>/dev/null; fuser -k ${PORT}/tcp 2>/dev/null
agg --theme github-dark --font-size 18 --idle-time-limit 1.2 --last-frame-duration 2.5 "$CAST" "$HERE/tideline.gif"
rm -f "$CAST"
echo "wrote $HERE/tideline.gif"
