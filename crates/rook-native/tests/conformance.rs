//! Native ordering conformance. Every stream here is built from the API,
//! compared byte for byte with the committed fixture, and checked against
//! the committed expected hashes or refusal. Set ROOK_WRITE_FIXTURES=1 to
//! rewrite the fixture directory; that is a golden change and only happens
//! when a session was named this directory and told to.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rook_native::{
    COMPONENT_ACTOR_ID, End, EventHeader, EventType, FLAG_ATTEMPTED, FLAG_CONFIRMED,
    FLAG_COUNT_UNKNOWN, Frame, GAP_REASON_OVERRUN, Gap, NATIVE_ENVELOPE_VERSION, SESSION_ACTOR_ID,
    SESSION_CHANNEL_ID, Session, StreamError, decode_stream, encode_stream, hash_frames,
    raw_record_hash,
};

const CORPUS: &[u8] = b"conformance@1";
const ENVIRONMENT_ACTOR_ID: u32 = 2;
const COMMAND_CHANNEL: u32 = 10;
const PUBLISH_CHANNEL: u32 = 20;
const UUID: [u8; 16] = *b"rook-conformance";

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/native-conformance")
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Assigns ordinals and observed times in list order; sequences are the
/// caller's so a test can break them on purpose.
fn stream(events: Vec<(u32, u32, u32, u64, EventHeader, Vec<u8>)>) -> Vec<Frame> {
    events
        .into_iter()
        .enumerate()
        .map(
            |(ordinal, (src_actor, dst_actor, channel_id, src_seq, header, body))| Frame {
                ordinal: ordinal as u64,
                src_actor,
                dst_actor,
                channel_id,
                src_seq,
                observed_ns: ordinal as u64 * 1_000_000,
                header,
                body,
            },
        )
        .collect()
}

fn header(event_type: EventType, flags: u32) -> EventHeader {
    EventHeader {
        event_type,
        schema: 1,
        flags,
    }
}

fn session() -> (u32, u32, u32, u64, EventHeader, Vec<u8>) {
    (
        SESSION_ACTOR_ID,
        SESSION_ACTOR_ID,
        SESSION_CHANNEL_ID,
        0,
        header(EventType::Session, 0),
        Session {
            uuid: UUID,
            record_utc_ns: 1_757_000_000_000_000_000,
            pid: 4242,
        }
        .encode(),
    )
}

fn end(frame_count: u64, seq: u64) -> (u32, u32, u32, u64, EventHeader, Vec<u8>) {
    (
        SESSION_ACTOR_ID,
        SESSION_ACTOR_ID,
        SESSION_CHANNEL_ID,
        seq,
        header(EventType::End, 0),
        End {
            uuid: UUID,
            frame_count,
        }
        .encode(),
    )
}

/// Message body: serialization 2 (raw) then the bytes.
fn input(seq: u64, text: &[u8]) -> (u32, u32, u32, u64, EventHeader, Vec<u8>) {
    let mut body = 2_u16.to_le_bytes().to_vec();
    body.extend_from_slice(text);
    (
        ENVIRONMENT_ACTOR_ID,
        COMPONENT_ACTOR_ID,
        COMMAND_CHANNEL,
        seq,
        header(EventType::Message, 0),
        body,
    )
}

/// Publish body: transport status 0, serialization 2, then the bytes.
fn output(seq: u64, text: &[u8]) -> (u32, u32, u32, u64, EventHeader, Vec<u8>) {
    let mut body = 0_i32.to_le_bytes().to_vec();
    body.extend_from_slice(&2_u16.to_le_bytes());
    body.extend_from_slice(text);
    (
        COMPONENT_ACTOR_ID,
        ENVIRONMENT_ACTOR_ID,
        PUBLISH_CHANNEL,
        seq,
        header(EventType::Publish, FLAG_ATTEMPTED | FLAG_CONFIRMED),
        body,
    )
}

fn gap(seq: u64) -> (u32, u32, u32, u64, EventHeader, Vec<u8>) {
    let gap = Gap {
        reason: GAP_REASON_OVERRUN,
        lost: Some(3),
        detail: b"recorder queue overrun".to_vec(),
    };
    (
        SESSION_ACTOR_ID,
        SESSION_ACTOR_ID,
        SESSION_CHANNEL_ID,
        seq,
        header(EventType::Gap, gap.flags()),
        gap.encode().expect("short detail"),
    )
}

fn interleaved() -> Vec<u8> {
    encode_stream(&stream(vec![
        session(),
        input(0, b"A"),
        output(0, b"a"),
        input(1, b"B"),
        output(1, b"b"),
        end(5, 1),
    ]))
}

