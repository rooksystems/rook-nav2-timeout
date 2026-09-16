//! Records the reference scenarios against the `old` component through a
//! scripted environment and writes them as `native-adapter` capsules. The
//! environment script is the recording's only source of inputs; the
//! recorder writes every frame in the order it happened, effects included,
//! and captures each effect a second time as the environment saw it.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use rook_native::{
    COMPONENT_ACTOR_ID, End, EventHeader, EventType, FLAG_ATTEMPTED, FLAG_CONFIRMED, Frame,
    GAP_REASON_OVERRUN, Gap, SESSION_ACTOR_ID, SESSION_CHANNEL_ID, Session, encode_stream,
    hash_frames, raw_record_hash,
};
use sha2::{Digest, Sha256};

use crate::bodies::{
    self, CancelResponse, Clock, GoalResponse, GoalResult, GoalSend, STATE_METHOD_FRESH,
};
use crate::component::{Component, Consumed, Input, Variant, Wait};
use crate::property::{PROPERTY_TIMELY_ACK, PROPERTY_TIMEOUT_CANCEL};
use crate::{
    ACTOR_ACTION_SERVER, ACTOR_CLOCK, ACTOR_COMMAND, ADAPTER_NAME, CH_ACTION_CANCEL,
    CH_ACTION_GOAL, CH_ACTION_RESPONSE, CH_CLOCK, CH_GOAL_COMMAND, CLOCK_ROS, CORPUS,
    GOAL_RESPONSE_DEADLINE_NS, PROTOCOL_VERSION, encode_hex, source_identity,
};

/// What the scripted environment does next.
#[derive(Clone, Debug)]
pub enum Action {
    Command(Vec<u8>),
    /// Serves the component's pending clock read with this value.
    Clock(i64),
    GoalResponse {
        accepted: bool,
        stamp_ns: i64,
    },
    Result {
        status: u8,
    },
    CancelResponse {
        return_code: u8,
    },
    /// The recorder lost events here; the script ends.
    Gap,
}

pub struct Scenario {
    pub name: &'static str,
    pub property: &'static str,
    pub completion: &'static str,
    pub grade: &'static str,
    pub grade_reason: &'static str,
    pub actions: Vec<Action>,
}

pub fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "timeout",
            property: PROPERTY_TIMEOUT_CANCEL,
            completion: "cancel_request_issued",
            grade: "Complete",
            grade_reason: "scripted environment; every input the component consumed was recorded",
            actions: vec![
                Action::Command(b"goal 1".to_vec()),
                Action::Clock(0),
                Action::Clock(500_000_000),
                Action::Clock(1_200_000_000),
            ],
        },
        Scenario {
            name: "timely",
            property: PROPERTY_TIMELY_ACK,
            completion: "observation_interval_elapsed",
            grade: "Complete",
            grade_reason: "scripted environment; every input the component consumed was recorded",
            actions: vec![
                Action::Command(b"goal 1".to_vec()),
                Action::Clock(0),
                Action::Clock(200_000_000),
                Action::GoalResponse {
                    accepted: true,
                    stamp_ns: 200_000_000,
                },
                Action::Result {
                    status: bodies::RESULT_SUCCEEDED,
                },
            ],
        },
        Scenario {
            name: "command-gap",
            property: PROPERTY_TIMEOUT_CANCEL,
            completion: "cancel_request_issued",
            grade: "Inferred",
            grade_reason: "the recorder lost events right after the goal command; the component's first clock read was never recorded",
            actions: vec![Action::Command(b"goal 1".to_vec()), Action::Gap],
        },
        Scenario {
            name: "command-end",
            property: PROPERTY_TIMEOUT_CANCEL,
            completion: "cancel_request_issued",
            grade: "Complete",
            grade_reason: "scripted environment closed right after the goal command; the component's first clock read was never served",
            actions: vec![Action::Command(b"goal 1".to_vec())],
        },
        Scenario {
            name: "timeout-gap",
            property: PROPERTY_TIMEOUT_CANCEL,
            completion: "cancel_request_issued",
            grade: "Inferred",
            grade_reason: "the recorder admits lost events at the gap frame; replay is valid only before it",
            actions: vec![
                Action::Command(b"goal 1".to_vec()),
                Action::Clock(0),
                Action::Clock(500_000_000),
                Action::Gap,
            ],
        },
    ]
}

