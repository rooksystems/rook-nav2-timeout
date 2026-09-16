//! Sabotage tests: each one tampers with a copy of the holdout capsule and
//! asserts the verifier names the damage. The capsule artifacts are the
//! threat model; the verifier must refuse or diverge, never verify.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rook_host::encode_hex;
use sha2::{Digest, Sha256};

fn capsule_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../capsules/rmf-blockade-holdout")
}

/// Copies the capsule into a fresh temp directory unique to this test.
fn copy_capsule(test_name: &str) -> PathBuf {
    let destination = std::env::temp_dir().join(format!(
        "rook-verify-sabotage-{test_name}-{}",
        std::process::id()
    ));
    if destination.exists() {
        fs::remove_dir_all(&destination).expect("clear stale capsule copy");
    }
    fs::create_dir_all(&destination).expect("create capsule copy directory");
    for entry in fs::read_dir(capsule_dir()).expect("read capsule directory") {
        let entry = entry.expect("read capsule entry");
        fs::copy(entry.path(), destination.join(entry.file_name())).expect("copy capsule file");
    }
    destination
}

fn run_verifier(dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rook-verify"))
        .arg(dir)
        .output()
        .expect("run rook-verify")
}

/// Rewrites one file's line in MANIFEST.sha256 so the manifest passes and
/// the test reaches the replay.
fn repair_manifest_line(dir: &Path, name: &str) {
    let manifest_path = dir.join("MANIFEST.sha256");
    let manifest = fs::read_to_string(&manifest_path).expect("read manifest");
    let hex = encode_hex(&Sha256::digest(
        fs::read(dir.join(name)).expect("read tampered file"),
    ));
    let rewritten: Vec<String> = manifest
        .lines()
        .map(|line| {
            if line.ends_with(&format!("  {name}")) {
                format!("{hex}  {name}")
            } else {
                line.to_string()
            }
        })
        .collect();
    fs::write(&manifest_path, rewritten.join("\n") + "\n").expect("write repaired manifest");
}

#[test]
fn flipped_heartbeat_byte_names_the_tick_containing_the_flip() {
    let dir = copy_capsule("flipped-heartbeat");
    let path = dir.join("heartbeats_recorded.bin");
    let mut bytes = fs::read(&path).expect("read heartbeats");

    // Records are (tick u64 LE, payload length u32 LE, payload bytes).
    let mut cursor = 0_usize;
    let mut tampered_tick = None;
    for index in 0.. {
        let tick = u64::from_le_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
        let length =
            u32::from_le_bytes(bytes[cursor + 8..cursor + 12].try_into().unwrap()) as usize;
        assert!(length > 0, "heartbeat payloads are never empty");
        if index == 100 {
            bytes[cursor + 12] ^= 0x01;
            tampered_tick = Some(tick);
            break;
        }
        cursor += 12 + length;
    }
    let tampered_tick = tampered_tick.expect("recording has more than 100 heartbeats");
    fs::write(&path, &bytes).expect("write tampered heartbeats");
    repair_manifest_line(&dir, "heartbeats_recorded.bin");

    let output = run_verifier(&dir);
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert_eq!(output.status.code(), Some(1), "stdout:\n{stdout}");
    assert!(
        stdout.contains(&format!("first divergent tick: {tampered_tick}")),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("diverged:"), "stdout:\n{stdout}");
    fs::remove_dir_all(&dir).expect("clean up capsule copy");
}

#[test]
fn flipped_delivery_byte_fails_replay_and_names_a_divergent_tick() {
    let dir = copy_capsule("flipped-delivery");
    let path = dir.join("deliveries.bin");
    let mut bytes = fs::read(&path).expect("read deliveries");

    // Records are (tick u64 LE, channel u32 LE, payload length u32 LE,
    // payload bytes). Tamper the 200th delivery on a blockade channel
    // (10..=14; those payloads are never empty), mid-file. The flipped byte
    // is the low byte of the checkpoint field (payload offset 16), not the
    // last payload byte: the last byte is the checkpoint's most significant
    // byte and the cell aborts on ids >= 2^32 instead of replaying to a
    // divergence it can name.
    let mut cursor = 0_usize;
    let mut blockade_deliveries = 0_usize;
    let mut tampered = false;
    while cursor < bytes.len() {
        let channel = u32::from_le_bytes(bytes[cursor + 8..cursor + 12].try_into().unwrap());
        let length =
            u32::from_le_bytes(bytes[cursor + 12..cursor + 16].try_into().unwrap()) as usize;
        if (10..=14).contains(&channel) {
            blockade_deliveries += 1;
            if blockade_deliveries == 200 {
                assert!(length > 16, "blockade delivery carries a checkpoint field");
                bytes[cursor + 16 + 16] ^= 0x01;
                tampered = true;
                break;
            }
        }
        cursor += 16 + length;
    }
    assert!(tampered, "recording has at least 200 blockade deliveries");
    fs::write(&path, &bytes).expect("write tampered deliveries");
    repair_manifest_line(&dir, "deliveries.bin");

    let output = run_verifier(&dir);
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert_eq!(output.status.code(), Some(1), "stdout:\n{stdout}");
    assert!(
        stdout.contains("diverged: bag_run_hash"),
        "stdout:\n{stdout}"
    );
    // 1787185426672789656 is deterministic for this fixed flip and cell.
    assert!(
        stdout.contains("first divergent tick: 1787185426672789656"),
        "stdout:\n{stdout}"
    );
    fs::remove_dir_all(&dir).expect("clean up capsule copy");
}

