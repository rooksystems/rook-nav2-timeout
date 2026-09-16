//! Drives any adapter over stdio through one recording, collects the
//! replayed stream, compares effects with the recording under the
//! reference comparison policy (raw bytes, in order, full multiplicity;
//! normalization none@1), and builds the property input. This is the
//! reference driver for the tests and the documentation; #18's runner is
//! the product.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail};
use rook_native::{
    COMPONENT_ACTOR_ID, End, EnvelopeKind, EventHeader, EventType, Frame, Gap, NativeHashes,
    SESSION_ACTOR_ID, SESSION_CHANNEL_ID, Session, hash_frames,
};

use crate::protocol::{
    FinishReason, Pending, Request, RequestRef, Response, StartingState, StepStatus, WireFrame,
};
use crate::{decode_hex, encode_hex, event_type_name, parse_event_type};

/// Why the driver stopped delivering recorded inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Every recorded input was delivered and the end marker reached.
    Exhausted,
    /// The recording admits lost events here; nothing after it is delivered.
    Gap { ordinal: u64, gap: Gap },
    /// The adapter refused a recorded input as no longer valid.
    Refused { ordinal: u64, reason: String },
}

/// One delivered input: its recorded ordinal, its ordinal in the replayed
/// stream (None if the adapter refused it and it was dropped), the
/// adapter's status and reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub recorded_ordinal: u64,
    pub replayed_ordinal: Option<u64>,
    pub status: StepStatus,
    pub reason: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Run {
    /// The replayed stream: session and starting-state markers, every
    /// delivered and not-refused input, every effect in the order the
    /// adapter emitted it, and an end marker, renumbered from zero.
    pub frames: Vec<Frame>,
    pub steps: Vec<Step>,
    /// (callback recorded ordinal, replayed ordinal of the effect).
    pub effects: Vec<(u64, u64)>,
    /// (callback recorded ordinal, progress name, replayed ordinal reached).
    pub progress: Vec<(u64, String, u64)>,
    /// Pending operations reported by the last step and by finish.
    pub pending_after_last_step: Vec<Pending>,
    pub pending_at_finish: Vec<Pending>,
    pub stop: Stop,
    pub finish_reason: FinishReason,
}

pub fn wire_frame(frame: &Frame) -> WireFrame {
    WireFrame {
        ordinal: Some(frame.ordinal),
        src_actor: frame.src_actor,
        dst_actor: frame.dst_actor,
        channel_id: frame.channel_id,
        src_seq: frame.src_seq,
        event_type: event_type_name(frame.header.event_type),
        schema: frame.header.schema,
        flags: frame.header.flags,
        body_hex: encode_hex(&frame.body),
    }
}

fn frame_from_wire(ordinal: u64, wire: &WireFrame) -> Result<Frame> {
    Ok(Frame {
        ordinal,
        src_actor: wire.src_actor,
        dst_actor: wire.dst_actor,
        channel_id: wire.channel_id,
        src_seq: wire.src_seq,
        observed_ns: 0,
        header: EventHeader {
            event_type: parse_event_type(&wire.event_type)
                .with_context(|| format!("unknown event type {}", wire.event_type))?,
            schema: wire.schema,
            flags: wire.flags,
        },
        body: decode_hex(&wire.body_hex)?,
    })
}

/// For a response input, the recorded request frame its body names. The
/// request ordinal is the leading u64 of every response body in the
/// contract; the frame it names must be an effect of the component.
pub fn request_ref(recording: &[Frame], input: &Frame) -> Result<Option<RequestRef>> {
    let names_request = matches!(
        input.header.event_type,
        EventType::GoalResponse
            | EventType::Feedback
            | EventType::Result
            | EventType::CancelResponse
            | EventType::ServiceResponse
    );
    if !names_request {
        return Ok(None);
    }
    let request_ordinal = u64::from_le_bytes(
        input
            .body
            .get(0..8)
            .with_context(|| {
                format!(
                    "response at ordinal {} has no request ordinal",
                    input.ordinal
                )
            })?
            .try_into()?,
    );
    let request = recording
        .get(request_ordinal as usize)
        .filter(|frame| frame.ordinal == request_ordinal)
        .with_context(|| {
            format!(
                "response at ordinal {} names request ordinal {request_ordinal}, which the recording does not hold",
                input.ordinal
            )
        })?;
    if request.kind() != EnvelopeKind::Emit
        || request.src_actor != COMPONENT_ACTOR_ID
        || request.ordinal >= input.ordinal
    {
        bail!(
            "response at ordinal {} names ordinal {request_ordinal}, which is not an earlier effect of the component",
            input.ordinal
        );
    }
    Ok(Some(RequestRef {
        ordinal: request.ordinal,
        src_actor: request.src_actor,
        dst_actor: request.dst_actor,
        channel_id: request.channel_id,
        src_seq: request.src_seq,
    }))
}

