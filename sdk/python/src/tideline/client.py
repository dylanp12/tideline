"""The TLR/1 client.

Standard library only: ``urllib.request``. A compliance team installing a
verifier should not be installing a dependency tree with it.
"""

from __future__ import annotations

import json
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from typing import Any, Dict, Iterator, List, Optional, Sequence

from .canon import Event, parse_record
from .chain import ChainOk, verify_chain


class TidelineError(Exception):
    """The server answered, and said no."""

    def __init__(self, status: int, body: str) -> None:
        super().__init__(f"server returned {status}: {body}")
        self.status = status
        self.body = body

    @property
    def is_conflict(self) -> bool:
        """The record's state forbids this: a sealed run, a gate already decided."""
        return self.status == 409

    @property
    def is_not_found(self) -> bool:
        return self.status == 404

    @property
    def is_unauthorized(self) -> bool:
        return self.status == 401


@dataclass
class Appended:
    seq: int
    ts: int
    prev_hash: str
    hash: str


@dataclass
class Gate:
    seq: int
    action: str
    expires_at: int
    state: str
    decision: Optional[str] = None
    reviewer: Optional[str] = None
    reviewer_principal: Optional[str] = None
    attested: bool = False
    note: Optional[str] = None

    @property
    def is_pending(self) -> bool:
        return self.state == "pending"


@dataclass
class Resolution:
    seq: int
    decision: str
    reviewer: Optional[str]
    note: Optional[str]
    #: Whether the server authenticated the reviewer, rather than taking their
    #: word for who they are.
    attested: bool

    @property
    def approved(self) -> bool:
        return self.decision == "approved"


class Tideline:
    """A client for one TLR/1 server."""

    def __init__(self, url: str, key: Optional[str] = None, timeout: float = 30.0) -> None:
        self.base = url.rstrip("/")
        self.key = key
        self.timeout = timeout

    # --- transport --------------------------------------------------------

    def _request(
        self,
        method: str,
        path: str,
        body: Any = None,
        headers: Optional[Dict[str, str]] = None,
    ) -> str:
        data = None
        hdrs = dict(headers or {})
        if body is not None:
            # Compact separators. Correctness does not depend on this — the
            # server hashes whatever bytes arrive, and a record verifies against
            # those — but it keeps payloads small and makes a record produced by
            # this client read the same as one from any other.
            data = json.dumps(body, separators=(",", ":")).encode("utf-8")
            hdrs["content-type"] = "application/json"
        if self.key:
            hdrs["authorization"] = f"Bearer {self.key}"

        req = urllib.request.Request(f"{self.base}{path}", data=data, headers=hdrs, method=method)
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as res:
                return res.read().decode("utf-8")
        except urllib.error.HTTPError as e:
            raise TidelineError(e.code, e.read().decode("utf-8", "replace")) from None

    def _json(self, method: str, path: str, body: Any = None, **kw: Any) -> Any:
        return json.loads(self._request(method, path, body, **kw) or "null")

    # --- runs -------------------------------------------------------------

    def start_run(
        self,
        run_id: str,
        agent_name: str,
        agent_version: str,
        subject_ref: Optional[str] = None,
        labels: Optional[Dict[str, str]] = None,
    ) -> "Run":
        """Open a run. Its envelope becomes the first link in the chain.

        ``subject_ref`` is an opaque reference. Never personal data: it is not
        redactable and it appears in list queries.
        """
        body: Dict[str, Any] = {
            "run_id": run_id,
            "agent": {"name": agent_name, "version": agent_version},
        }
        if subject_ref is not None:
            body["subject_ref"] = subject_ref
        if labels:
            body["labels"] = labels
        env = self._json("POST", "/v1/runs", body)
        return Run(self, env["run_id"])

    def run(self, run_id: str) -> "Run":
        """A handle to a run that already exists. Contacts nothing."""
        return Run(self, run_id)

    def list_runs(self, **query: Any) -> Dict[str, Any]:
        params = {k: str(v) for k, v in query.items() if v is not None}
        suffix = f"?{urllib.parse.urlencode(params)}" if params else ""
        return self._json("GET", f"/v1/runs{suffix}")

    def well_known(self) -> Dict[str, Any]:
        """Capabilities, supported protocol versions, and checkpoint keys."""
        return self._json("GET", "/v1/.well-known/tideline")