#[test]
fn edited_expected_hash_is_refused_at_the_manifest_step() {
    let dir = copy_capsule("edited-expected");
    let path = dir.join("expected");
    let text = fs::read_to_string(&path).expect("read expected");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let hash_line = lines
        .iter_mut()
        .find(|line| line.starts_with("bag_run_hash"))
        .expect("expected file has bag_run_hash");
    let flipped = if hash_line.ends_with('5') { '6' } else { '5' };
    hash_line.pop();
    hash_line.push(flipped);
    fs::write(&path, lines.join("\n") + "\n").expect("write tampered expected");
    // The manifest is deliberately not repaired.

    let output = run_verifier(&dir);
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert_eq!(output.status.code(), Some(2), "stdout:\n{stdout}");
    assert!(
        stdout.contains("manifest: expected: sha256 mismatch"),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("refusing to run"), "stdout:\n{stdout}");
    assert!(
        !stdout.contains("bag_run_hash="),
        "the replay must never have run; stdout:\n{stdout}"
    );
    fs::remove_dir_all(&dir).expect("clean up capsule copy");
}

#[test]
fn edited_expected_with_dropped_manifest_line_is_refused() {
    let dir = copy_capsule("edited-expected-dropped-manifest");
    let path = dir.join("expected");
    let text = fs::read_to_string(&path).expect("read expected");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let hash_line = lines
        .iter_mut()
        .find(|line| line.starts_with("bag_run_hash"))
        .expect("expected file has bag_run_hash");
    let flipped = if hash_line.ends_with('5') { '6' } else { '5' };
    hash_line.pop();
    hash_line.push(flipped);
    fs::write(&path, lines.join("\n") + "\n").expect("write tampered expected");

    let manifest_path = dir.join("MANIFEST.sha256");
    let manifest = fs::read_to_string(&manifest_path).expect("read manifest");
    let rewritten: Vec<String> = manifest
        .lines()
        .filter(|line| !line.ends_with("  expected"))
        .map(str::to_string)
        .collect();
    fs::write(&manifest_path, rewritten.join("\n") + "\n").expect("write dropped-line manifest");

    let output = run_verifier(&dir);
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert_eq!(output.status.code(), Some(2), "stdout:\n{stdout}");
    assert!(
        stdout.contains("manifest: expected: not covered by the manifest"),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("refusing to run"), "stdout:\n{stdout}");
    assert!(
        !stdout.contains("bag_run_hash="),
        "the replay must never have run; stdout:\n{stdout}"
    );
    fs::remove_dir_all(&dir).expect("clean up capsule copy");
}

#[test]
fn tick_tamper_with_repaired_manifest_is_named() {
    let dir = copy_capsule("tick-tamper");
    let path = dir.join("heartbeats_recorded.bin");
    let mut bytes = fs::read(&path).expect("read heartbeats");
    let tick = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
    bytes[0..8].copy_from_slice(&(tick + 1).to_le_bytes());
    fs::write(&path, &bytes).expect("write tampered heartbeats");
    repair_manifest_line(&dir, "heartbeats_recorded.bin");

    let output = run_verifier(&dir);
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert_eq!(output.status.code(), Some(1), "stdout:\n{stdout}");
    assert!(
        stdout.contains("diverged: recorded heartbeat stream (sha256 does not match expected)"),
        "stdout:\n{stdout}"
    );
    fs::remove_dir_all(&dir).expect("clean up capsule copy");
}

#[test]
fn symlinked_manifest_entry_is_refused() {
    let dir = copy_capsule("symlinked-manifest");
    let outside = dir.parent().unwrap().join(format!(
        "{}-expected-outside",
        dir.file_name().unwrap().to_string_lossy()
    ));
    fs::rename(dir.join("expected"), &outside).expect("move expected outside capsule");
    std::os::unix::fs::symlink(&outside, dir.join("expected")).expect("symlink expected");

    let output = run_verifier(&dir);
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert_eq!(output.status.code(), Some(2), "stdout:\n{stdout}");
    assert!(
        stdout.contains("manifest: expected: not a regular file"),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("refusing to run"), "stdout:\n{stdout}");
    fs::remove_dir_all(&dir).expect("clean up capsule copy");
    fs::remove_file(&outside).expect("clean up moved expected");
}

