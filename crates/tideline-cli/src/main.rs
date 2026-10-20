//! `tideline` — verify an agent-run record, export one, or follow one live.
//!
//! `verify` is the reason this exists. It reads a record from a file and proves
//! nothing in it was altered, using no server, no credentials, and no network.
//! Built with `--no-default-features`, it cannot open a socket at all.

use clap::{Parser, Subcommand};
use std::io::Read;
use std::process::ExitCode;
use tideline_proto::{verify_chain, ChainError, Checkpoint, RunEvent};

#[derive(Parser)]
#[command(
    name = "tideline",
    version,
    about = "Verify, export, and follow tamper-evident AI agent run records (TLR/1)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Verify a record's hash chain. Exits 0 if sound, 1 if not, 2 if unreadable.
    Verify {
        /// Path to a JSON record, or `-` for standard input.
        path: String,
        /// A signed checkpoint to check the record against.
        #[arg(long)]
        checkpoint: Option<String>,
        /// The server's Ed25519 public key, base64, from /v1/.well-known/tideline.
        #[arg(long)]
        public_key: Option<String>,
        /// Print only the verdict.
        #[arg(long, short)]
        quiet: bool,
    },

    /// Fetch a whole record from a server and write it to a file.
    #[cfg(feature = "client")]
    Export {
        run_id: String,
        #[arg(long, env = "TIDELINE_URL", default_value = "http://localhost:8080")]
        url: String,
        #[arg(long, env = "TIDELINE_KEY")]
        key: Option<String>,
        /// Where to write. Defaults to `<run-id>.json`.
        #[arg(long, short)]
        out: Option<String>,
    },

    /// Follow a run live, printing each event as it lands.
    #[cfg(feature = "client")]
    Tail {
        run_id: String,
        #[arg(long, env = "TIDELINE_URL", default_value = "http://localhost:8080")]
        url: String,
        #[arg(long, env = "TIDELINE_KEY")]
        key: Option<String>,
        /// Start from this sequence number.
        #[arg(long, default_value_t = 0)]
        from: u64,
    },
}

/// 0 sound, 1 not sound, 2 unreadable. A script gating a deploy on
/// `tideline verify` depends on nothing else, so the distinction between "this
/// record is bad" and "this is not a record" has to survive.
const OK: u8 = 0;
const UNSOUND: u8 = 1;
const UNREADABLE: u8 = 2;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Verify {
            path,
            checkpoint,
            public_key,
            quiet,
        } => verify(&path, checkpoint.as_deref(), public_key.as_deref(), quiet),
        #[cfg(feature = "client")]
        Command::Export {
            run_id,
            url,
            key,
            out,
        } => client::export(&run_id, &url, key.as_deref(), out.as_deref()),
        #[cfg(feature = "client")]
        Command::Tail {
            run_id,
            url,
            key,
            from,
        } => client::tail(&run_id, &url, key.as_deref(), from),
    };
    ExitCode::from(code)
}

fn read_input(path: &str) -> Result<String, String> {
    if path == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("cannot read standard input: {e}"))?;
        return Ok(buf);
    }
    std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))
}

fn verify(path: &str, checkpoint: Option<&str>, public_key: Option<&str>, quiet: bool) -> u8 {
    let text = match read_input(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return UNREADABLE;
        }
    };
    // From the text, never through a generic JSON value: that would reorder
    // metadata keys and fail a sound record.
    let events: Vec<RunEvent> = match serde_json::from_str(&text) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("not a TLR/1 record: {e}");
            return UNREADABLE;
        }
    };

    let ok = match verify_chain(&events) {
        Ok(ok) => ok,
        Err(e) => {
            eprintln!("{}", describe(&e));
            return UNSOUND;
        }
    };

    if quiet {
        println!("ok");
    } else {
        println!("{} events verified", ok.len);
        println!("head      {} (seq {})", ok.head_hash.to_hex(), ok.head_seq);
    }

    let mut code = OK;
    if let Some(cp_path) = checkpoint {
        code = code.max(check_checkpoint(cp_path, public_key, &events, &ok, quiet));
    } else if !ok.sealed && !quiet {
        // Saying so is the difference between a verifier and a rubber stamp.
        println!(
            "\nThis record is not sealed and no checkpoint was supplied, so events\n\
             could have been removed from the end without leaving a trace. The\n\
             chain proves nothing was altered; it cannot prove nothing is missing."
        );
    } else if !quiet {
        println!("sealed    yes");
    }
    code
}

