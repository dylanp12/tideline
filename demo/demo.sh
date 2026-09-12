#!/usr/bin/env bash
# Scripted narrative for the README GIF: an answer streams into a stream, the
# connection drops mid-way, and a reconnect resumes exactly. Drives a real engine
# on :8080 (started by record.sh). Run via record.sh, not directly.
B=http://127.0.0.1:8080
S1=$(mktemp)
G=$'\033[32m'; D=$'\033[2m'; C=$'\033[36m'; Y=$'\033[33m'; R=$'\033[31m'; BO=$'\033[1m'; X=$'\033[0m'
p(){ printf '%s\n' "$1"; }
# green words (live); drop SSE id/event lines and blank separators
s1(){ sed -u -e '/^id:/d' -e '/^event:/d' -e '/^data: *$/d' -e '/^[[:space:]]*$/d' -e "s/^data: /   ${G}»${X} /"; }
# cyan words (resume) + turn the terminal 'done' event into a success line
s2(){ sed -u -e "s/^event: done.*/   ${G}✓ resumed — no gap, no repeat${X}/" \
              -e '/^id:/d' -e '/^event:/d' -e '/^data: *$/d' -e '/^[[:space:]]*$/d' -e "s/^data: /   ${C}»${X} /"; }

p "${BO}${C}tideline${X} ${D}— resumable streaming for AI output${X}"
sleep 1.0
p ""
p "${D}# an LLM streams an answer into stream 'chat' …${X}"
sleep 0.6

# paced publisher (one word per 0.2s), then mark the stream complete
(
  for w in "The " "Eiffel " "Tower " "stands " "in " "Paris, " "France."; do
    curl -s -XPOST "$B/streams/chat" --data "$w" >/dev/null
    sleep 0.20
  done
  curl -s -XPOST "$B/streams/chat/complete" >/dev/null
) &
PUB=$!

sleep 0.15
p "${D}\$${X} ${BO}curl -N $B/streams/chat${X}"
sleep 0.25
timeout 0.5 curl -sN "$B/streams/chat" | tee "$S1" | s1
LAST=$(grep '^id:' "$S1" | tail -1 | cut -d' ' -f2); [ -z "$LAST" ] && LAST=0
sleep 0.2
p "   ${R}✕ connection lost${X} ${D}(last offset seen: ${LAST})${X}"
sleep 0.7
wait $PUB 2>/dev/null

p ""
p "${D}# reconnect — the browser resends Last-Event-ID, resume is exact${X}"
sleep 0.5
p "${D}\$${X} ${BO}curl -N -H 'Last-Event-ID: ${LAST}' $B/streams/chat${X}"
sleep 0.3
curl -sN -H "Last-Event-ID: $LAST" "$B/streams/chat" | s2
sleep 1.1
rm -f "$S1"
