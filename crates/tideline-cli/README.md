# tideline

Verify a tamper-evident AI agent run record — offline, with no server, no
credentials, and no network.

```bash
cargo install tideline-cli        # installs the `tideline` binary
```

```console
$ tideline verify evidence.json
8 events verified
head      1711f9a6737029f6…f96f53e885952 (seq 7)
sealed    yes
```

Exit codes are the contract: **0** sound, **1** not sound, **2** unreadable. A CI
gate can depend on nothing else, and "this record is bad" stays distinct from
"this is not a record".

```console
$ tideline verify tampered.json; echo $?
event seq 3 was altered. Its contents no longer match the hash recorded for it.
1
```

## Truncation

A hash chain proves nothing was *altered*. It cannot prove nothing is *missing*:
delete the last three events and the remaining prefix is still a valid chain.
`verify` says so rather than implying more than it knows.

```console
$ tideline verify open-run.json
4 events verified
head      6ff299a8db2ccddf…535d25ad4f03e (seq 3)

This record is not sealed and no checkpoint was supplied, so events
could have been removed from the end without leaving a trace. The
chain proves nothing was altered; it cannot prove nothing is missing.
```

Pass a signed checkpoint and the server's public key — both from
`/v1/.well-known/tideline` — and the gap closes:

```console
$ tideline verify truncated.json --checkpoint cp.json --public-key "$(cat key.txt)"; echo $?
5 events verified
TRUNCATED: the checkpoint covers seq 7 but the record ends at seq 4. 3 events are missing from the end.
1
```

## An auditor's install

```bash
cargo install tideline-cli --no-default-features
```

Drops `export` and `tail` and, with them, every HTTP dependency. What remains
cannot open a socket — which is the point when the thing being audited is the
server you would otherwise be talking to.

## Fetching and following

```bash
tideline export loan-4821 --url https://records.example.com --key "$TIDELINE_KEY"
tideline tail   loan-4821
```

`export` verifies on the way out. An export nobody checked is just a file.

Licensed under MIT OR Apache-2.0.
