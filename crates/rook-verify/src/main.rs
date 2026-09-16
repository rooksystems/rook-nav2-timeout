//! Verifies a self-contained replay capsule: manifest first, then the
//! capsule kind decides. A `wasm-rmf-blockade` capsule replays the recorded
//! deliveries through its cell and compares with the recorded heartbeat
//! stream and the expected hashes. A `native-adapter` capsule gets its file
//! integrity checked and its recorded hashes recomputed; execution is not
//! this binary's to perform, so it never says verified. Any other kind is
//! refused by name. Capsule format v1 is in docs/internals/format-spec.md.
//!
//! Exit codes: 0 verified, 1 divergence, 2 refused (bad arguments, failed
//! manifest, malformed capsule, kind not executable by this build).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use rook_host::{encode_hex, rmf, rmf_bag};
use sha2::{Digest, Sha256};

const CAPSULE_HEADER: &str = "# rook-capsule-v1 kind=";
const KIND_WASM_RMF_BLOCKADE: &str = "wasm-rmf-blockade";
const KIND_NATIVE_ADAPTER: &str = "native-adapter";
const WASM_RMF_BLOCKADE_FILES: [&str; 5] = [
    "expected",
    "deliveries.bin",
    "heartbeats_recorded.bin",
    "params",
    "rmf_blockade_cell.wasm",
];
const NATIVE_ADAPTER_FILES: [&str; 5] = [
    "expected",
    "events.bin",
    "effects_captured.bin",
    "identity",
    "case",
];
const NATIVE_IDENTITY_KEYS: [&str; 10] = [
    "component",
    "adapter",
    "adapter_protocol",
    "property",
    "property_source_blake3",
    "normalization",
    "normalization_source_blake3",
    "starting_state",
    "effect_record_order",
    "endpoint.1",
];
const NATIVE_CASE_KEYS: [&str; 6] = [
    "corpus",
    "scenario",
    "property",
    "scope.completion",
    "scope.observation_end",
    "comparison_policy",
];

const USAGE: &str = "usage: rook-verify <capsule-dir>";

fn main() {
    let arguments = match std::env::args_os()
        .skip(1)
        .map(|arg| arg.into_string())
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(arguments) => arguments,
        Err(_) => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    let code = match arguments.as_slice() {
        [flag] if flag == "--version" => {
            println!("rook-verify {}", env!("CARGO_PKG_VERSION"));
            0
        }
        [dir] if !dir.starts_with('-') => match run(Path::new(dir)) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("error: {error:#}");
                2
            }
        },
        _ => {
            eprintln!("{USAGE}");
            2
        }
    };
    std::process::exit(code);
}