fn grouped() -> Vec<u8> {
    encode_stream(&stream(vec![
        session(),
        input(0, b"A"),
        input(1, b"B"),
        output(0, b"a"),
        output(1, b"b"),
        end(5, 1),
    ]))
}

fn with_gap() -> Vec<u8> {
    encode_stream(&stream(vec![
        session(),
        input(0, b"A"),
        output(0, b"a"),
        gap(1),
        end(4, 2),
    ]))
}

/// Frame boundaries of an encoded stream, for byte-level surgery.
fn frame_ranges(bytes: &[u8]) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let len = u32::from_le_bytes(bytes[cursor + 28..cursor + 32].try_into().unwrap()) as usize;
        ranges.push(cursor..cursor + 80 + len);
        cursor += 80 + len;
    }
    ranges
}

fn swap_frames(bytes: &[u8], first: usize, second: usize) -> Vec<u8> {
    let ranges = frame_ranges(bytes);
    let mut out = Vec::new();
    for (index, range) in ranges.iter().enumerate() {
        let source = if index == first {
            &ranges[second]
        } else if index == second {
            &ranges[first]
        } else {
            range
        };
        out.extend_from_slice(&bytes[source.clone()]);
    }
    out
}

struct Case {
    name: &'static str,
    bytes: Vec<u8>,
    outcome: Result<(), StreamError>,
}

fn cases() -> Vec<Case> {
    let interleaved_bytes = interleaved();
    let ranges = frame_ranges(&interleaved_bytes);
    let mut cases = vec![
        Case {
            name: "interleaved",
            bytes: interleaved_bytes.clone(),
            outcome: Ok(()),
        },
        Case {
            name: "grouped",
            bytes: grouped(),
            outcome: Ok(()),
        },
        Case {
            name: "gap",
            bytes: with_gap(),
            outcome: Ok(()),
        },
        // Two frames swapped verbatim: the ordinals no longer count up.
        Case {
            name: "reordered",
            bytes: swap_frames(&interleaved_bytes, 1, 2),
            outcome: Err(StreamError::NonContiguousOrdinal {
                expected: 1,
                found: 2,
            }),
        },
        // Inputs swapped and ordinals renumbered: the per-source sequence
        // still tells.
        Case {
            name: "reordered-renumbered",
            bytes: encode_stream(&stream(vec![
                session(),
                input(1, b"B"),
                output(0, b"a"),
                input(0, b"A"),
                output(1, b"b"),
                end(5, 1),
            ])),
            outcome: Err(StreamError::SequenceGap {
                ordinal: 1,
                expected: 0,
                found: 1,
            }),
        },
        // Output a removed and ordinals renumbered, sequence untouched.
        Case {
            name: "omitted",
            bytes: encode_stream(&stream(vec![
                session(),
                input(0, b"A"),
                input(1, b"B"),
                output(1, b"b"),
                end(4, 1),
            ])),
            outcome: Err(StreamError::SequenceGap {
                ordinal: 3,
                expected: 0,
                found: 1,
            }),
        },
        // Output a removed with everything renumbered: a valid stream whose
        // hashes differ from the interleaved expected values.
        Case {
            name: "omitted-renumbered",
            bytes: encode_stream(&stream(vec![
                session(),
                input(0, b"A"),
                input(1, b"B"),
                output(0, b"b"),
                end(4, 1),
            ])),
            outcome: Ok(()),
        },
        // Output a duplicated with its sequence: refused.
        Case {
            name: "extra",
            bytes: encode_stream(&stream(vec![
                session(),
                input(0, b"A"),
                output(0, b"a"),
                output(0, b"a"),
                input(1, b"B"),
                output(1, b"b"),
                end(6, 1),
            ])),
            outcome: Err(StreamError::SequenceGap {
                ordinal: 3,
                expected: 1,
                found: 0,
            }),
        },
        // An extra output with fresh sequences: valid, different hashes.
        Case {
            name: "extra-renumbered",
            bytes: encode_stream(&stream(vec![
                session(),
                input(0, b"A"),
                output(0, b"a"),
                output(1, b"a"),
                input(1, b"B"),
                output(2, b"b"),
                end(6, 1),
            ])),
            outcome: Ok(()),
        },
        // The end marker never arrived.
        Case {
            name: "truncated",
            bytes: interleaved_bytes[..ranges[5].start].to_vec(),
            outcome: Err(StreamError::MissingEnd),
        },
        // The file ends inside output b.
        Case {
            name: "torn",
            bytes: interleaved_bytes[..ranges[4].start + 85].to_vec(),
            outcome: Err(StreamError::Truncated {
                offset: ranges[4].start,
            }),
        },
        // A tail cut after output a and closed with a fresh end marker: a
        // valid stream, so only the expected hashes can catch it.
        Case {
            name: "forged-end",
            bytes: encode_stream(&stream(vec![
                session(),
                input(0, b"A"),
                output(0, b"a"),
                end(3, 1),
            ])),
            outcome: Ok(()),
        },
    ];
    // Hash-consistent headers outside the format: an unknown schema and a
    // reserved flag bit, each on input A.
    let mut bad_schema = stream(vec![session(), input(0, b"A"), end(2, 1)]);
    bad_schema[1].header.schema = 2;
    cases.push(Case {
        name: "bad-schema",
        bytes: encode_stream(&bad_schema),
        outcome: Err(StreamError::UnsupportedSchema {
            ordinal: 1,
            found: 2,
        }),
    });
    let mut reserved_flags = stream(vec![session(), input(0, b"A"), end(2, 1)]);
    reserved_flags[1].header.flags = 0x8000_0000;
    cases.push(Case {
        name: "reserved-flags",
        bytes: encode_stream(&reserved_flags),
        outcome: Err(StreamError::ReservedFlags {
            ordinal: 1,
            found: 0x8000_0000,
        }),
    });
    // An end marker from an actor other than the session.
    let mut stray_end = stream(vec![session(), input(0, b"A"), end(2, 1)]);
    stray_end[2].src_actor = ENVIRONMENT_ACTOR_ID;
    stray_end[2].src_seq = 0;
    cases.push(Case {
        name: "stray-end",
        bytes: encode_stream(&stray_end),
        outcome: Err(StreamError::MalformedMarker { ordinal: 2 }),
    });
    // A gap whose COUNT_UNKNOWN flag disagrees with its body.
    let mut gap_flag = stream(vec![session(), input(0, b"A"), gap(1), end(3, 2)]);
    gap_flag[2].header.flags = FLAG_COUNT_UNKNOWN;
    cases.push(Case {
        name: "gap-flag-mismatch",
        bytes: encode_stream(&gap_flag),
        outcome: Err(StreamError::MalformedMarker { ordinal: 2 }),
    });
    // A message with no serialization field: hash-consistent, refused by
    // layout.
    let mut short_message = stream(vec![session(), input(0, b"A"), end(2, 1)]);
    short_message[1].body.clear();
    cases.push(Case {
        name: "short-body",
        bytes: encode_stream(&short_message),
        outcome: Err(StreamError::MalformedBody {
            ordinal: 1,
            event_type: EventType::Message,
        }),
    });
    let mut wasm_version = interleaved_bytes.clone();
    wasm_version[4..6].copy_from_slice(&1_u16.to_le_bytes());
    cases.push(Case {
        name: "wasm-version",
        bytes: wasm_version,
        outcome: Err(StreamError::WrongEnvelopeVersion {
            ordinal: 0,
            found: 1,
        }),
    });
    cases
}