fn check_checkpoint(
    path: &str,
    public_key: Option<&str>,
    events: &[RunEvent],
    ok: &tideline_proto::ChainOk,
    quiet: bool,
) -> u8 {
    let text = match read_input(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return UNREADABLE;
        }
    };
    let cp: Checkpoint = match serde_json::from_str(&text) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("not a checkpoint: {e}");
            return UNREADABLE;
        }
    };

    // A checkpoint names the run it covers. Accepting one without checking that
    // would let a validly signed checkpoint for a different run vouch for this
    // record.
    if let Some(run_id) = run_id_of(events) {
        if cp.run_id != run_id {
            eprintln!(
                "the checkpoint covers run {}, but this record is run {run_id}",
                cp.run_id
            );
            return UNSOUND;
        }
    }

    if cp.seq > ok.head_seq {
        eprintln!(
            "TRUNCATED: the checkpoint covers seq {} but the record ends at seq {}. \
             {} events are missing from the end.",
            cp.seq,
            ok.head_seq,
            cp.seq - ok.head_seq
        );
        return UNSOUND;
    }

    // Match the checkpoint against the event it actually covers, not only
    // against the head. A checkpoint from a different history can sit below
    // this record's head and would otherwise pass unexamined.
    match events.iter().find(|e| e.seq == cp.seq) {
        Some(covered) if covered.hash != cp.head_hash => {
            eprintln!(
                "the checkpoint's head for seq {} does not match this record: it attests \
                 {}…, the record has {}…. These are different histories.",
                cp.seq,
                &cp.head_hash.to_hex()[..16],
                &covered.hash.to_hex()[..16]
            );
            return UNSOUND;
        }
        None => {
            eprintln!(
                "the checkpoint covers seq {}, which is not in this record",
                cp.seq
            );
            return UNSOUND;
        }
        Some(_) => {}
    }

    let Some(key_b64) = public_key else {
        if !quiet {
            println!(
                "checkpoint matches seq {} — pass --public-key to check its signature",
                cp.seq
            );
        }
        return OK;
    };

    use base64::Engine as _;
    let raw = match base64::engine::general_purpose::STANDARD.decode(key_b64) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("--public-key is not base64: {e}");
            return UNREADABLE;
        }
    };
    let bytes: [u8; 32] = match raw.try_into() {
        Ok(b) => b,
        Err(_) => {
            eprintln!("--public-key must be 32 bytes");
            return UNREADABLE;
        }
    };
    let key = match ed25519_dalek::VerifyingKey::from_bytes(&bytes) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("--public-key is not a valid Ed25519 key: {e}");
            return UNREADABLE;
        }
    };

    match cp.verify(&key) {
        Ok(()) => {
            if !quiet {
                println!(
                    "checkpoint signature valid (key {}), covering seq {}",
                    cp.key_id, cp.seq
                );
                if cp.seq < ok.head_seq {
                    println!(
                        "note: it covers seq {} of {}, so the {} events after it are \
                         protected by the chain but not by this checkpoint",
                        cp.seq,
                        ok.head_seq,
                        ok.head_seq - cp.seq
                    );
                }
            }
            OK
        }
        Err(e) => {
            eprintln!("checkpoint signature does not verify: {e:?}");
            UNSOUND
        }
    }
}

/// The run id a record declares, read from its `run_started` envelope.
fn run_id_of(events: &[RunEvent]) -> Option<String> {
    let first = events.first()?;
    let metadata = first.metadata_raw()?;
    let value: serde_json::Value = serde_json::from_str(metadata).ok()?;
    Some(value.get("run_id")?.as_str()?.to_string())
}

/// Say what went wrong in terms of the record, not the data structure.
fn describe(e: &ChainError) -> String {
    match e {
        ChainError::Empty => "the record is empty".into(),
        ChainError::NotRunStarted => {
            "this record does not begin with run_started, so its envelope is missing".into()
        }
        ChainError::SeqGap { expected, found } => format!(
            "an event is missing: expected seq {expected}, found seq {found}. \
             The record was edited after it was written."
        ),
        ChainError::PrevMismatch { seq, .. } => format!(
            "the link into seq {seq} was rewritten. The record was edited after it was written."
        ),
        ChainError::HashMismatch { seq, .. } => format!(
            "event seq {seq} was altered. Its contents no longer match the hash recorded for it."
        ),
    }
}

#[cfg(feature = "client")]
mod client {
    use super::{OK, UNREADABLE, UNSOUND};
    use tideline_sdk::Tideline;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Runtime::new().expect("start a runtime")
    }

    pub fn export(run_id: &str, url: &str, key: Option<&str>, out: Option<&str>) -> u8 {
        runtime().block_on(async {
            let run = Tideline::new(url, key).run(run_id);
            let events = match run.events().await {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("cannot fetch {run_id}: {e}");
                    return UNREADABLE;
                }
            };
            let path = out
                .map(str::to_string)
                .unwrap_or_else(|| format!("{run_id}.json"));
            let mut json = match serde_json::to_string_pretty(&events) {
                Ok(j) => j,
                Err(e) => {
                    eprintln!("cannot serialise the record: {e}");
                    return UNREADABLE;
                }
            };
            json.push('\n');
            if let Err(e) = std::fs::write(&path, json) {
                eprintln!("cannot write {path}: {e}");
                return UNREADABLE;
            }

            // Verify on the way out. An export nobody checked is just a file.
            match tideline_proto::verify_chain(&events) {
                Ok(ok) => {
                    println!("wrote {path} — {} events, verified", ok.len);
                    if !ok.sealed {
                        println!("note: this run is not sealed; its tail could still grow.");
                    }
                    OK
                }
                Err(e) => {
                    eprintln!(
                        "wrote {path}, but it DOES NOT VERIFY: {}",
                        super::describe(&e)
                    );
                    UNSOUND
                }
            }
        })
    }

    pub fn tail(run_id: &str, url: &str, key: Option<&str>, from: u64) -> u8 {
        use futures::StreamExt;
        runtime().block_on(async {
            let run = Tideline::new(url, key).run(run_id);
            let stream = match run.watch(from).await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("cannot follow {run_id}: {e}");
                    return UNREADABLE;
                }
            };
            let mut stream = Box::pin(stream);
            while let Some(event) = stream.next().await {
                match event {
                    Ok(e) => println!(
                        "{:>4}  {:<13} {}",
                        e.seq,
                        e.kind.as_str(),
                        e.name.as_deref().or(e.content.as_deref()).unwrap_or("")
                    ),
                    Err(e) => {
                        eprintln!("stream error: {e}");
                        return UNREADABLE;
                    }
                }
            }
            OK
        })
    }
}
