# Security

## Reporting a vulnerability

Email **dylan.parent5@gmail.com** with "tideline security" in the subject. Please
do not open a public issue for a vulnerability.

Expect an acknowledgement within three working days and an assessment within ten.
If a fix is warranted we will agree a disclosure date with you, and credit you
unless you prefer otherwise.

## What the design does and does not protect

Being precise about this is part of the product. A verifier that implies more
than it knows is worse than none.

**The chain proves integrity, not honesty.** Verification shows a record was not
altered after it was written. It says nothing about whether the agent reported
its actions truthfully in the first place. That is why capture belongs in the
execution path, and why an approval whose reviewer the server could not
authenticate is recorded as `attested: false` rather than quietly trusted.

**A chain alone cannot detect truncation.** Remove the last N events and the
remaining prefix is still a valid chain. Signed checkpoints close this, but only
if you retain them somewhere the record's operator does not control. `tideline
verify` says plainly when a record is unsealed and uncheckpointed.

**Checkpoint keys are the trust root.** Anyone who can sign with the server's key
can re-sign rewritten history. Hold the key separately from the record store, and
anchor checkpoints externally where the threat model includes the operator.

**Redaction is irreversible.** An erased value keeps its digest, so it can be
confirmed if supplied from elsewhere, but it cannot be recovered from the record.

**`subject_ref` and `labels` are not redactable** and appear in list queries.
They must not carry personal data.

## Deployment expectations

- Set `TIDELINE_PUBLISH_TOKEN`, or `TIDELINE_AUTH_URL`, before exposing a server.
  Record reads fall open only when nothing at all is configured, and the server
  warns at startup when it is running that way.
- The checkpoint signing key persists: from `TIDELINE_CHECKPOINT_KEY`, or a key
  file beside the record database created at mode 0600. Set the environment
  variable if you want it held somewhere else, such as a secret manager. Note
  that a key stored beside the records is readable by anyone who can read them,
  which is why external anchoring — not this key — is what protects against the
  operator.
- Do not run with `TIDELINE_REDIS_URL` and local record storage behind a load
  balancer. The server refuses to start in that configuration, because each
  instance would hold a fragment of every run.

## Supported versions

Pre-1.0. Security fixes land on the latest minor release.