fn native_fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/native-ref/timeout")
}

fn copy_dir(source: &Path, test_name: &str) -> PathBuf {
    let destination = std::env::temp_dir().join(format!(
        "rook-verify-sabotage-{test_name}-{}",
        std::process::id()
    ));
    if destination.exists() {
        fs::remove_dir_all(&destination).expect("clear stale copy");
    }
    fs::create_dir_all(&destination).expect("create copy directory");
    for entry in fs::read_dir(source).expect("read source directory") {
        let entry = entry.expect("read entry");
        fs::copy(entry.path(), destination.join(entry.file_name())).expect("copy file");
    }
    destination
}

fn rewrite_kind_header(dir: &Path, header: &str) {
    let manifest_path = dir.join("MANIFEST.sha256");
    let manifest = fs::read_to_string(&manifest_path).expect("read manifest");
    let mut lines: Vec<&str> = manifest.lines().collect();
    assert!(lines[0].starts_with("# rook-capsule-v1 kind="));
    lines[0] = header;
    fs::write(&manifest_path, lines.join("\n") + "\n").expect("write manifest");
}

#[test]
fn forged_native_kind_on_the_wasm_capsule_is_refused_by_name() {
    let dir = copy_capsule("forged-native-kind");
    rewrite_kind_header(&dir, "# rook-capsule-v1 kind=native-adapter");
    let output = run_verifier(&dir);
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert_eq!(output.status.code(), Some(2), "stdout:\n{stdout}");
    assert!(
        stdout.contains(
            "manifest: events.bin: not covered by the manifest (kind native-adapter requires it)"
        ),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("refusing to run"), "stdout:\n{stdout}");
    assert!(!stdout.contains("verified:"), "stdout:\n{stdout}");
    fs::remove_dir_all(&dir).expect("clean up");
}

#[test]
fn unknown_kind_is_refused_by_name() {
    let dir = copy_capsule("unknown-kind");
    rewrite_kind_header(&dir, "# rook-capsule-v1 kind=wasm-nav2");
    let output = run_verifier(&dir);
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert_eq!(output.status.code(), Some(2), "stdout:\n{stdout}");
    assert!(
        stdout.contains("refusing to run: capsule kind wasm-nav2 not supported by this build"),
        "stdout:\n{stdout}"
    );
    assert!(!stdout.contains("bag_run_hash="), "stdout:\n{stdout}");
    fs::remove_dir_all(&dir).expect("clean up");
}

#[test]
fn missing_kind_header_is_refused() {
    let dir = copy_capsule("missing-kind");
    let manifest_path = dir.join("MANIFEST.sha256");
    let manifest = fs::read_to_string(&manifest_path).expect("read manifest");
    let without: Vec<&str> = manifest.lines().skip(1).collect();
    fs::write(&manifest_path, without.join("\n") + "\n").expect("write manifest");
    let output = run_verifier(&dir);
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert_eq!(output.status.code(), Some(2), "stdout:\n{stdout}");
    assert!(
        stdout.contains("manifest: first line is not '# rook-capsule-v1 kind=<kind>'"),
        "stdout:\n{stdout}"
    );
    fs::remove_dir_all(&dir).expect("clean up");
}

#[test]
fn native_capsule_reports_four_claims_and_never_says_verified() {
    let output = run_verifier(&native_fixture_dir());
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert_eq!(output.status.code(), Some(2), "stdout:\n{stdout}");
    for line in [
        "capsule: format v1, kind native-adapter",
        "raw_record_blake3=",
        "file_integrity: verified (9 frames, raw record and captured effects match expected)",
        "execution_agreement: not performed by this build",
        "capture_completeness: declared grade Complete, end marker present, 0 gap frames; declared, not verified here",
        "incident_authenticity: outside this verifier",
        "claim: measured",
        "not executed: capsule kind native-adapter is not executable by this build",
    ] {
        assert!(
            stdout.contains(line),
            "missing {line:?} in stdout:\n{stdout}"
        );
    }
    assert!(!stdout.contains("verified:"), "stdout:\n{stdout}");
    assert!(!stdout.contains("guaranteed"), "stdout:\n{stdout}");
}