fn expected_lines(cases: &[Case]) -> String {
    let mut lines = vec![
        "# Native conformance fixtures, one block per stream. Hashes are hex".to_string(),
        "# BLAKE3; refusals are the rook-native StreamError. Corpus conformance@1.".to_string(),
    ];
    for case in cases {
        match &case.outcome {
            Ok(()) => {
                let frames = decode_stream(&case.bytes).expect(case.name);
                let hashes = hash_frames(CORPUS, &frames);
                lines.push(format!(
                    "{}.raw_record_blake3 = {}",
                    case.name,
                    encode_hex(&raw_record_hash(&case.bytes))
                ));
                lines.push(format!(
                    "{}.trace_hash = {}",
                    case.name,
                    encode_hex(&hashes.trace_hash)
                ));
                lines.push(format!(
                    "{}.run_hash = {}",
                    case.name,
                    encode_hex(&hashes.run_hash)
                ));
                lines.push(format!(
                    "{}.output_digest = {}",
                    case.name,
                    encode_hex(&hashes.output_digest)
                ));
            }
            Err(error) => {
                lines.push(format!("{}.refusal = {error:?}", case.name));
            }
        }
    }
    lines.join("\n") + "\n"
}

fn read_expected(path: &Path) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(path).expect("read expected");
    text.lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (key, value) = line.split_once('=').expect("key = value");
            (key.trim().to_string(), value.trim().to_string())
        })
        .collect()
}