struct AdapterProcess {
    child: Child,
    stdin: Option<std::process::ChildStdin>,
    stdout: BufReader<std::process::ChildStdout>,
}

impl AdapterProcess {
    fn spawn(command: &mut Command) -> Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .context("spawn adapter")?;
        let stdin = child.stdin.take().context("adapter stdin")?;
        let stdout = BufReader::new(child.stdout.take().context("adapter stdout")?);
        Ok(AdapterProcess {
            child,
            stdin: Some(stdin),
            stdout,
        })
    }

    fn send(&mut self, request: &Request) -> Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .context("adapter stdin already closed")?;
        serde_json::to_writer(&mut *stdin, request)?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
    }

    fn receive(&mut self) -> Result<Response> {
        let mut line = String::new();
        if self.stdout.read_line(&mut line)? == 0 {
            bail!("adapter closed its stdout");
        }
        serde_json::from_str(&line).with_context(|| format!("parse adapter line: {line}"))
    }

    /// Closes stdin first so an adapter that waits for EOF can exit.
    fn finish(&mut self) -> Result<()> {
        self.stdin.take();
        self.child.wait()?;
        Ok(())
    }
}

/// Never leaves an adapter behind: an early error kills and reaps it.
impl Drop for AdapterProcess {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Replays `recording` (a decoded native stream) through the adapter
/// started by `command`. Recorded effects are never sent; the adapter's own
/// effects are appended where they occur.
pub fn replay(
    command: &mut Command,
    recording: &[Frame],
    corpus: &str,
    scenario: &str,
) -> Result<Run> {
    let session_frame = recording.first().context("empty recording")?;
    let session = Session::decode(&session_frame.body)?;
    let requested_state = StartingState {
        method: "fresh".to_string(),
        blake3: crate::adapter::fresh_state_blake3(),
    };
    let mut adapter = AdapterProcess::spawn(command)?;
    adapter.send(&Request::Start {
        protocol: crate::PROTOCOL_VERSION,
        corpus: corpus.to_string(),
        scenario: scenario.to_string(),
        state: requested_state.clone(),
    })?;
    match adapter.receive()? {
        Response::Started {
            protocol, state, ..
        } if protocol == crate::PROTOCOL_VERSION => {
            if state.method != requested_state.method || state.blake3 != requested_state.blake3 {
                bail!(
                    "adapter started in state {}:{} instead of the requested {}:{}",
                    state.method,
                    state.blake3,
                    requested_state.method,
                    requested_state.blake3
                );
            }
        }
        Response::Refused { reason } => bail!("adapter refused to start: {reason}"),
        other => bail!("unexpected adapter response to start: {other:?}"),
    }

    let mut frames: Vec<Frame> = Vec::new();
    let mut steps = Vec::new();
    let mut effects = Vec::new();
    let mut progress = Vec::new();
    let mut pending_after_last_step = Vec::new();
    let mut stop = Stop::Exhausted;
    let mut session_sequence = 0_u64;

    // Effect sequences the adapter must count from zero per channel, so
    // the replayed stream decodes under the same rules as a recording.
    let mut effect_sequences: std::collections::BTreeMap<u32, u64> =
        std::collections::BTreeMap::new();
    let push = |frames: &mut Vec<Frame>, mut frame: Frame| -> u64 {
        frame.ordinal = frames.len() as u64;
        frame.observed_ns = 0;
        frames.push(frame);
        frames.len() as u64 - 1
    };

    'deliver: for recorded in recording {
        match recorded.header.event_type {
            EventType::Session => {
                push(&mut frames, recorded.clone());
                session_sequence = recorded.src_seq + 1;
                continue;
            }
            EventType::End => break,
            EventType::Gap => {
                stop = Stop::Gap {
                    ordinal: recorded.ordinal,
                    gap: Gap::decode(recorded.ordinal, &recorded.body)?,
                };
                break;
            }
            _ if recorded.kind() == EnvelopeKind::Emit => continue,
            _ => {}
        }
        let request = request_ref(recording, recorded)?;
        if let Some(request) = &request {
            // The recorded response answers the recorded request. It is
            // valid for this run only if this run issued an equivalent
            // request: same channel and sequence, same destination, same
            // type and bytes. Otherwise the response belongs to a request
            // this run never made and is not consumed.
            let recorded_request = &recording[request.ordinal as usize];
            let issued = frames.iter().find(|frame| {
                frame.kind() == EnvelopeKind::Emit
                    && frame.src_actor == COMPONENT_ACTOR_ID
                    && frame.channel_id == request.channel_id
                    && frame.src_seq == request.src_seq
            });
            let equivalent = issued.is_some_and(|issued| {
                issued.dst_actor == recorded_request.dst_actor
                    && issued.header.event_type == recorded_request.header.event_type
                    && issued.body == recorded_request.body
            });
            if !equivalent {
                let reason = match issued {
                    Some(_) => format!(
                        "recorded response at ordinal {} answers recorded request {}, but this run's request on channel {} sequence {} differs from it",
                        recorded.ordinal, request.ordinal, request.channel_id, request.src_seq
                    ),
                    None => format!(
                        "recorded response at ordinal {} answers recorded request {}, which this run never issued",
                        recorded.ordinal, request.ordinal
                    ),
                };
                steps.push(Step {
                    recorded_ordinal: recorded.ordinal,
                    replayed_ordinal: None,
                    status: StepStatus::Refused,
                    reason: Some(reason.clone()),
                });
                stop = Stop::Refused {
                    ordinal: recorded.ordinal,
                    reason,
                };
                break;
            }
        }
        adapter.send(&Request::Step {
            input: wire_frame(recorded),
            request,
        })?;
        // The callbacks an effect of this step may belong to: the input's
        // own (a new callback), the one a clock observation names, or the
        // one that issued the request a response answers. The adapter's
        // claim is checked against this, never trusted.
        let mut authorized: Vec<u64> = vec![recorded.ordinal];
        if recorded.header.event_type == EventType::Clock
            && let Some(callback) = recorded
                .body
                .get(10..18)
                .map(|bytes| u64::from_le_bytes(bytes.try_into().unwrap()))
        {
            authorized.push(callback);
        }
        if let Some(request) = &request
            && let Some(issued) = frames.iter().find(|frame| {
                frame.kind() == EnvelopeKind::Emit
                    && frame.channel_id == request.channel_id
                    && frame.src_seq == request.src_seq
            })
            && let Some((callback, _)) = effects
                .iter()
                .find(|(_, ordinal)| *ordinal == issued.ordinal)
        {
            authorized.push(*callback);
        }
        let replayed_ordinal = push(&mut frames, recorded.clone());
        loop {
            match adapter.receive()? {
                Response::Effect {
                    step,
                    callback,
                    effect,
                } => {
                    if step != recorded.ordinal || !authorized.contains(&callback) {
                        bail!(
                            "adapter attributed an effect to step {step}, callback {callback}, while processing recorded ordinal {} (authorized callbacks {authorized:?})",
                            recorded.ordinal
                        );
                    }
                    let frame = frame_from_wire(0, &effect)?;
                    if frame.src_actor != COMPONENT_ACTOR_ID || frame.kind() != EnvelopeKind::Emit {
                        bail!(
                            "adapter reported an effect that is not an Emit from the component ({:?} from actor {})",
                            frame.header.event_type,
                            frame.src_actor
                        );
                    }
                    if frame.header.schema != rook_native::EVENT_SCHEMA
                        || frame.header.flags & !rook_native::FLAGS_DEFINED != 0
                    {
                        bail!(
                            "adapter effect has schema {} and flags {:#x}; the stream allows schema {} and flags within {:#x}",
                            frame.header.schema,
                            frame.header.flags,
                            rook_native::EVENT_SCHEMA,
                            rook_native::FLAGS_DEFINED
                        );
                    }
                    let expected_seq = effect_sequences.entry(frame.channel_id).or_insert(0);
                    if frame.src_seq != *expected_seq {
                        bail!(
                            "adapter effect on channel {} has sequence {}, expected {}",
                            frame.channel_id,
                            frame.src_seq,
                            expected_seq
                        );
                    }
                    *expected_seq += 1;
                    let ordinal = push(&mut frames, frame);
                    effects.push((callback, ordinal));
                }
                Response::Progress {
                    step,
                    callback,
                    name,
                } => {
                    if step != recorded.ordinal || !authorized.contains(&callback) {
                        bail!(
                            "adapter attributed progress {name} to step {step}, callback {callback}, while processing recorded ordinal {} (authorized callbacks {authorized:?})",
                            recorded.ordinal
                        );
                    }
                    progress.push((callback, name, frames.len() as u64 - 1));
                }
                Response::Stepped {
                    ordinal,
                    status,
                    reason,
                    pending,
                } => {
                    if ordinal != recorded.ordinal {
                        bail!(
                            "adapter answered ordinal {ordinal} for step {}",
                            recorded.ordinal
                        );
                    }
                    pending_after_last_step = pending;
                    if status == StepStatus::Refused {
                        // A refused input was never applied, so it is not
                        // evidence in the replayed stream. Nothing can have
                        // been appended after it: effects only follow a
                        // consumed input.
                        if frames.len() as u64 != replayed_ordinal + 1 {
                            bail!("adapter emitted effects for an input it refused");
                        }
                        frames.pop();
                        steps.push(Step {
                            recorded_ordinal: recorded.ordinal,
                            replayed_ordinal: None,
                            status,
                            reason: reason.clone(),
                        });
                        stop = Stop::Refused {
                            ordinal: recorded.ordinal,
                            reason: reason.unwrap_or_default(),
                        };
                        break 'deliver;
                    }
                    // Only an input that stays in the stream advances the
                    // session counter the End marker continues.
                    if recorded.src_actor == SESSION_ACTOR_ID
                        && recorded.channel_id == SESSION_CHANNEL_ID
                    {
                        session_sequence = recorded.src_seq + 1;
                    }
                    steps.push(Step {
                        recorded_ordinal: recorded.ordinal,
                        replayed_ordinal: Some(replayed_ordinal),
                        status,
                        reason,
                    });
                    break;
                }
                other => bail!("unexpected adapter response during step: {other:?}"),
            }
        }
    }