#[test]
fn native_capsule_with_an_incomplete_identity_is_refused() {
    let dir = copy_dir(&native_fixture_dir(), "native-identity-missing-key");
    let path = dir.join("identity");
    let text = fs::read_to_string(&path).expect("read identity");
    let without: Vec<&str> = text
        .lines()
        .filter(|line| !line.starts_with("property_source_blake3"))
        .collect();
    fs::write(&path, without.join("\n") + "\n").expect("write identity");
    repair_manifest_line(&dir, "identity");
    let output = run_verifier(&dir);
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert_eq!(output.status.code(), Some(2), "stdout:\n{stdout}");
    assert!(
        stdout.contains("refusing to run: identity file missing key property_source_blake3"),
        "stdout:\n{stdout}"
    );
    assert!(!stdout.contains("file_integrity"), "stdout:\n{stdout}");
    fs::remove_dir_all(&dir).expect("clean up");
}

#[test]
fn native_capsule_with_a_torn_captured_effects_file_is_refused() {
    let dir = copy_dir(&native_fixture_dir(), "native-captured-torn");
    let path = dir.join("effects_captured.bin");
    let bytes = fs::read(&path).expect("read captured effects");
    fs::write(&path, &bytes[..bytes.len() - 3]).expect("write torn file");
    repair_manifest_line(&dir, "effects_captured.bin");
    let output = run_verifier(&dir);
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert_eq!(output.status.code(), Some(2), "stdout:\n{stdout}");
    assert!(
        stdout.contains("refusing to run: effects_captured.bin is malformed"),
        "stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("file_integrity: verified"),
        "stdout:\n{stdout}"
    );
    fs::remove_dir_all(&dir).expect("clean up");
}

#[test]
fn native_capsule_with_a_non_effect_captured_record_is_refused() {
    let dir = copy_dir(&native_fixture_dir(), "native-captured-not-effect");
    let path = dir.join("effects_captured.bin");
    let mut bytes = fs::read(&path).expect("read captured effects");
    // The first record's event type (bytes 4..6, after the length prefix)
    // becomes Message (0x0010), a Deliver, with an otherwise valid body.
    bytes[4..6].copy_from_slice(&0x0010_u16.to_le_bytes());
    fs::write(&path, &bytes).expect("write edited file");
    repair_manifest_line(&dir, "effects_captured.bin");
    let output = run_verifier(&dir);
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert_eq!(output.status.code(), Some(2), "stdout:\n{stdout}");
    assert!(
        stdout.contains("refusing to run: effects_captured.bin is malformed"),
        "stdout:\n{stdout}"
    );
    fs::remove_dir_all(&dir).expect("clean up");
}

#[test]
fn forged_wasm_kind_on_the_native_capsule_is_refused_by_name() {
    let dir = copy_dir(&native_fixture_dir(), "forged-wasm-kind");
    rewrite_kind_header(&dir, "# rook-capsule-v1 kind=wasm-rmf-blockade");
    let output = run_verifier(&dir);
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert_eq!(output.status.code(), Some(2), "stdout:\n{stdout}");
    assert!(
        stdout.contains("manifest: deliveries.bin: not covered by the manifest (kind wasm-rmf-blockade requires it)"),
        "stdout:\n{stdout}"
    );
    fs::remove_dir_all(&dir).expect("clean up");
}

#[test]
fn native_tail_cut_with_repaired_manifest_is_refused_as_unclosed() {
    let dir = copy_dir(&native_fixture_dir(), "native-tail-cut");
    let path = dir.join("events.bin");
    let bytes = fs::read(&path).expect("read events");
    // Drop the end marker frame: 80-byte header plus its 8 + 24 byte payload.
    fs::write(&path, &bytes[..bytes.len() - 112]).expect("write cut events");
    repair_manifest_line(&dir, "events.bin");
    let output = run_verifier(&dir);
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert_eq!(output.status.code(), Some(2), "stdout:\n{stdout}");
    assert!(
        stdout.contains("diverged: raw_record_blake3"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("refusing to run: events.bin is not a closed native stream (MissingEnd)"),
        "stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("file_integrity: verified"),
        "stdout:\n{stdout}"
    );
    fs::remove_dir_all(&dir).expect("clean up");
}

#[test]
fn native_observed_time_edit_with_repaired_manifest_diverges_on_the_raw_record() {
    let dir = copy_dir(&native_fixture_dir(), "native-observed-time");
    let path = dir.join("events.bin");
    let mut bytes = fs::read(&path).expect("read events");
    // Frame 0's observed_ns lives at bytes 72..80; the execution hashes do
    // not see it, so only the raw record hash can name this edit.
    bytes[72] ^= 1;
    fs::write(&path, &bytes).expect("write edited events");
    repair_manifest_line(&dir, "events.bin");
    let output = run_verifier(&dir);
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert_eq!(output.status.code(), Some(1), "stdout:\n{stdout}");
    assert!(
        stdout.contains("diverged: raw_record_blake3"),
        "stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("diverged: recorded run_hash"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("file_integrity: diverged"),
        "stdout:\n{stdout}"
    );
    fs::remove_dir_all(&dir).expect("clean up");
}