#[test]
fn fixtures_match_the_committed_bytes_and_expected_values() {
    let dir = fixture_dir();
    let cases = cases();
    let expected_text = expected_lines(&cases);
    if std::env::var_os("ROOK_WRITE_FIXTURES").is_some() {
        std::fs::create_dir_all(&dir).unwrap();
        for case in &cases {
            std::fs::write(dir.join(format!("{}.bin", case.name)), &case.bytes).unwrap();
        }
        std::fs::write(dir.join("expected"), &expected_text).unwrap();
    }
    for case in &cases {
        let committed = std::fs::read(dir.join(format!("{}.bin", case.name)))
            .unwrap_or_else(|_| panic!("fixture {}.bin missing", case.name));
        assert_eq!(committed, case.bytes, "fixture {} drifted", case.name);
        assert_eq!(
            decode_stream(&case.bytes).map(|_| ()),
            case.outcome,
            "outcome of {}",
            case.name
        );
    }
    let committed = read_expected(&dir.join("expected"));
    let produced = read_expected_from_text(&expected_text);
    assert_eq!(produced, committed, "expected values drifted");
}

fn read_expected_from_text(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (key, value) = line.split_once('=').unwrap();
            (key.trim().to_string(), value.trim().to_string())
        })
        .collect()
}

#[test]
fn interleaving_is_committed_by_every_execution_hash() {
    let interleaved = hash_frames(CORPUS, &decode_stream(&interleaved()).unwrap());
    let grouped = hash_frames(CORPUS, &decode_stream(&grouped()).unwrap());
    assert_ne!(interleaved.trace_hash, grouped.trace_hash);
    assert_ne!(interleaved.run_hash, grouped.run_hash);
    assert_ne!(interleaved.output_digest, grouped.output_digest);
}

#[test]
fn valid_but_different_streams_never_reproduce_the_expected_hashes() {
    let reference = hash_frames(CORPUS, &decode_stream(&interleaved()).unwrap());
    for case in cases() {
        if case.outcome.is_ok() && case.name != "interleaved" {
            let hashes = hash_frames(CORPUS, &decode_stream(&case.bytes).unwrap());
            assert_ne!(hashes.run_hash, reference.run_hash, "{}", case.name);
            assert_ne!(
                hashes.output_digest, reference.output_digest,
                "{}",
                case.name
            );
        }
    }
}

#[test]
fn observed_time_is_outside_every_execution_hash() {
    let mut frames = decode_stream(&interleaved()).unwrap();
    let before = hash_frames(CORPUS, &frames);
    let raw_before = raw_record_hash(&encode_stream(&frames));
    for frame in &mut frames {
        frame.observed_ns += 7;
    }
    assert_eq!(hash_frames(CORPUS, &frames), before);
    assert_ne!(raw_record_hash(&encode_stream(&frames)), raw_before);
}

#[test]
fn every_frame_carries_the_native_envelope_version() {
    let bytes = interleaved();
    for range in frame_ranges(&bytes) {
        assert_eq!(&bytes[range.start..range.start + 4], b"RKE1");
        assert_eq!(
            u16::from_le_bytes([bytes[range.start + 4], bytes[range.start + 5]]),
            NATIVE_ENVELOPE_VERSION
        );
    }
}

#[test]
fn a_gap_detail_that_does_not_fit_is_not_encoded() {
    let gap = Gap {
        reason: GAP_REASON_OVERRUN,
        lost: None,
        detail: vec![b'x'; 65_536],
    };
    assert_eq!(gap.encode(), None);
}

#[test]
fn a_second_session_marker_is_refused() {
    let mut second = session();
    second.3 = 1;
    let bytes = encode_stream(&stream(vec![session(), second, end(2, 2)]));
    assert_eq!(
        decode_stream(&bytes).map(|_| ()),
        Err(StreamError::UnexpectedSession { ordinal: 1 })
    );
}

#[test]
fn end_with_wrong_count_is_refused() {
    let bytes = encode_stream(&stream(vec![session(), input(0, b"A"), end(7, 1)]));
    assert_eq!(
        decode_stream(&bytes).map(|_| ()),
        Err(StreamError::EndMismatch { ordinal: 2 })
    );
}

#[test]
fn frames_after_end_are_refused() {
    let bytes = encode_stream(&stream(vec![session(), end(1, 1), input(0, b"A")]));
    assert_eq!(
        decode_stream(&bytes).map(|_| ()),
        Err(StreamError::UnexpectedEnd { ordinal: 1 })
    );
}

#[test]
fn a_flipped_payload_byte_is_refused_at_its_ordinal() {
    let mut bytes = interleaved();
    let ranges = frame_ranges(&bytes);
    let last = ranges[2].end - 1;
    bytes[last] ^= 1;
    assert_eq!(
        decode_stream(&bytes).map(|_| ()),
        Err(StreamError::PayloadHashMismatch { ordinal: 2 })
    );
}