fn run(dir: &Path) -> Result<i32> {
    let Some((kind, names)) = verify_manifest(dir)? else {
        println!("refusing to run: capsule failed manifest verification");
        return Ok(2);
    };
    println!("capsule: format v1, kind {kind}");
    let required: &[&str] = match kind.as_str() {
        KIND_WASM_RMF_BLOCKADE => &WASM_RMF_BLOCKADE_FILES,
        KIND_NATIVE_ADAPTER => &NATIVE_ADAPTER_FILES,
        _ => {
            println!("refusing to run: capsule kind {kind} not supported by this build");
            return Ok(2);
        }
    };
    for name in required {
        if !names.contains(*name) {
            println!("manifest: {name}: not covered by the manifest (kind {kind} requires it)");
            println!("refusing to run: capsule failed manifest verification");
            return Ok(2);
        }
    }
    if kind == KIND_NATIVE_ADAPTER {
        return run_native(dir);
    }
    let expected = read_expected(&dir.join("expected"))?;

    let recorded_bytes = std::fs::read(dir.join("heartbeats_recorded.bin"))
        .context("read heartbeats_recorded.bin")?;
    let mut diverged =
        encode_hex(&Sha256::digest(&recorded_bytes)) != expected.heartbeats_recorded_sha256;
    if diverged {
        println!("diverged: recorded heartbeat stream (sha256 does not match expected)");
    }

    let deliveries = rmf_bag::read_deliveries(&dir.join("deliveries.bin"))?;
    let recorded = rmf_bag::read_heartbeats(&dir.join("heartbeats_recorded.bin"))?;
    let min_conflict_angle = rmf_bag::read_min_conflict_angle(&dir.join("params"))?;
    let replayed = rmf::replay_bag_deliveries(
        &dir.join("rmf_blockade_cell.wasm"),
        min_conflict_angle,
        &deliveries,
    )?;
    let hashes = rmf_bag::bag_hashes(&deliveries, &replayed)?;

    let run_hash = encode_hex(&hashes.run_hash);
    let output_digest = encode_hex(&hashes.output_digest);
    println!("bag_run_hash={run_hash}");
    println!("bag_output_digest={output_digest}");

    if run_hash != expected.bag_run_hash {
        println!(
            "diverged: bag_run_hash (expected {})",
            expected.bag_run_hash
        );
        diverged = true;
    }
    if output_digest != expected.bag_output_digest {
        println!(
            "diverged: bag_output_digest (expected {})",
            expected.bag_output_digest
        );
        diverged = true;
    }
    let hashes_matched =
        run_hash == expected.bag_run_hash && output_digest == expected.bag_output_digest;

    match first_payload_divergence(&recorded, &replayed) {
        Some(index) => {
            let tick = recorded
                .get(index)
                .or_else(|| replayed.get(index))
                .map(|(tick, _)| *tick)
                .context("divergence index outside both streams")?;
            println!("first divergent tick: {tick}");
            println!(
                "divergence: heartbeat {index}: recorded and replayed decisions differ \
                 (recorded tick {}, replay tick {})",
                stream_tick(&recorded, index),
                stream_tick(&replayed, index)
            );
            if hashes_matched {
                println!("diverged: recorded heartbeat stream");
            }
            diverged = true;
        }
        None if diverged => {
            println!(
                "no output divergence; the mismatch is in the recorded deliveries or the expected file"
            );
        }
        None => {}
    }
    if diverged {
        return Ok(1);
    }

    let replay_states =
        rmf_bag::distinct_states(replayed.iter().map(|(_, payload)| payload.as_slice()));
    println!(
        "states: {}/{}",
        replay_states.len(),
        expected.recorded_states
    );
    if replay_states.len() != expected.recorded_states {
        println!(
            "diverged: distinct replay states (expected {})",
            expected.recorded_states
        );
        return Ok(1);
    }
    println!("verified: replay matches the recorded decisions and the expected hashes");
    Ok(0)
}