    let finish_reason = if stop == Stop::Exhausted {
        FinishReason::Exhausted
    } else {
        FinishReason::Scope
    };
    adapter.send(&Request::Finish {
        reason: finish_reason,
    })?;
    let pending_at_finish = match adapter.receive()? {
        Response::Finished {
            reason,
            pending,
            effects: reported,
        } => {
            if reason != finish_reason {
                bail!("adapter finished with reason {reason:?}, the runner sent {finish_reason:?}");
            }
            if reported != effects.len() as u64 {
                bail!(
                    "adapter reports {reported} effects, the runner collected {}",
                    effects.len()
                );
            }
            pending
        }
        other => bail!("unexpected adapter response to finish: {other:?}"),
    };
    adapter.finish()?;

    let frame_count = frames.len() as u64;
    frames.push(Frame {
        ordinal: frame_count,
        src_actor: SESSION_ACTOR_ID,
        dst_actor: SESSION_ACTOR_ID,
        channel_id: SESSION_CHANNEL_ID,
        src_seq: session_sequence,
        observed_ns: 0,
        header: EventHeader {
            event_type: EventType::End,
            schema: 1,
            flags: 0,
        },
        body: End {
            uuid: session.uuid,
            frame_count,
        }
        .encode(),
    });

    Ok(Run {
        frames,
        steps,
        effects,
        progress,
        pending_after_last_step,
        pending_at_finish,
        stop,
        finish_reason,
    })
}

