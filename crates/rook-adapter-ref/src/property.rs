//! The two reference properties and the property input they read. A
//! property is a program: fixed input schema, fixed result schema, exit 0
//! pass, 1 fail, 2 invalid input, 3 inconclusive. It sees only what the
//! run observed; it never invents a response the recording lacks.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use rook_native::{EnvelopeKind, EventType, FLAG_ATTEMPTED, Frame};
use serde::{Deserialize, Serialize};

use crate::bodies::{CancelSend, Clock, GoalResponse, GoalSend};
use crate::driver::{Run, Stop};
use crate::protocol::{Pending, StepStatus};
use crate::{decode_hex, encode_hex, event_type_name, parse_event_type};

pub const INPUT_SCHEMA: &str = "rook-property-input@1";
pub const RESULT_SCHEMA: &str = "rook-property-result@1";
/// Goal submitted, deadline seen to expire without a response, then a
/// cancel request for that goal. Completes at the request.
pub const PROPERTY_TIMEOUT_CANCEL: &str = "goal-response-timeout-cancel@1";
/// Goal submitted, acknowledged before the deadline, and no cancel request
/// during the observation interval.
pub const PROPERTY_TIMELY_ACK: &str = "timely-ack-no-cancel@1";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PropertyEvent {
    /// Ordinal in the replayed stream.
    pub ordinal: u64,
    /// "recorded" for inputs the runner delivered, "replayed" for effects
    /// this run produced.
    pub origin: String,
    /// For recorded inputs, the recording's ordinal, which is what every
    /// `callback` and body ordinal refers to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_ordinal: Option<u64>,
    /// For recorded inputs, whether the adapter consumed it. An input the
    /// component did not consume is not evidence of anything it observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed: Option<bool>,
    pub kind: String,
    pub event_type: String,
    pub src_actor: u32,
    pub dst_actor: u32,
    pub channel_id: u32,
    pub src_seq: u64,
    pub flags: u32,
    pub body_hex: String,
    /// For effects: the recorded ordinal of the input whose callback
    /// produced them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Scope {
    pub completion: String,
    pub observation_end: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GapMark {
    pub ordinal: u64,
    pub reason: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Refusal {
    pub ordinal: u64,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Evidence {
    /// The recording's end marker was reached and every input delivered.
    pub end_of_recording: bool,
    pub gaps: Vec<GapMark>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Refusal>,
    pub pending_at_finish: Vec<Pending>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PropertyInput {
    pub schema: String,
    pub property: String,
    pub scenario: String,
    pub scope: Scope,
    pub goal_response_deadline_ns: i64,
    pub events: Vec<PropertyEvent>,
    pub progress: Vec<(u64, String)>,
    pub evidence: Evidence,
    pub identities: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Fail,
    Inconclusive,
    /// The input could not be evaluated at all.
    Invalid,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PropertyResult {
    pub schema: String,
    pub property: String,
    pub result: Verdict,
    /// The predicate that decided it, for fail and inconclusive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
    /// The event ordinal the verdict points at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordinal: Option<u64>,
    /// Where the bounded property completed, for pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_ordinal: Option<u64>,
    /// Stronger claims this result does not establish.
    pub unavailable: Vec<String>,
    /// The next missing input, for inconclusive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing: Option<String>,
    pub detail: String,
}

impl PropertyResult {
    pub fn exit_code(&self) -> i32 {
        match self.result {
            Verdict::Pass => 0,
            Verdict::Fail => 1,
            Verdict::Invalid => 2,
            Verdict::Inconclusive => 3,
        }
    }

    /// The structured line for an input that could not be evaluated.
    pub fn invalid(property: &str, detail: String) -> Self {
        result(property, Verdict::Invalid, detail)
    }
}

/// Builds the property input from a driver run.
pub fn property_input(
    property: &str,
    scenario: &str,
    scope: Scope,
    deadline_ns: i64,
    run: &Run,
    identities: BTreeMap<String, String>,
) -> PropertyInput {
    let effect_callbacks: BTreeMap<u64, u64> = run
        .effects
        .iter()
        .map(|(callback, ordinal)| (*ordinal, *callback))
        .collect();
    let steps: BTreeMap<u64, (u64, StepStatus)> = run
        .steps
        .iter()
        .filter_map(|step| {
            step.replayed_ordinal
                .map(|replayed| (replayed, (step.recorded_ordinal, step.status)))
        })
        .collect();
    let events = run
        .frames
        .iter()
        .map(|frame: &Frame| {
            let step = steps.get(&frame.ordinal);
            PropertyEvent {
                ordinal: frame.ordinal,
                origin: if frame.kind() == EnvelopeKind::Emit {
                    "replayed".to_string()
                } else {
                    "recorded".to_string()
                },
                recorded_ordinal: step.map(|(recorded, _)| *recorded),
                consumed: step.map(|(_, status)| *status == StepStatus::Ok),
                kind: format!("{:?}", frame.kind()),
                event_type: event_type_name(frame.header.event_type),
                src_actor: frame.src_actor,
                dst_actor: frame.dst_actor,
                channel_id: frame.channel_id,
                src_seq: frame.src_seq,
                flags: frame.header.flags,
                body_hex: encode_hex(&frame.body),
                callback: effect_callbacks.get(&frame.ordinal).copied(),
            }
        })
        .collect();
    let (gaps, refusal) = match &run.stop {
        Stop::Exhausted => (vec![], None),
        Stop::Gap { ordinal, gap } => (
            vec![GapMark {
                ordinal: *ordinal,
                reason: gap.reason,
                lost: gap.lost,
            }],
            None,
        ),
        Stop::Refused { ordinal, reason } => (
            vec![],
            Some(Refusal {
                ordinal: *ordinal,
                reason: reason.clone(),
            }),
        ),
    };
    PropertyInput {
        schema: INPUT_SCHEMA.to_string(),
        property: property.to_string(),
        scenario: scenario.to_string(),
        scope,
        goal_response_deadline_ns: deadline_ns,
        events,
        progress: run
            .progress
            .iter()
            .map(|(_, name, ordinal)| (*ordinal, name.clone()))
            .collect(),
        evidence: Evidence {
            end_of_recording: run.stop == Stop::Exhausted,
            gaps,
            refusal,
            pending_at_finish: run.pending_at_finish.clone(),
        },
        identities,
    }
}

struct Decoded {
    ordinal: u64,
    recorded_ordinal: Option<u64>,
    consumed: bool,
    callback: Option<u64>,
    event_type: EventType,
    replayed: bool,
    dst_actor: u32,
    flags: u32,
    body: Vec<u8>,
}

fn decode_events(input: &PropertyInput) -> Result<Vec<Decoded>> {
    input
        .events
        .iter()
        .map(|event| {
            Ok(Decoded {
                ordinal: event.ordinal,
                recorded_ordinal: event.recorded_ordinal,
                consumed: event.consumed.unwrap_or(false),
                callback: event.callback,
                event_type: parse_event_type(&event.event_type)
                    .with_context(|| format!("unknown event type {}", event.event_type))?,
                replayed: match event.origin.as_str() {
                    "recorded" => false,
                    "replayed" => true,
                    other => anyhow::bail!("unknown event origin {other}"),
                },
                dst_actor: event.dst_actor,
                flags: event.flags,
                body: decode_hex(&event.body_hex)?,
            })
        })
        .collect()
}

fn result(property: &str, verdict: Verdict, detail: String) -> PropertyResult {
    PropertyResult {
        schema: RESULT_SCHEMA.to_string(),
        property: property.to_string(),
        result: verdict,
        predicate: None,
        ordinal: None,
        completion_ordinal: None,
        unavailable: vec![],
        missing: None,
        detail,
    }
}

/// Evidence stops (a gap, a refusal, or a run that ended early) make a
/// missing later event inconclusive rather than a failure.
fn evidence_stopped(input: &PropertyInput) -> Option<String> {
    if let Some(gap) = input.evidence.gaps.first() {
        return Some(format!(
            "recording has a gap at recorded ordinal {} (reason {}, lost {})",
            gap.ordinal,
            gap.reason,
            gap.lost.map_or("unknown".to_string(), |n| n.to_string())
        ));
    }
    if let Some(refusal) = &input.evidence.refusal {
        return Some(format!(
            "adapter refused recorded ordinal {}: {}",
            refusal.ordinal, refusal.reason
        ));
    }
    if !input.evidence.end_of_recording {
        return Some("run stopped before the end of the recording".to_string());
    }
    None
}

fn pending_for(input: &PropertyInput, callback: u64) -> Option<String> {
    input
        .evidence
        .pending_at_finish
        .iter()
        .find_map(|pending| match pending {
            Pending::ClockRead {
                callback: c,
                clock_id,
            } if *c == callback => Some(format!(
                "clock observation (clock {clock_id}) for callback {callback}"
            )),
            Pending::Future { callback: c, name } if *c == callback => {
                Some(format!("{name} for callback {callback}"))
            }
            _ => None,
        })
}

pub fn evaluate(input: &PropertyInput) -> Result<PropertyResult> {
    if input.schema != INPUT_SCHEMA {
        anyhow::bail!("input schema {} is not {INPUT_SCHEMA}", input.schema);
    }
    let events = decode_events(input)?;
    // Each property has one scope it can evaluate; any other declared
    // scope is a different case, refused rather than evaluated under the
    // program's own.
    type Evaluator = fn(&PropertyInput, &[Decoded]) -> Result<PropertyResult>;
    let (completion, evaluator): (&str, Evaluator) = match input.property.as_str() {
        PROPERTY_TIMEOUT_CANCEL => ("cancel_request_issued", timeout_cancel),
        PROPERTY_TIMELY_ACK => ("observation_interval_elapsed", timely_ack),
        other => anyhow::bail!("unknown property {other}"),
    };
    if input.scope.completion != completion {
        anyhow::bail!(
            "scope.completion {} is not this property's completion rule {completion}",
            input.scope.completion
        );
    }
    if input.scope.observation_end != "end_of_recording" {
        anyhow::bail!(
            "scope.observation_end {} is not supported; this program evaluates to end_of_recording",
            input.scope.observation_end
        );
    }
    evaluator(input, &events)
}

/// The goal the first consumed command produced: its callback (the
/// command's recorded ordinal), the send, its id and the clock the
/// callback consumed to send it.
struct Submission {
    callback: u64,
    ordinal: u64,
    goal_id: [u8; 16],
    /// The endpoint the goal went to; a cancel must go to the same one.
    server: u32,
    sent_at_ns: i64,
    deadline_ns: i64,
}

/// Clock observations this callback consumed, in order.
fn clocks_of(events: &[Decoded], callback: u64) -> impl Iterator<Item = (&Decoded, Clock)> {
    events
        .iter()
        .filter(move |event| {
            event.event_type == EventType::Clock && !event.replayed && event.consumed
        })
        .filter_map(move |event| {
            let clock = Clock::decode(&event.body).ok()?;
            (clock.callback_ordinal == callback).then_some((event, clock))
        })
}

fn submission(
    input: &PropertyInput,
    events: &[Decoded],
) -> Result<Result<Submission, PropertyResult>> {
    let command = events
        .iter()
        .find(|event| event.event_type == EventType::Message && !event.replayed && event.consumed);
    let Some(command) = command else {
        let mut r = result(
            &input.property,
            Verdict::Inconclusive,
            "no goal command was delivered and consumed".to_string(),
        );
        r.predicate = Some("goal_command_delivered".to_string());
        r.missing = Some("goal command message".to_string());
        return Ok(Err(r));
    };
    let callback = command
        .recorded_ordinal
        .context("a recorded input carries its recorded ordinal")?;
    // The send this command produced: an effect of the command's callback,
    // after the command. A later command's send never counts for this one.
    let send = events.iter().find(|event| {
        event.event_type == EventType::GoalSend
            && event.replayed
            && event.ordinal > command.ordinal
            && event.callback == Some(callback)
            && event.flags & FLAG_ATTEMPTED != 0
    });
    let Some(send) = send else {
        // A callback still waiting on a recorded input at the end of the
        // run was never given the chance to submit, whether the recording
        // stopped at a gap or simply closed.
        if let Some(pending) = pending_for(input, callback) {
            let mut r = result(
                &input.property,
                Verdict::Inconclusive,
                format!(
                    "the goal command's callback was still waiting when the evidence ran out: {}",
                    evidence_stopped(input).unwrap_or_else(|| "the recording closed".to_string())
                ),
            );
            r.predicate = Some("goal_submitted".to_string());
            r.ordinal = Some(command.ordinal);
            r.missing = Some(pending);
            return Ok(Err(r));
        }
        let mut r = result(
            &input.property,
            Verdict::Fail,
            "the component never submitted a goal for the goal command".to_string(),
        );
        r.predicate = Some("goal_submitted".to_string());
        r.ordinal = Some(command.ordinal);
        return Ok(Err(r));
    };
    let goal_id = GoalSend::decode(&send.body)?.goal_id;
    let (_, sent_at) = clocks_of(events, callback)
        .filter(|(event, _)| event.ordinal < send.ordinal)
        .last()
        .context("the goal was sent without a clock observation its callback consumed")?;
    let deadline_ns = sent_at
        .value_ns
        .checked_add(input.goal_response_deadline_ns)
        .context("send clock plus deadline overflows i64")?;
    Ok(Ok(Submission {
        callback,
        ordinal: send.ordinal,
        goal_id,
        server: send.dst_actor,
        sent_at_ns: sent_at.value_ns,
        deadline_ns,
    }))
}

fn cancels_for(events: &[Decoded], submission: &Submission) -> Vec<(u64, CancelSend)> {
    events
        .iter()
        .filter(|event| {
            event.event_type == EventType::CancelSend
                && event.replayed
                && event.ordinal > submission.ordinal
                && event.dst_actor == submission.server
                && event.flags & FLAG_ATTEMPTED != 0
        })
        .filter_map(|event| {
            CancelSend::decode(&event.body)
                .ok()
                .map(|c| (event.ordinal, c))
        })
        .filter(|(_, cancel)| cancel.goal_id == submission.goal_id || cancel.goal_id == [0; 16])
        .collect()
}

fn acknowledgment<'a>(events: &'a [Decoded], submission: &Submission) -> Option<&'a Decoded> {
    events.iter().find(|event| {
        event.event_type == EventType::GoalResponse
            && !event.replayed
            && event.consumed
            && event.ordinal > submission.ordinal
            && GoalResponse::decode(&event.body).is_ok_and(|r| r.goal_id == submission.goal_id)
    })
}

fn timeout_cancel(input: &PropertyInput, events: &[Decoded]) -> Result<PropertyResult> {
    let property = &input.property;
    let submission = match submission(input, events)? {
        Ok(submission) => submission,
        Err(verdict) => return Ok(verdict),
    };
    let deadline_ns = submission.deadline_ns;
    let acknowledged = acknowledgment(events, &submission);

    // Deadline expiry: a clock observation the goal callback consumed, at
    // or past the deadline, after the send and before any acknowledgment.
    let expiry = clocks_of(events, submission.callback)
        .filter(|(event, _)| event.ordinal > submission.ordinal)
        .filter(|(event, _)| acknowledged.is_none_or(|ack| event.ordinal < ack.ordinal))
        .find(|(_, clock)| clock.value_ns >= deadline_ns)
        .map(|(event, clock)| (event.ordinal, clock.value_ns));

    let cancels = cancels_for(events, &submission);

    let Some((expiry_ordinal, expiry_ns)) = expiry else {
        if let Some((ordinal, _)) = cancels.first() {
            let mut r = result(
                property,
                Verdict::Fail,
                format!(
                    "a cancel request for the goal was issued at ordinal {ordinal} before any observed deadline expiry"
                ),
            );
            r.predicate = Some("cancel_after_deadline_expiry".to_string());
            r.ordinal = Some(*ordinal);
            return Ok(r);
        }
        let mut r = result(
            property,
            Verdict::Inconclusive,
            match acknowledged {
                Some(ack) => format!(
                    "the goal was acknowledged at ordinal {} before the deadline; this scenario does not exercise the timeout",
                    ack.ordinal
                ),
                None => format!(
                    "no clock observation at or past the deadline ({} + {} ns) was consumed by the goal callback before the run ended",
                    submission.sent_at_ns, input.goal_response_deadline_ns
                ),
            },
        );
        r.predicate = Some("deadline_expired".to_string());
        r.missing = Some(match evidence_stopped(input) {
            Some(stop) => format!(
                "clock observation with value >= {deadline_ns} ns for callback {}; {stop}",
                submission.callback
            ),
            None => format!(
                "clock observation with value >= {deadline_ns} ns for callback {}",
                submission.callback
            ),
        });
        return Ok(r);
    };

    if let Some((ordinal, _)) = cancels
        .iter()
        .find(|(ordinal, _)| *ordinal < expiry_ordinal)
    {
        let mut r = result(
            property,
            Verdict::Fail,
            format!(
                "a cancel request was issued at ordinal {ordinal}, before the deadline expired at ordinal {expiry_ordinal}"
            ),
        );
        r.predicate = Some("cancel_after_deadline_expiry".to_string());
        r.ordinal = Some(*ordinal);
        return Ok(r);
    }
    match cancels
        .iter()
        .find(|(ordinal, _)| *ordinal > expiry_ordinal)
    {
        Some((ordinal, _)) => {
            let mut r = result(
                property,
                Verdict::Pass,
                format!(
                    "goal submitted at ordinal {}, deadline expired at ordinal {expiry_ordinal} (clock {expiry_ns} ns >= {deadline_ns} ns), cancel request issued at ordinal {ordinal}; the property ends at the request",
                    submission.ordinal
                ),
            );
            r.completion_ordinal = Some(*ordinal);
            r.unavailable = vec![
                "cancel_acknowledged".to_string(),
                "goal_terminated".to_string(),
            ];
            Ok(r)
        }
        None => {
            if let Some(stop) = evidence_stopped(input) {
                let mut r = result(
                    property,
                    Verdict::Inconclusive,
                    format!(
                        "the deadline expired at ordinal {expiry_ordinal} but the run stopped before a cancel request could be observed: {stop}"
                    ),
                );
                r.predicate = Some("cancel_request_issued".to_string());
                r.missing = Some("recorded inputs after the deadline expiry".to_string());
                return Ok(r);
            }
            let mut r = result(
                property,
                Verdict::Fail,
                format!(
                    "the deadline expired at ordinal {expiry_ordinal} (clock {expiry_ns} ns >= {deadline_ns} ns) and no cancel request for the goal followed before the end of the recording"
                ),
            );
            r.predicate = Some("cancel_request_issued".to_string());
            r.ordinal = Some(expiry_ordinal);
            Ok(r)
        }
    }
}

fn timely_ack(input: &PropertyInput, events: &[Decoded]) -> Result<PropertyResult> {
    let property = &input.property;
    let submission = match submission(input, events)? {
        Ok(submission) => submission,
        Err(verdict) => return Ok(verdict),
    };
    let deadline_ns = submission.deadline_ns;

    if let Some((ordinal, _)) = cancels_for(events, &submission).first() {
        let mut r = result(
            property,
            Verdict::Fail,
            format!(
                "a cancel request for the goal was issued at ordinal {ordinal} inside the observation interval"
            ),
        );
        r.predicate = Some("no_cancel_in_interval".to_string());
        r.ordinal = Some(*ordinal);
        return Ok(r);
    }

    let accepted = acknowledgment(events, &submission)
        .filter(|event| GoalResponse::decode(&event.body).is_ok_and(|r| r.accepted));
    let Some(ack) = accepted else {
        let mut r = result(
            property,
            Verdict::Inconclusive,
            "no acceptance for the goal was consumed".to_string(),
        );
        r.predicate = Some("acknowledged_before_deadline".to_string());
        r.missing = Some(match evidence_stopped(input) {
            Some(stop) => format!("goal_response for the submitted goal; {stop}"),
            None => "goal_response for the submitted goal".to_string(),
        });
        return Ok(r);
    };
    let late = clocks_of(events, submission.callback).any(|(event, clock)| {
        event.ordinal > submission.ordinal
            && event.ordinal < ack.ordinal
            && clock.value_ns >= deadline_ns
    });
    if late {
        let mut r = result(
            property,
            Verdict::Inconclusive,
            "the deadline expired before the acknowledgment; this scenario does not exercise timely acknowledgment".to_string(),
        );
        r.predicate = Some("acknowledged_before_deadline".to_string());
        return Ok(r);
    }
    if let Some(stop) = evidence_stopped(input) {
        let mut r = result(
            property,
            Verdict::Inconclusive,
            format!("the observation interval did not reach the end of the recording: {stop}"),
        );
        r.predicate = Some("observation_interval_elapsed".to_string());
        r.missing = Some("recorded inputs to the end of the recording".to_string());
        return Ok(r);
    }
    let mut r = result(
        property,
        Verdict::Pass,
        format!(
            "goal submitted at ordinal {}, accepted at ordinal {} before the deadline, and no cancel request followed to the end of the recording",
            submission.ordinal, ack.ordinal
        ),
    );
    r.completion_ordinal = input.events.last().map(|event| event.ordinal);
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bodies;
    use crate::component::Component;

    fn event(
        ordinal: u64,
        origin: &str,
        event_type: EventType,
        body: Vec<u8>,
        recorded_ordinal: Option<u64>,
        callback: Option<u64>,
    ) -> PropertyEvent {
        PropertyEvent {
            ordinal,
            origin: origin.to_string(),
            recorded_ordinal,
            consumed: recorded_ordinal.map(|_| true),
            kind: String::new(),
            event_type: event_type_name(event_type),
            src_actor: 0,
            dst_actor: 3,
            channel_id: 0,
            src_seq: 0,
            flags: if origin == "replayed" {
                FLAG_ATTEMPTED
            } else {
                0
            },
            body_hex: encode_hex(&body),
            callback,
        }
    }

    fn input(events: Vec<PropertyEvent>) -> PropertyInput {
        PropertyInput {
            schema: INPUT_SCHEMA.to_string(),
            property: PROPERTY_TIMELY_ACK.to_string(),
            scenario: "unit".to_string(),
            scope: Scope {
                completion: "observation_interval_elapsed".to_string(),
                observation_end: "end_of_recording".to_string(),
            },
            goal_response_deadline_ns: 1_000_000_000,
            events,
            progress: vec![],
            evidence: Evidence {
                end_of_recording: true,
                gaps: vec![],
                refusal: None,
                pending_at_finish: vec![],
            },
            identities: BTreeMap::new(),
        }
    }

    /// The first command never submitted; a later command did and was
    /// acknowledged. The later goal must not pass the first command.
    #[test]
    fn a_later_goal_never_answers_for_an_earlier_command() {
        let goal_b = Component::goal_id(0);
        let clock = |value_ns: i64, callback: u64| {
            Clock {
                clock_id: 1,
                value_ns,
                callback_ordinal: callback,
            }
            .encode()
        };
        let events = vec![
            event(
                1,
                "recorded",
                EventType::Message,
                bodies::message(b"a"),
                Some(2),
                None,
            ),
            event(
                2,
                "recorded",
                EventType::Message,
                bodies::message(b"b"),
                Some(5),
                None,
            ),
            event(3, "recorded", EventType::Clock, clock(0, 5), Some(6), None),
            event(
                4,
                "replayed",
                EventType::GoalSend,
                GoalSend {
                    goal_id: goal_b,
                    goal: b"b".to_vec(),
                }
                .encode(),
                None,
                Some(5),
            ),
            event(
                5,
                "recorded",
                EventType::GoalResponse,
                GoalResponse {
                    goal_send_ordinal: 7,
                    goal_id: goal_b,
                    accepted: true,
                    stamp_ns: 1,
                }
                .encode(),
                Some(8),
                None,
            ),
        ];
        let result = evaluate(&input(events)).unwrap();
        assert_eq!(result.result, Verdict::Fail, "{}", result.detail);
        assert_eq!(result.predicate.as_deref(), Some("goal_submitted"));
        assert_eq!(result.ordinal, Some(1));
    }

    /// A cancel that did not go to the goal's server, or was never
    /// attempted, is not a cancel request for that goal.
    #[test]
    fn a_cancel_counts_only_when_attempted_to_the_goals_server() {
        let goal = Component::goal_id(0);
        let clock = |value_ns: i64, callback: u64| {
            Clock {
                clock_id: 1,
                value_ns,
                callback_ordinal: callback,
            }
            .encode()
        };
        let base = vec![
            event(
                1,
                "recorded",
                EventType::Message,
                bodies::message(b"a"),
                Some(2),
                None,
            ),
            event(2, "recorded", EventType::Clock, clock(0, 2), Some(3), None),
            event(
                3,
                "replayed",
                EventType::GoalSend,
                GoalSend {
                    goal_id: goal,
                    goal: b"a".to_vec(),
                }
                .encode(),
                None,
                Some(2),
            ),
            event(
                4,
                "recorded",
                EventType::Clock,
                clock(5_000_000_000, 2),
                Some(5),
                None,
            ),
            event(
                5,
                "replayed",
                EventType::CancelSend,
                CancelSend {
                    goal_id: goal,
                    stamp_ns: 5_000_000_000,
                }
                .encode(),
                None,
                Some(2),
            ),
        ];
        let run = |events: Vec<PropertyEvent>| {
            let mut input = input(events);
            input.property = PROPERTY_TIMEOUT_CANCEL.to_string();
            input.scope.completion = "cancel_request_issued".to_string();
            evaluate(&input).unwrap()
        };
        assert_eq!(run(base.clone()).result, Verdict::Pass);
        let mut wrong_server = base.clone();
        wrong_server[4].dst_actor = 999;
        let result = run(wrong_server);
        assert_eq!(result.result, Verdict::Fail, "{}", result.detail);
        assert_eq!(result.predicate.as_deref(), Some("cancel_request_issued"));
        let mut not_attempted = base;
        not_attempted[4].flags = 0;
        let result = run(not_attempted);
        assert_eq!(result.result, Verdict::Fail, "{}", result.detail);
        assert_eq!(result.predicate.as_deref(), Some("cancel_request_issued"));
    }

    /// A clock observation for another callback is not this goal's
    /// deadline evidence, and an unconsumed one is not evidence at all.
    #[test]
    fn deadline_evidence_must_be_consumed_by_the_goal_callback() {
        let goal = Component::goal_id(0);
        let clock = |value_ns: i64, callback: u64| {
            Clock {
                clock_id: 1,
                value_ns,
                callback_ordinal: callback,
            }
            .encode()
        };
        let mut events = vec![
            event(
                1,
                "recorded",
                EventType::Message,
                bodies::message(b"a"),
                Some(2),
                None,
            ),
            event(2, "recorded", EventType::Clock, clock(0, 2), Some(3), None),
            event(
                3,
                "replayed",
                EventType::GoalSend,
                GoalSend {
                    goal_id: goal,
                    goal: b"a".to_vec(),
                }
                .encode(),
                None,
                Some(2),
            ),
            event(
                4,
                "recorded",
                EventType::Clock,
                clock(5_000_000_000, 999),
                Some(5),
                None,
            ),
            event(
                5,
                "replayed",
                EventType::CancelSend,
                CancelSend {
                    goal_id: goal,
                    stamp_ns: 5_000_000_000,
                }
                .encode(),
                None,
                Some(2),
            ),
        ];
        let mut input = input(events.clone());
        input.property = PROPERTY_TIMEOUT_CANCEL.to_string();
        input.scope.completion = "cancel_request_issued".to_string();
        let result = evaluate(&input).unwrap();
        assert_eq!(result.result, Verdict::Fail, "{}", result.detail);
        assert_eq!(
            result.predicate.as_deref(),
            Some("cancel_after_deadline_expiry")
        );

        // Same callback but not consumed: still no expiry evidence.
        events[3] = event(
            4,
            "recorded",
            EventType::Clock,
            clock(5_000_000_000, 2),
            Some(5),
            None,
        );
        events[3].consumed = Some(false);
        let mut input = self::input(events);
        input.property = PROPERTY_TIMEOUT_CANCEL.to_string();
        input.scope.completion = "cancel_request_issued".to_string();
        let result = evaluate(&input).unwrap();
        assert_eq!(result.result, Verdict::Fail, "{}", result.detail);
        assert_eq!(
            result.predicate.as_deref(),
            Some("cancel_after_deadline_expiry")
        );
    }
}