class Run:
    """A handle to one run."""

    def __init__(self, client: Tideline, run_id: str) -> None:
        self.client = client
        self.id = run_id

    def _path(self, suffix: str = "") -> str:
        return f"/v1/runs/{urllib.parse.quote(self.id)}{suffix}"

    def envelope(self) -> Dict[str, Any]:
        return self.client._json("GET", self._path())

    def record(
        self,
        kind: str,
        role: Optional[str] = None,
        name: Optional[str] = None,
        content: Optional[str] = None,
        metadata: Any = None,
        idempotency_key: Optional[str] = None,
    ) -> Appended:
        """Append an event.

        Pass ``idempotency_key`` on anything you might retry. Without one, a
        retry after a timeout puts a duplicate into evidence permanently, and it
        verifies — the chain proves a record was not altered afterwards, not
        that it was right when written.
        """
        body: Dict[str, Any] = {"kind": kind}
        for k, v in (("role", role), ("name", name), ("content", content)):
            if v is not None:
                body[k] = v
        if metadata is not None:
            body["metadata"] = metadata
        headers = {"idempotency-key": idempotency_key} if idempotency_key else None
        return Appended(**self.client._json("POST", self._path("/events"), body, headers=headers))

    # --- gates ------------------------------------------------------------

    def open_gate(self, action: str, expires_in: Optional[int] = None) -> Dict[str, Any]:
        """Open a gate without waiting on it."""
        body: Dict[str, Any] = {"action": action}
        if expires_in is not None:
            body["expires_in"] = expires_in
        return self.client._json("POST", self._path("/approvals"), body)

    def gate(self, action: str, expires_in: Optional[int] = None) -> Resolution:
        """Open a gate and wait until a person decides, or it expires.

        This is what puts oversight in the agent's path rather than beside it:
        when it returns, the request and the decision are both already in the
        chain.

        It polls rather than holding a stream open. A gate can sit for hours, and
        a poll survives a proxy idle timeout, a NAT rebind, and a process restart
        where a long-lived connection quietly does not.

        A rejection returns rather than raising: an agent must be able to branch
        on a refusal without wrapping the normal case in exception handling.
        """
        opened = self.open_gate(action, expires_in)
        return self.await_gate(opened["seq"])

    def await_gate(self, seq: int, poll_seconds: float = 0.25) -> Resolution:
        delay = poll_seconds
        while True:
            gate = self.approval(seq)
            if not gate.is_pending:
                if gate.decision is None:
                    raise TidelineError(500, f"gate {seq} is resolved but carries no decision")
                return Resolution(
                    seq=seq,
                    decision=gate.decision,
                    reviewer=gate.reviewer,
                    note=gate.note,
                    attested=gate.attested,
                )
            time.sleep(delay)
            # Back off to five seconds: nobody answers faster than that, and a
            # tight loop on a multi-hour gate is rude.
            delay = min(delay * 2, 5.0)

    def approvals(self) -> List[Gate]:
        """Gates still awaiting a decision — the reviewer's queue."""
        return [Gate(**g) for g in self.client._json("GET", self._path("/approvals"))]

    def approval(self, seq: int) -> Gate:
        return Gate(**self.client._json("GET", self._path(f"/approvals/{seq}")))

    def resolve(
        self,
        seq: int,
        decision: str,
        reviewer: Optional[str] = None,
        note: Optional[str] = None,
    ) -> None:
        """Record a person's decision on a gate."""
        self.client._json(
            "POST",
            self._path(f"/approvals/{seq}/resolve"),
            {"decision": decision, "reviewer": reviewer, "note": note},
        )

    # --- the record -------------------------------------------------------

    def events(self, frm: int = 0, limit: int = 5000) -> List[Event]:
        """The record.

        Read from the response text, never ``json.loads`` alone: a parsed value
        discards the metadata bytes and every event then fails verification.
        """
        text = self.client._request("GET", self._path(f"/events?from={frm}&limit={limit}"))
        return parse_record(text)

    def verified_events(self) -> "tuple[List[Event], ChainOk]":
        """Fetch the record and verify it in one step."""
        events = self.events()
        return events, verify_chain(events)

    def watch(self, frm: int = 0) -> Iterator[Event]:
        """Follow the record live: history first, then events as they land."""
        from .canon import parse_event

        req = urllib.request.Request(
            f"{self.client.base}{self._path(f'/watch?from={frm}')}",
            headers={
                "accept": "text/event-stream",
                **({"authorization": f"Bearer {self.client.key}"} if self.client.key else {}),
            },
        )
        with urllib.request.urlopen(req) as res:
            # Line by line, not read(n): a buffered read blocks until it has n
            # bytes or the stream ends, and an SSE frame is a few dozen bytes on
            # a connection that stays open for hours. readline returns as soon
            # as a newline arrives.
            frame: List[str] = []
            while True:
                raw = res.readline()
                if not raw:
                    return
                line = raw.decode("utf-8", "replace").rstrip("\n")
                if line:
                    frame.append(line)
                    continue

                data, terminal = "", False
                for entry in frame:
                    if entry.startswith("data:"):
                        data += entry[5:].lstrip()
                    elif entry.startswith("event:"):
                        terminal = entry[6:].strip() in ("done", "stream_error")
                frame = []
                if terminal:
                    return
                if data:
                    # From the frame text, so the metadata bytes survive.
                    yield parse_event(data)

    def complete(self) -> Appended:
        return Appended(**self.client._json("POST", self._path("/complete")))

    def checkpoint(self) -> Optional[Dict[str, Any]]:
        """The latest signed checkpoint, or None if the server has issued none."""
        try:
            return self.client._json("GET", self._path("/checkpoint"))
        except TidelineError as e:
            if e.is_not_found:
                return None
            raise

    def redact(self, target_seq: int, fields: Sequence[str], authority: str) -> Appended:
        """Erase fields of an earlier event, keeping the chain intact."""
        return Appended(
            **self.client._json(
                "POST",
                self._path("/redactions"),
                {"target_seq": target_seq, "fields": list(fields), "authority": authority},
            )
        )