/// Comparison policy raw-bytes-in-order@1: effect frames compared by
/// destination, channel, event type, flags and body, in order and with full
/// multiplicity. Ordinals are compared through the run hash, which also
/// commits the interleaving with inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comparison {
    pub recorded_effects: usize,
    pub replayed_effects: usize,
    /// Index into the effect sequence of the first difference, if any.
    pub first_difference: Option<usize>,
    pub recorded_hashes: NativeHashes,
    pub replayed_hashes: NativeHashes,
}

impl Comparison {
    pub fn effects_agree(&self) -> bool {
        self.first_difference.is_none()
    }

    pub fn run_agrees(&self) -> bool {
        self.recorded_hashes.run_hash == self.replayed_hashes.run_hash
    }
}

fn effect_key(frame: &Frame) -> (u32, u32, EventType, u32, &[u8]) {
    (
        frame.dst_actor,
        frame.channel_id,
        frame.header.event_type,
        frame.header.flags,
        &frame.body,
    )
}

pub fn compare(corpus: &str, recording: &[Frame], run: &Run) -> Comparison {
    let recorded: Vec<&Frame> = recording
        .iter()
        .filter(|frame| frame.kind() == EnvelopeKind::Emit)
        .collect();
    let replayed: Vec<&Frame> = run
        .frames
        .iter()
        .filter(|frame| frame.kind() == EnvelopeKind::Emit)
        .collect();
    let shared = recorded.len().min(replayed.len());
    let mut first_difference =
        (0..shared).find(|index| effect_key(recorded[*index]) != effect_key(replayed[*index]));
    if first_difference.is_none() && recorded.len() != replayed.len() {
        first_difference = Some(shared);
    }
    Comparison {
        recorded_effects: recorded.len(),
        replayed_effects: replayed.len(),
        first_difference,
        recorded_hashes: hash_frames(corpus.as_bytes(), recording),
        replayed_hashes: hash_frames(corpus.as_bytes(), &run.frames),
    }
}