/// A `native-adapter` capsule. This build checks what it can without
/// executing anything: every listed file, the raw record and captured
/// effect hashes pinned in `expected`, the recording's framing (session
/// marker, contiguous ordinals, end marker) and the recorded execution
/// hashes recomputed from `events.bin`. It then names what it did not do.
/// Execution agreement belongs to `rook verify` on the case's adapter host,
/// so the exit code is 2 and the word verified never appears.
fn run_native(dir: &Path) -> Result<i32> {
    let expected = read_key_values(&dir.join("expected"))?;
    let case = read_key_values(&dir.join("case"))?;
    let take = |key: &str| -> Result<String> {
        expected
            .get(key)
            .cloned()
            .with_context(|| format!("expected file missing key {key}"))
    };
    let expected_raw = take("raw_record_blake3")?;
    let expected_captured = take("effects_captured_blake3")?;
    let expected_trace = take("trace_hash")?;
    let expected_run = take("run_hash")?;
    let expected_output = take("output_digest")?;
    let grade = take("grade")?;
    let claim = take("claim")?;
    let identity = read_key_values(&dir.join("identity"))?;
    for (file, values, keys) in [
        ("identity", &identity, &NATIVE_IDENTITY_KEYS[..]),
        ("case", &case, &NATIVE_CASE_KEYS[..]),
    ] {
        if let Some(key) = keys.iter().find(|key| !values.contains_key(**key)) {
            println!("refusing to run: {file} file missing key {key}");
            return Ok(2);
        }
    }
    let corpus = &case["corpus"];
    if claim != "measured" {
        println!("refusing to run: native capsule claims {claim}; native claims are measured");
        return Ok(2);
    }
    if !["Complete", "Rebuilt", "Inferred", "Watch-only"].contains(&grade.as_str()) {
        println!("refusing to run: unknown recording grade {grade}");
        return Ok(2);
    }

    let events = std::fs::read(dir.join("events.bin")).context("read events.bin")?;
    let captured =
        std::fs::read(dir.join("effects_captured.bin")).context("read effects_captured.bin")?;
    let raw = encode_hex(&rook_native::raw_record_hash(&events));
    let captured_hex = encode_hex(&rook_native::raw_record_hash(&captured));
    println!("raw_record_blake3={raw}");
    println!("effects_captured_blake3={captured_hex}");
    let mut diverged = false;
    if raw != expected_raw {
        println!("diverged: raw_record_blake3 (expected {expected_raw}, file {raw})");
        diverged = true;
    }
    if captured_hex != expected_captured {
        println!(
            "diverged: effects_captured_blake3 (expected {expected_captured}, file {captured_hex})"
        );
        diverged = true;
    }
    if let Err(error) = check_captured_records(&captured) {
        println!("refusing to run: effects_captured.bin is malformed ({error})");
        return Ok(2);
    }
    let frames = match rook_native::decode_stream(&events) {
        Ok(frames) => frames,
        Err(error) => {
            println!("refusing to run: events.bin is not a closed native stream ({error:?})");
            return Ok(2);
        }
    };
    let hashes = rook_native::hash_frames(corpus.as_bytes(), &frames);
    for (key, expected_hex, actual) in [
        ("trace_hash", &expected_trace, hashes.trace_hash),
        ("run_hash", &expected_run, hashes.run_hash),
        ("output_digest", &expected_output, hashes.output_digest),
    ] {
        let actual = encode_hex(&actual);
        println!("recorded_{key}={actual}");
        if &actual != expected_hex {
            println!("diverged: recorded {key} (expected {expected_hex})");
            diverged = true;
        }
    }
    if diverged {
        println!("file_integrity: diverged");
        return Ok(1);
    }
    let gaps = frames
        .iter()
        .filter(|frame| frame.header.event_type == rook_native::EventType::Gap)
        .count();
    println!(
        "file_integrity: verified ({} frames, raw record and captured effects match expected)",
        frames.len()
    );
    println!("execution_agreement: not performed by this build");
    println!(
        "capture_completeness: declared grade {grade}, end marker present, {gaps} gap frames; declared, not verified here"
    );
    println!("incident_authenticity: outside this verifier");
    println!("claim: measured");
    println!(
        "not executed: capsule kind {KIND_NATIVE_ADAPTER} is not executable by this build; run rook verify on the case's adapter host to measure execution agreement"
    );
    Ok(2)
}

/// `effects_captured.bin` is `len u32 LE` then payload, repeated; every
/// payload must be a valid effect (an Emit event with a well-formed body).
fn check_captured_records(bytes: &[u8]) -> Result<usize> {
    let mut cursor = 0_usize;
    let mut records = 0_usize;
    while cursor < bytes.len() {
        let length = bytes
            .get(cursor..cursor + 4)
            .map(|header| u32::from_le_bytes(header.try_into().unwrap()) as usize)
            .with_context(|| format!("record {records} header cut at byte {cursor}"))?;
        let start = cursor + 4;
        let end = start
            .checked_add(length)
            .filter(|end| *end <= bytes.len())
            .with_context(|| format!("record {records} runs past the end of the file"))?;
        let (header, _) = rook_native::decode_payload(records as u64, &bytes[start..end])
            .map_err(|error| anyhow::anyhow!("record {records}: {error:?}"))?;
        if header.event_type.kind() != rook_native::EnvelopeKind::Emit {
            bail!(
                "record {records} is a {:?}, not an effect",
                header.event_type
            );
        }
        cursor = end;
        records += 1;
    }
    Ok(records)
}

/// Index of the first payload difference between the recorded heartbeat
/// stream and the replayed emit stream, in order and full multiplicity. A
/// longer stream with an identical prefix diverges at the shorter one's
/// length. `None` means the payload streams are byte-identical.
fn first_payload_divergence(
    recorded: &[(u64, Vec<u8>)],
    replayed: &[(u64, Vec<u8>)],
) -> Option<usize> {
    let shared = recorded.len().min(replayed.len());
    for index in 0..shared {
        if recorded[index].1 != replayed[index].1 {
            return Some(index);
        }
    }
    (recorded.len() != replayed.len()).then_some(shared)
}

