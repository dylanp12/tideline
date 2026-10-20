#!/usr/bin/env bash
#
# Run the TLR/1 conformance suite across every implementation.
#
# The point of four SDKs is that they agree. This is where that is checked:
# each surface reproduces the same corpus hashes and completes the same HTTP
# transcript against the same server. A protocol change that one implementation
# misses fails here rather than in someone's production record months later.
#
#   ./conformance/run-all.sh            all surfaces
#   ./conformance/run-all.sh rust ts    only those
#
# Surfaces whose toolchain is absent are skipped, not failed.

set -uo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"

BOLD=$'\033[1m'; DIM=$'\033[2m'; RED=$'\033[31m'; GREEN=$'\033[32m'; YELLOW=$'\033[33m'; OFF=$'\033[0m'

WANTED=("$@")
want() {
  [ ${#WANTED[@]} -eq 0 ] && return 0
  local s; for s in "${WANTED[@]}"; do [ "$s" = "$1" ] && return 0; done
  return 1
}

declare -a NAMES STATUSES DETAILS
FAILED=0
LOGDIR="$(mktemp -d)"
trap 'rm -rf "$LOGDIR"' EXIT

record() { NAMES+=("$1"); STATUSES+=("$2"); DETAILS+=("$3"); [ "$2" = "FAIL" ] && FAILED=1; return 0; }

run_surface() {
  # Separate lines: bash expands every argument to `local` before assigning any
  # of them, so `log` cannot refer to `name` on the same line.
  local name="$1"
  local log="$LOGDIR/$name.log"
  shift
  printf '%s▸ %s%s\n' "$DIM" "$name" "$OFF"
  if "$@" >"$log" 2>&1; then
    record "$name" "PASS" "$(summarise "$name" "$log")"
  else
    record "$name" "FAIL" "$(summarise "$name" "$log")"
    printf '%s' "$RED"; tail -25 "$log"; printf '%s' "$OFF"
  fi
}

# Pull a test count out of each toolchain's very different output.
summarise() {
  local name="$1"
  local log="$2"
  case "$name" in
    rust)   awk '/^test result:/ {n += $4} END {printf "%d tests", n}' "$log" ;;
    ts)     grep -oE 'Tests +[0-9]+ passed' "$log" | tail -1 | grep -oE '[0-9]+' | xargs -I{} echo "{} tests" ;;
    python) grep -oE '[0-9]+ passed' "$log" | tail -1 | sed 's/passed/tests/' ;;
    go)     grep -c -E '^(--- PASS|    --- PASS)' "$log" | xargs -I{} echo "{} tests" ;;
    *)      grep -oE '[0-9]+ vectors' "$log" | tail -1 ;;
  esac
}

echo "${BOLD}TLR/1 conformance${OFF}"
echo

# The SDK suites drive a real server, so build it once up front.
if want rust || want ts || want python || want go; then
  printf '%sbuilding the reference server…%s\n' "$DIM" "$OFF"
  if ! cargo build -q --bin tideline-server 2>"$LOGDIR/build.log"; then
    printf '%scannot build tideline-server:%s\n' "$RED" "$OFF"; cat "$LOGDIR/build.log"; exit 1
  fi
fi

want independent && run_surface independent python3 conformance/independent_check.py
want rust       && run_surface rust  cargo test --workspace --all-targets

if want ts; then
  if command -v pnpm >/dev/null && [ -d sdk/ts/node_modules ]; then
    run_surface ts bash -c "cd '$ROOT/sdk/ts' && pnpm vitest run"
  else
    record ts "SKIP" "pnpm install needed in sdk/ts"
  fi
fi

if want python; then
  PY="${TIDELINE_PYTHON:-python3}"
  if "$PY" -c 'import pytest' 2>/dev/null; then
    run_surface python env PYTHONPATH="$ROOT/sdk/python/src" "$PY" -m pytest sdk/python/tests -q
  else
    record python "SKIP" "pytest not installed (set TIDELINE_PYTHON to an interpreter that has it)"
  fi
fi

if want go; then
  if command -v go >/dev/null; then
    run_surface go bash -c "cd '$ROOT/sdk/go' && go test -v ./..."
  else
    record go "SKIP" "go toolchain not found"
  fi
fi

echo
printf '%s%-14s %-6s %s%s\n' "$BOLD" "surface" "result" "detail" "$OFF"
for i in "${!NAMES[@]}"; do
  case "${STATUSES[$i]}" in
    PASS) colour="$GREEN" ;;
    FAIL) colour="$RED" ;;
    *)    colour="$YELLOW" ;;
  esac
  printf '%-14s %s%-6s%s %s\n' "${NAMES[$i]}" "$colour" "${STATUSES[$i]}" "$OFF" "${DETAILS[$i]}"
done
echo

if [ "$FAILED" -ne 0 ]; then
  printf '%sImplementations disagree — see the output above.%s\n' "$RED" "$OFF"
  exit 1
fi
printf '%sEvery surface agrees on the same bytes.%s\n' "$GREEN" "$OFF"