pub struct Recording {
    pub frames: Vec<Frame>,
    /// Effect payloads as the environment captured them, in receipt order.
    pub captured: Vec<Vec<u8>>,
}

struct Recorder {
    frames: Vec<Frame>,
    captured: Vec<Vec<u8>>,
    sequences: BTreeMap<(u32, u32), u64>,
}

impl Recorder {
    fn push(
        &mut self,
        src_actor: u32,
        dst_actor: u32,
        channel_id: u32,
        header: EventHeader,
        body: Vec<u8>,
    ) -> u64 {
        let ordinal = self.frames.len() as u64;
        let seq = self.sequences.entry((src_actor, channel_id)).or_insert(0);
        self.frames.push(Frame {
            ordinal,
            src_actor,
            dst_actor,
            channel_id,
            src_seq: *seq,
            observed_ns: ordinal * 1_000_000,
            header,
            body,
        });
        *seq += 1;
        ordinal
    }
}

fn header(event_type: EventType, flags: u32) -> EventHeader {
    EventHeader {
        event_type,
        schema: 1,
        flags,
    }
}

pub fn session_uuid(scenario: &str) -> [u8; 16] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"rook-adapter-ref session ");
    hasher.update(scenario.as_bytes());
    hasher.finalize().as_bytes()[..16].try_into().unwrap()
}