fn stream_tick(stream: &[(u64, Vec<u8>)], index: usize) -> String {
    match stream.get(index) {
        Some((tick, _)) => tick.to_string(),
        None => "(stream ended)".to_string(),
    }
}

struct Expected {
    bag_run_hash: String,
    bag_output_digest: String,
    heartbeats_recorded_sha256: String,
    recorded_states: usize,
}

/// Same key=value format as the goldens; '#' comments allowed.
fn read_key_values(path: &Path) -> Result<BTreeMap<String, String>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .with_context(|| format!("malformed line in {}: {line}", path.display()))?;
        let key = key.trim().to_string();
        let value = value.trim().to_string();
        if values.contains_key(&key) {
            bail!("duplicate key {key} in {}", path.display());
        }
        values.insert(key, value);
    }
    Ok(values)
}

fn read_expected(path: &Path) -> Result<Expected> {
    let mut values = read_key_values(path)?;
    let mut take = |key: &str| -> Result<String> {
        values
            .remove(key)
            .with_context(|| format!("expected file missing key {key}"))
    };
    let bag_run_hash = take("bag_run_hash")?;
    let bag_output_digest = take("bag_output_digest")?;
    let heartbeats_recorded_sha256 = take("heartbeats_recorded_sha256")?;
    let recorded_states = take("recorded_states")?
        .parse()
        .context("parse recorded_states")?;
    if let Some(key) = values.keys().next() {
        bail!("unknown expected key {key}");
    }
    for (key, value) in [
        ("bag_run_hash", bag_run_hash.as_str()),
        ("bag_output_digest", bag_output_digest.as_str()),
        (
            "heartbeats_recorded_sha256",
            heartbeats_recorded_sha256.as_str(),
        ),
    ] {
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            bail!("{key} is not 64 lowercase hex characters");
        }
    }
    Ok(Expected {
        bag_run_hash,
        bag_output_digest,
        heartbeats_recorded_sha256,
        recorded_states,
    })
}

/// Checks every file named in MANIFEST.sha256 against its SHA-256. Names
/// must be regular files inside the capsule directory; the verifier never
/// follows a symlink or opens a path outside it. Every file the verifier
/// later reads must be listed exactly once; unlisted files in the directory
/// are ignored because they are never read. The first line names the
/// capsule kind (`# rook-capsule-v1 kind=...`); `shasum -c` ignores it.
/// Returns the kind and the covered names, or None (after printing the
/// failure) if the capsule must be refused.
fn verify_manifest(dir: &Path) -> Result<Option<(String, BTreeSet<String>)>> {
    let manifest_path = dir.join("MANIFEST.sha256");
    let text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("read manifest {}", manifest_path.display()))?;
    let Some(kind) = text
        .lines()
        .next()
        .and_then(|line| line.trim_end().strip_prefix(CAPSULE_HEADER))
        .filter(|kind| !kind.is_empty() && kind.bytes().all(|b| b.is_ascii_graphic()))
    else {
        println!("manifest: first line is not '{CAPSULE_HEADER}<kind>'");
        return Ok(None);
    };
    let mut verified = 0_usize;
    let mut names = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (hex, name) = line
            .split_once("  ")
            .with_context(|| format!("malformed manifest line: {line}"))?;
        if name.contains('/') || name.contains('\\') || name.contains("..") {
            bail!("manifest name escapes the capsule: {name}");
        }
        if !names.insert(name.to_string()) {
            println!("manifest: {name}: listed twice");
            return Ok(None);
        }
        let path = dir.join(name);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                println!("manifest: {name}: missing");
                return Ok(None);
            }
        };
        if !metadata.is_file() {
            println!("manifest: {name}: not a regular file");
            return Ok(None);
        }
        let Ok(bytes) = std::fs::read(&path) else {
            println!("manifest: {name}: missing");
            return Ok(None);
        };
        let file_hex = encode_hex(&Sha256::digest(&bytes));
        if file_hex != hex {
            println!("manifest: {name}: sha256 mismatch (manifest {hex}, file {file_hex})");
            return Ok(None);
        }
        verified += 1;
    }
    println!("manifest: {verified} files verified");
    Ok(Some((kind.to_string(), names)))
}
