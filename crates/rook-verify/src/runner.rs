//! Drives the public adapter protocol and evaluates a fixed property at emitted
//! effects, before waiting for callback completion or another environmental input.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail};
use rook_native::{
    COMPONENT_ACTOR_ID, End, EnvelopeKind, EventHeader, EventType, Frame, Gap, SESSION_ACTOR_ID,
    SESSION_CHANNEL_ID, Session,
};

use rook_adapter_ref::protocol::{
    FinishReason, Request, RequestRef, Response, StartingState, StepStatus, WireFrame,
};
use rook_adapter_ref::{decode_hex, encode_hex, event_type_name, parse_event_type};

use crate::case::{Case, Identity, entry};
use crate::normalization::Goals;
use anyhow::ensure;
use rook_adapter_ref::driver::{Run, Step, Stop};
use rook_adapter_ref::property::{self, PropertyResult, Verdict};

pub struct Execution {
    pub run: Run,
    pub normalized: Vec<Frame>,
    pub property: PropertyResult,
    pub scope_completed: bool,
}

pub struct PropertyProgram {
    pub executable: std::path::PathBuf,
    pub arguments: Vec<String>,
}

impl PropertyProgram {
    pub fn evaluate(&self, case: &Case, run: &Run, identity: &Identity) -> Result<PropertyResult> {
        let mut input = property::property_input(
            entry(&case.declaration, "property")?,
            entry(&case.declaration, "scenario")?,
            property::Scope {
                completion: entry(&case.declaration, "scope.completion")?.into(),
                observation_end: entry(&case.declaration, "scope.observation_end")?.into(),
            },
            entry(&case.declaration, "goal_response_deadline_ns")?.parse()?,
            run,
            identity.clone(),
        );
        if run.finish_reason == FinishReason::Scope {
            input.evidence.end_of_recording = false;
        }
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rook-property-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        struct Temporary(std::path::PathBuf);
        impl Drop for Temporary {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let _temporary = Temporary(path.clone());
        serde_json::to_writer(&mut file, &input)?;
        file.flush()?;
        let mut command = Command::new(&self.executable);
        command.args(&self.arguments).arg(path);
        let mut process = AdapterProcess::spawn(&mut command)?;
        process.stdin.take();
        let line = process.line()?;
        let result: PropertyResult = serde_json::from_str(&line).context("property result JSON")?;
        ensure!(
            result.schema == property::RESULT_SCHEMA && result.property == input.property,
            "property result schema or identity differs"
        );
        ensure!(
            process.finish()? == result.exit_code(),
            "property result and process exit disagree"
        );
        if result.result == Verdict::Pass {
            let ordinal = result
                .completion_ordinal
                .context("property passed without completion evidence")?;
            ensure!(
                run.frames.iter().any(|f| f.ordinal == ordinal),
                "property completion ordinal is outside observed evidence"
            );
        }
        Ok(result)
    }
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
    ensure!(
        input.src_actor == request.dst_actor && input.dst_actor == request.src_actor,
        "response at ordinal {} does not come from the request's endpoint",
        input.ordinal
    );
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
    stdout: std::sync::mpsc::Receiver<Result<String>>,
}

impl AdapterProcess {
    fn spawn(command: &mut Command) -> Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .context("spawn adapter")?;
        let stdin = child.stdin.take().context("adapter stdin")?;
        let stdout = child.stdout.take().context("adapter stdout")?;
        let (send, receive) = std::sync::mpsc::sync_channel(16);
        std::thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                let result = (&mut stdout)
                    .take(1_048_577)
                    .read_line(&mut line)
                    .map_err(anyhow::Error::from)
                    .and_then(|n| {
                        ensure!(n > 0, "adapter closed stdout");
                        ensure!(
                            n <= 1_048_576 && line.ends_with('\n'),
                            "adapter line exceeds 1 MiB or is unterminated"
                        );
                        Ok(line)
                    });
                let failed = result.is_err();
                if send.send(result).is_err() || failed {
                    break;
                }
            }
        });
        Ok(AdapterProcess {
            child,
            stdin: Some(stdin),
            stdout: receive,
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

    fn line(&self) -> Result<String> {
        self.stdout
            .recv_timeout(std::time::Duration::from_secs(10))
            .context("adapter/property did not respond within 10 seconds")?
    }
    fn receive(&mut self) -> Result<Response> {
        serde_json::from_str(&self.line()?).context("parse adapter response")
    }
    fn finish(&mut self) -> Result<i32> {
        self.stdin.take();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait()? {
                return status.code().context("child terminated by signal");
            }
            ensure!(
                std::time::Instant::now() < deadline,
                "child failed to exit within 10 seconds"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
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
    case: &Case,
    identity: &Identity,
    property: &PropertyProgram,
    candidate: bool,
) -> Result<Execution> {
    let recording = &case.frames;
    let corpus = entry(&case.declaration, "corpus")?;
    let scenario = entry(&case.declaration, "scenario")?;
    let mut goals = Goals::new(
        recording,
        identity,
        entry(&case.declaration, "comparison_policy")?,
    )?;
    let mut normalized_effects = std::collections::BTreeMap::new();
    let mut terminal_property = None;
    let mut finish_callbacks = Vec::new();
    let session_frame = recording.first().context("empty recording")?;
    let session = Session::decode(&session_frame.body)?;
    let requested_state = StartingState {
        method: entry(identity, "starting_state")?.into(),
        blake3: entry(identity, "starting_state_blake3")?.into(),
    };
    let mut adapter = AdapterProcess::spawn(command)?;
    adapter.send(&Request::Start {
        protocol: rook_adapter_ref::PROTOCOL_VERSION,
        corpus: corpus.to_string(),
        scenario: scenario.to_string(),
        state: requested_state.clone(),
    })?;
    match adapter.receive()? {
        Response::Started {
            protocol,
            state,
            identity: started,
            endpoints,
            channels,
        } if protocol == rook_adapter_ref::PROTOCOL_VERSION => {
            ensure!(
                started.component == entry(identity, "component")?,
                "identity drift at component"
            );
            ensure!(
                started.variant == entry(identity, "component_variant")?,
                "identity drift at component_variant"
            );
            ensure!(
                started.adapter == entry(identity, "adapter")?,
                "identity drift at adapter"
            );
            ensure!(
                started.protocol.to_string() == entry(identity, "adapter_protocol")?,
                "identity drift at adapter_protocol"
            );
            let mut observed = Identity::new();
            for endpoint in endpoints {
                ensure!(
                    observed
                        .insert(format!("endpoint.{}", endpoint.actor), endpoint.name)
                        .is_none(),
                    "duplicate startup endpoint"
                );
            }
            for channel in channels {
                ensure!(
                    observed
                        .insert(
                            format!("channel.{}", channel.id),
                            format!(
                                "{} {} {}",
                                channel.name,
                                channel.direction,
                                channel.event_types.join(",")
                            )
                        )
                        .is_none(),
                    "duplicate startup channel"
                );
            }
            let expected = identity
                .iter()
                .filter(|(k, _)| k.starts_with("endpoint.") || k.starts_with("channel."))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            crate::case::same_identity(&expected, &observed, false)?;
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
        let mut delivered = recorded.clone();
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
                let canonical = normalized_effects.get(&issued.ordinal).unwrap_or(issued);
                canonical.dst_actor == recorded_request.dst_actor
                    && canonical.header == recorded_request.header
                    && canonical.body == recorded_request.body
            });
            let correlated = goals.input(recorded, recorded_request);
            if !equivalent || correlated.is_err() {
                let reason = if let Err(error) = &correlated {
                    format!(
                        "unsupported response dependency at ordinal {} ({error:#})",
                        recorded.ordinal
                    )
                } else {
                    match issued {
                        Some(_) => format!(
                            "recorded response at ordinal {} answers recorded request {}, but this run's request on channel {} sequence {} differs from it",
                            recorded.ordinal, request.ordinal, request.channel_id, request.src_seq
                        ),
                        None => format!(
                            "recorded response at ordinal {} answers recorded request {}, which this run never issued",
                            recorded.ordinal, request.ordinal
                        ),
                    }
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
            delivered = correlated?;
        }
        adapter.send(&Request::Step {
            input: wire_frame(&delivered),
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
                .and_then(|bytes| <[u8; 8]>::try_from(bytes).ok())
                .map(u64::from_le_bytes)
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
        let replayed_ordinal = push(&mut frames, delivered);
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
                    let frame = frame_from_wire(frames.len() as u64, &effect)?;
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
                    crate::case::validate_event(&frame, identity)?;
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
                    // An effect confirms consumption of the input that caused it.
                    // Evaluate now so a waiting callback cannot hide scope completion.
                    rook_native::decode_payload(frames.len() as u64, &frame.payload())?;
                    let mut canonical = goals.effect(&frame)?;
                    let ordinal = push(&mut frames, frame);
                    canonical.ordinal = ordinal;
                    normalized_effects.insert(ordinal, canonical);
                    effects.push((callback, ordinal));
                    if candidate {
                        let mut current_steps = steps.clone();
                        current_steps.push(Step {
                            recorded_ordinal: recorded.ordinal,
                            replayed_ordinal: Some(replayed_ordinal),
                            status: StepStatus::Ok,
                            reason: None,
                        });
                        let current = Run {
                            frames: frames.clone(),
                            steps: current_steps,
                            effects: effects.clone(),
                            progress: progress.clone(),
                            pending_after_last_step: pending_after_last_step.clone(),
                            pending_at_finish: pending_after_last_step.clone(),
                            stop: Stop::Exhausted,
                            finish_reason: FinishReason::Scope,
                        };
                        let result = property.evaluate(case, &current, identity)?;
                        if matches!(
                            result.result,
                            Verdict::Pass | Verdict::Fail | Verdict::Invalid
                        ) {
                            terminal_property = Some(result);
                            finish_callbacks = authorized.clone();
                            steps = current.steps;
                            stop = current.stop;
                            break 'deliver;
                        }
                    }
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
                    ensure!(
                        status == StepStatus::Ok || frames.len() as u64 == replayed_ordinal + 1,
                        "adapter emitted effects for an unconsumed input"
                    );
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

    let finish_reason = if stop == Stop::Exhausted && terminal_property.is_none() {
        FinishReason::Exhausted
    } else {
        FinishReason::Scope
    };
    adapter.send(&Request::Finish {
        reason: finish_reason,
    })?;
    let pending_at_finish = loop {
        match adapter.receive()? {
            Response::Finished {
                reason,
                pending,
                effects: reported,
            } => {
                if reason != finish_reason {
                    bail!(
                        "adapter finished with reason {reason:?}, the runner sent {finish_reason:?}"
                    );
                }
                if reported != effects.len() as u64 {
                    bail!(
                        "adapter reports {reported} effects, the runner collected {}",
                        effects.len()
                    );
                }
                break pending;
            }
            Response::Progress {
                step,
                callback,
                name,
            } if terminal_property.is_some() => {
                ensure!(
                    steps.last().is_some_and(|s| s.recorded_ordinal == step)
                        && finish_callbacks.contains(&callback),
                    "invalid progress after scope completion"
                );
                progress.push((callback, name, frames.len() as u64 - 1));
            }
            Response::Stepped {
                ordinal,
                status,
                pending,
                ..
            } if terminal_property.is_some() => {
                ensure!(
                    steps.last().is_some_and(|s| s.recorded_ordinal == ordinal)
                        && status == StepStatus::Ok,
                    "adapter denied consumption after emitting a completion effect"
                );
                pending_after_last_step = pending;
            }
            // Effects already buffered after the bounded completion are outside the
            // property. They remain visible and counted, without extending its claim.
            Response::Effect {
                step,
                callback,
                effect,
            } if terminal_property.is_some() => {
                ensure!(
                    steps.last().is_some_and(|s| s.recorded_ordinal == step)
                        && finish_callbacks.contains(&callback),
                    "invalid effect association after scope completion"
                );
                let frame = frame_from_wire(frames.len() as u64, &effect)?;
                ensure!(
                    frame.src_actor == COMPONENT_ACTOR_ID && frame.kind() == EnvelopeKind::Emit,
                    "invalid effect after scope completion"
                );
                rook_native::decode_payload(frame.ordinal, &frame.payload())?;
                crate::case::validate_event(&frame, identity)?;
                let expected = effect_sequences.entry(frame.channel_id).or_default();
                ensure!(
                    frame.src_seq == *expected,
                    "invalid effect sequence after scope completion"
                );
                *expected += 1;
                let canonical = goals.effect(&frame)?;
                normalized_effects.insert(frame.ordinal, canonical);
                let ordinal = push(&mut frames, frame);
                effects.push((callback, ordinal));
            }
            other => bail!("unexpected adapter response to finish: {other:?}"),
        }
    };
    ensure!(adapter.finish()? == 0, "adapter exited unsuccessfully");

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

    let run = Run {
        frames,
        steps,
        effects,
        progress,
        pending_after_last_step,
        pending_at_finish,
        stop,
        finish_reason,
    };
    let property = match terminal_property {
        Some(result) => result,
        None => property.evaluate(case, &run, identity)?,
    };
    let scope_completed = property.result == Verdict::Pass;
    let mut normalized = run.frames.clone();
    for frame in &mut normalized {
        if let Some(effect) = normalized_effects.get(&frame.ordinal) {
            *frame = effect.clone();
        } else if let Some(step) = run
            .steps
            .iter()
            .find(|s| s.replayed_ordinal == Some(frame.ordinal))
        {
            // Hash original input bodies, whose ordinals always refer to the recording.
            frame.body = recording[step.recorded_ordinal as usize].body.clone();
        }
    }
    Ok(Execution {
        run,
        normalized,
        property,
        scope_completed,
    })
}