/// Records one scenario. The component is stepped in-process; the frames
/// it would have crossed the adapter protocol with are written directly.
pub fn record(variant: Variant, scenario: &Scenario) -> Result<Recording> {
    let mut recorder = Recorder {
        frames: Vec::new(),
        captured: Vec::new(),
        sequences: BTreeMap::new(),
    };
    let uuid = session_uuid(scenario.name);
    recorder.push(
        SESSION_ACTOR_ID,
        SESSION_ACTOR_ID,
        SESSION_CHANNEL_ID,
        header(EventType::Session, 0),
        Session {
            uuid,
            record_utc_ns: 1_757_000_000_000_000_000,
            pid: 4242,
        }
        .encode(),
    );
    recorder.push(
        SESSION_ACTOR_ID,
        COMPONENT_ACTOR_ID,
        SESSION_CHANNEL_ID,
        header(EventType::StartingState, 0),
        bodies::starting_state(STATE_METHOD_FRESH, *blake3::hash(b"").as_bytes()),
    );

    let mut component = Component::new(variant);
    // (recorded ordinal, sequence on its channel, goal id) of the last send.
    let mut last_goal: Option<(u64, u64, [u8; 16])> = None;
    let mut last_cancel: Option<(u64, u64, [u8; 16])> = None;

    for action in &scenario.actions {
        let (src_actor, channel_id, event_type, body) = match action {
            Action::Command(goal) => (
                ACTOR_COMMAND,
                CH_GOAL_COMMAND,
                EventType::Message,
                bodies::message(goal),
            ),
            Action::Clock(value_ns) => {
                let Some(Wait::ClockRead { callback, clock_id }) = component
                    .waits()
                    .into_iter()
                    .find(|wait| matches!(wait, Wait::ClockRead { .. }))
                else {
                    bail!("script serves a clock read nobody asked for");
                };
                if clock_id != CLOCK_ROS {
                    bail!("only the ROS clock is scripted");
                }
                (
                    ACTOR_CLOCK,
                    CH_CLOCK,
                    EventType::Clock,
                    Clock {
                        clock_id,
                        value_ns: *value_ns,
                        callback_ordinal: callback,
                    }
                    .encode(),
                )
            }
            Action::GoalResponse { accepted, stamp_ns } => {
                let (goal_send_ordinal, _, goal_id) = last_goal.context("no goal to answer")?;
                (
                    ACTOR_ACTION_SERVER,
                    CH_ACTION_RESPONSE,
                    EventType::GoalResponse,
                    GoalResponse {
                        goal_send_ordinal,
                        goal_id,
                        accepted: *accepted,
                        stamp_ns: *stamp_ns,
                    }
                    .encode(),
                )
            }
            Action::Result { status } => {
                let (goal_send_ordinal, _, goal_id) = last_goal.context("no goal to finish")?;
                (
                    ACTOR_ACTION_SERVER,
                    CH_ACTION_RESPONSE,
                    EventType::Result,
                    GoalResult {
                        goal_send_ordinal,
                        goal_id,
                        status: *status,
                        result: b"done".to_vec(),
                    }
                    .encode(),
                )
            }
            Action::CancelResponse { return_code } => {
                let (cancel_send_ordinal, _, goal_id) =
                    last_cancel.context("no cancel to answer")?;
                (
                    ACTOR_ACTION_SERVER,
                    CH_ACTION_RESPONSE,
                    EventType::CancelResponse,
                    CancelResponse {
                        cancel_send_ordinal,
                        return_code: *return_code,
                        goals: vec![goal_id],
                    }
                    .encode()?,
                )
            }
            Action::Gap => {
                let gap = Gap {
                    reason: GAP_REASON_OVERRUN,
                    lost: Some(1),
                    detail: b"scripted recorder overrun".to_vec(),
                };
                recorder.push(
                    SESSION_ACTOR_ID,
                    SESSION_ACTOR_ID,
                    SESSION_CHANNEL_ID,
                    header(EventType::Gap, gap.flags()),
                    gap.encode().context("gap detail fits")?,
                );
                break;
            }
        };
        let ordinal = recorder.push(
            src_actor,
            COMPONENT_ACTOR_ID,
            channel_id,
            header(event_type, 0),
            body,
        );
        let input = recorder.frames[ordinal as usize].clone();
        let request = match input.header.event_type {
            EventType::GoalResponse | EventType::Result => {
                last_goal.map(|(_, seq, _)| (CH_ACTION_GOAL, seq))
            }
            EventType::CancelResponse => last_cancel.map(|(_, seq, _)| (CH_ACTION_CANCEL, seq)),
            _ => None,
        };
        let outcome = component.step(Input {
            ordinal,
            src_actor: input.src_actor,
            dst_actor: input.dst_actor,
            channel_id: input.channel_id,
            event_type: input.header.event_type,
            body: &input.body,
            request,
        });
        if let Some(Consumed::No(reason) | Consumed::Refused(reason)) = outcome.consumed {
            bail!("scripted input at ordinal {ordinal} was not consumed: {reason}");
        }
        for effect in outcome.effects {
            let flags = effect.flags & (FLAG_ATTEMPTED | FLAG_CONFIRMED);
            let body = effect.body.clone();
            let effect_ordinal = recorder.push(
                COMPONENT_ACTOR_ID,
                effect.dst_actor,
                effect.channel_id,
                header(effect.event_type, flags),
                body,
            );
            let frame = &recorder.frames[effect_ordinal as usize];
            if frame.src_seq != effect.src_seq {
                bail!("component and recorder disagree on an effect sequence");
            }
            recorder.captured.push(frame.payload());
            match effect.event_type {
                EventType::GoalSend => {
                    last_goal = Some((
                        effect_ordinal,
                        effect.src_seq,
                        GoalSend::decode(&effect.body)?.goal_id,
                    ));
                }
                EventType::CancelSend => {
                    last_cancel = Some((
                        effect_ordinal,
                        effect.src_seq,
                        bodies::CancelSend::decode(&effect.body)?.goal_id,
                    ));
                }
                _ => {}
            }
        }
    }
    let frame_count = recorder.frames.len() as u64;
    recorder.push(
        SESSION_ACTOR_ID,
        SESSION_ACTOR_ID,
        SESSION_CHANNEL_ID,
        header(EventType::End, 0),
        End { uuid, frame_count }.encode(),
    );
    Ok(Recording {
        frames: recorder.frames,
        captured: recorder.captured,
    })
}

/// `effects_captured.bin`: u32 LE length then payload, per captured effect.
pub fn encode_captured(captured: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for payload in captured {
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
    }
    out
}

pub fn decode_captured(bytes: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut captured = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let length = u32::from_le_bytes(
            bytes
                .get(cursor..cursor + 4)
                .context("truncated captured effect header")?
                .try_into()?,
        ) as usize;
        captured.push(
            bytes
                .get(cursor + 4..cursor + 4 + length)
                .context("truncated captured effect")?
                .to_vec(),
        );
        cursor += 4 + length;
    }
    Ok(captured)
}

