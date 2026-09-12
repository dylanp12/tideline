//! The CLI's contract is its exit code. A script gating a deploy on
//! `tideline verify` depends on nothing else, so 0 / 1 / 2 must stay exact.

use std::io::Write;
use std::process::{Command, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_tideline")
}

fn fixture(name: &str) -> String {
    format!(
        "{}/../../conformance/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin())
        .args(args)
        .output()
        .expect("run tideline");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn verify_accepts_a_sound_record() {
    let (code, stdout, stderr) = run(&["verify", &fixture("sealed-run.json")]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("8 events verified"), "stdout: {stdout}");
    assert!(stdout.contains("sealed    yes"), "stdout: {stdout}");
}

#[test]
fn verify_rejects_a_tampered_record_and_names_the_event() {
    let (code, _, stderr) = run(&["verify", &fixture("tampered-run.json")]);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("seq 3") && stderr.contains("altered"),
        "should name the altered event: {stderr}"
    );
}

#[test]
fn verify_says_plainly_that_an_unsealed_record_could_be_truncated() {
    // An unsealed record verifies, but its tail could have been removed without
    // trace. Saying so is the difference between a verifier and a rubber stamp.
    let (code, stdout, _) = run(&["verify", &fixture("open-run.json")]);
    assert_eq!(code, 0);
    assert!(stdout.contains("not sealed"), "stdout: {stdout}");
    assert!(
        stdout.contains("cannot prove nothing is missing"),
        "stdout: {stdout}"
    );
}

#[test]
fn a_checkpoint_exposes_a_truncated_record() {
    // The case the chain cannot see on its own.
    let record = std::fs::read_to_string(fixture("sealed-run.json")).unwrap();
    let mut events: Vec<serde_json::Value> = serde_json::from_str(&record).unwrap();
    events.truncate(5);

    let dir = std::env::temp_dir().join(format!("tideline-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("truncated.json");
    std::fs::write(&path, serde_json::to_string(&events).unwrap()).unwrap();

    let key = std::fs::read_to_string(fixture("sealed-run-pubkey.txt")).unwrap();
    let (code, _, stderr) = run(&[
        "verify",
        path.to_str().unwrap(),
        "--checkpoint",
        &fixture("sealed-run-checkpoint.json"),
        "--public-key",
        key.trim(),
    ]);
    assert_eq!(code, 1, "a truncated record must not pass");
    assert!(stderr.contains("TRUNCATED"), "stderr: {stderr}");
    assert!(stderr.contains("3 events are missing"), "stderr: {stderr}");
}

#[test]
fn a_checkpoint_over_a_complete_record_verifies() {
    let key = std::fs::read_to_string(fixture("sealed-run-pubkey.txt")).unwrap();
    let (code, stdout, stderr) = run(&[
        "verify",
        &fixture("sealed-run.json"),
        "--checkpoint",
        &fixture("sealed-run-checkpoint.json"),
        "--public-key",
        key.trim(),
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("signature valid"), "stdout: {stdout}");
}

#[test]
fn the_wrong_public_key_fails_the_checkpoint() {
    use base64::Engine as _;
    let wrong = base64::engine::general_purpose::STANDARD.encode(
        ed25519_dalek::SigningKey::from_bytes(&[9u8; 32])
            .verifying_key()
            .as_bytes(),
    );
    let (code, _, stderr) = run(&[
        "verify",
        &fixture("sealed-run.json"),
        "--checkpoint",
        &fixture("sealed-run-checkpoint.json"),
        "--public-key",
        &wrong,
    ]);
    assert_eq!(code, 1);
    assert!(stderr.contains("does not verify"), "stderr: {stderr}");
}

#[test]
fn verify_reads_standard_input() {
    let record = std::fs::read_to_string(fixture("sealed-run.json")).unwrap();
    let mut child = Command::new(bin())
        .args(["verify", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(record.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("8 events verified"));
}

#[test]
fn unparseable_input_exits_two_not_one() {
    // "This is not a record" and "this record is bad" are different answers,
    // and a CI gate needs to tell them apart.
    let mut child = Command::new(bin())
        .args(["verify", "-"])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"nonsense")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not a TLR/1 record"));
}

#[test]
fn a_missing_file_exits_two() {
    let (code, _, stderr) = run(&["verify", "/nonexistent/record.json"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("cannot read"), "stderr: {stderr}");
}

#[test]
fn quiet_prints_only_the_verdict() {
    let (code, stdout, _) = run(&["verify", "--quiet", &fixture("sealed-run.json")]);
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "ok");
}