/// Every file of a `native-adapter` capsule for one scenario, as bytes.
pub fn capsule_files(scenario: &Scenario) -> Result<BTreeMap<&'static str, Vec<u8>>> {
    let recording = record(Variant::Old, scenario)?;
    let events = encode_stream(&recording.frames);
    let captured = encode_captured(&recording.captured);
    let hashes = hash_frames(CORPUS.as_bytes(), &recording.frames);
    let expected = format!(
        "# Expected values for the {} reference scenario. Recorded from the old\n\
         # component through the scripted environment in crates/rook-adapter-ref.\n\
         raw_record_blake3 = {}\n\
         effects_captured_blake3 = {}\n\
         trace_hash = {}\n\
         run_hash = {}\n\
         output_digest = {}\n\
         grade = {}\n\
         claim = measured\n",
        scenario.name,
        encode_hex(&raw_record_hash(&events)),
        encode_hex(&raw_record_hash(&captured)),
        encode_hex(&hashes.trace_hash),
        encode_hex(&hashes.run_hash),
        encode_hex(&hashes.output_digest),
        scenario.grade,
    );
    let sources = source_identity();
    let identity = format!(
        "# Identity manifest of the reference case. Content identities are BLAKE3\n\
         # over the source embedded in the reference binaries; the runner pins the\n\
         # binaries it runs.\n\
         component = goal-client\n\
         component_variant = old\n\
         component_source_blake3 = {}\n\
         adapter = {ADAPTER_NAME}\n\
         adapter_protocol = {PROTOCOL_VERSION}\n\
         adapter_source_blake3 = {}\n\
         property = {}\n\
         property_program = rook-property-ref\n\
         property_input_schema = rook-property-input@1\n\
         property_source_blake3 = {}\n\
         normalization = none@1\n\
         normalization_program = rook-adapter-ref driver\n\
         normalization_source_blake3 = {}\n\
         starting_state = fresh\n\
         starting_state_blake3 = {}\n\
         effect_record_order = after-transport\n\
         {}\
         toolchain = rust 1.95 (workspace rust-version)\n\
         executable = built from public source; no ROS, no plugins, no shared libraries beyond the Rust toolchain\n",
        sources["component_source_blake3"],
        sources["adapter_source_blake3"],
        scenario.property,
        sources["property_source_blake3"],
        sources["normalization_source_blake3"],
        encode_hex(blake3::hash(b"").as_bytes()),
        endpoint_and_channel_lines(),
    );
    let case = format!(
        "# The case: what was recorded, what the property asks, where it ends.\n\
         corpus = {CORPUS}\n\
         scenario = {}\n\
         property = {}\n\
         scope.completion = {}\n\
         scope.observation_end = end_of_recording\n\
         comparison_policy = raw-bytes-in-order@1\n\
         goal_response_deadline_ns = {GOAL_RESPONSE_DEADLINE_NS}\n\
         grade_reason = {}\n\
         origin = scripted reference environment, not a field recording\n",
        scenario.name, scenario.property, scenario.completion, scenario.grade_reason,
    );
    let mut files: BTreeMap<&'static str, Vec<u8>> = BTreeMap::new();
    files.insert("events.bin", events);
    files.insert("effects_captured.bin", captured);
    files.insert("expected", expected.into_bytes());
    files.insert("identity", identity.into_bytes());
    files.insert("case", case.into_bytes());
    let mut manifest = String::from("# rook-capsule-v1 kind=native-adapter\n");
    for (name, bytes) in &files {
        manifest.push_str(&format!("{}  {name}\n", encode_hex(&Sha256::digest(bytes))));
    }
    files.insert("MANIFEST.sha256", manifest.into_bytes());
    Ok(files)
}

/// The adapter's endpoint and channel tables as identity lines, one per
/// actor and channel, so a reader can resolve every id in the recording.
fn endpoint_and_channel_lines() -> String {
    let mut lines = String::new();
    for endpoint in crate::adapter::endpoints() {
        lines.push_str(&format!(
            "endpoint.{} = {}\n",
            endpoint.actor, endpoint.name
        ));
    }
    for channel in crate::adapter::channels() {
        lines.push_str(&format!(
            "channel.{} = {} {} {}\n",
            channel.id,
            channel.name,
            channel.direction,
            channel.event_types.join(",")
        ));
    }
    lines
}

pub fn write_capsule(dir: &Path, scenario: &Scenario) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, bytes) in capsule_files(scenario)? {
        std::fs::write(dir.join(name), bytes)?;
    }
    Ok(())
}
