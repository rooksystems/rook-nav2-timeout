//! The adapter: wraps the component in the ndjson protocol. Effects and
//! progress are written the moment the component produces them, before the
//! `stepped` line, and a step never blocks on a pending future.

use std::io::{BufRead, Write};

use anyhow::{Context, Result, bail};
use rook_native::COMPONENT_ACTOR_ID;

use crate::component::{Component, Consumed, Input, Variant, Wait};
use crate::protocol::{
    Channel, Endpoint, Identity, Pending, Request, Response, StartingState, StepStatus, WireFrame,
};
use crate::{
    ACTOR_ACTION_SERVER, ACTOR_CLOCK, ACTOR_COMMAND, ACTOR_DIAGNOSTICS, ADAPTER_NAME,
    CH_ACTION_CANCEL, CH_ACTION_GOAL, CH_ACTION_RESPONSE, CH_CLOCK, CH_GOAL_COMMAND, CH_STATUS,
    PROTOCOL_VERSION, decode_hex, encode_hex, event_type_name, parse_event_type,
};

pub fn endpoints() -> Vec<Endpoint> {
    [
        (COMPONENT_ACTOR_ID, "component"),
        (ACTOR_CLOCK, "clock"),
        (ACTOR_ACTION_SERVER, "action_server"),
        (ACTOR_COMMAND, "command"),
        (ACTOR_DIAGNOSTICS, "diagnostics"),
    ]
    .into_iter()
    .map(|(actor, name)| Endpoint {
        actor,
        name: name.to_string(),
    })
    .collect()
}

pub fn channels() -> Vec<Channel> {
    [
        (CH_GOAL_COMMAND, "goal_command", "input", vec!["Message"]),
        (CH_CLOCK, "clock", "input", vec!["Clock"]),
        (
            CH_ACTION_RESPONSE,
            "action_response",
            "input",
            vec!["GoalResponse", "Result", "CancelResponse"],
        ),
        (CH_ACTION_GOAL, "action_goal", "effect", vec!["GoalSend"]),
        (
            CH_ACTION_CANCEL,
            "action_cancel",
            "effect",
            vec!["CancelSend"],
        ),
        (CH_STATUS, "status", "effect", vec!["Status"]),
    ]
    .into_iter()
    .map(|(id, name, direction, event_types)| Channel {
        id,
        name: name.to_string(),
        direction: direction.to_string(),
        event_types: event_types.into_iter().map(str::to_string).collect(),
    })
    .collect()
}

pub fn identity(variant: Variant) -> Identity {
    Identity {
        component: "goal-client".to_string(),
        variant: variant.name().to_string(),
        adapter: ADAPTER_NAME.to_string(),
        protocol: PROTOCOL_VERSION,
    }
}

fn pending(waits: &[Wait]) -> Vec<Pending> {
    waits
        .iter()
        .map(|wait| match wait {
            Wait::ClockRead { callback, clock_id } => Pending::ClockRead {
                callback: *callback,
                clock_id: *clock_id,
            },
            Wait::Future { callback, name } => Pending::Future {
                callback: *callback,
                name: name.to_string(),
            },
        })
        .collect()
}

/// Runs the protocol until `finish` or end of input.
pub fn serve(variant: Variant, reader: impl BufRead, mut writer: impl Write) -> Result<()> {
    let mut component: Option<Component> = None;
    let mut effects = 0_u64;
    let mut send = |response: &Response| -> Result<()> {
        serde_json::to_writer(&mut writer, response)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        Ok(())
    };
    for line in reader.lines() {
        let line = line.context("read request line")?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Request = serde_json::from_str(&line).context("parse request")?;
        match request {
            Request::Start {
                protocol, state, ..
            } => {
                if protocol != PROTOCOL_VERSION {
                    send(&Response::Refused {
                        reason: format!(
                            "protocol {protocol} unsupported; this adapter speaks {PROTOCOL_VERSION}"
                        ),
                    })?;
                    return Ok(());
                }
                if state.method != "fresh" {
                    send(&Response::Refused {
                        reason: format!(
                            "starting-state method {} unsupported; this adapter supports fresh",
                            state.method
                        ),
                    })?;
                    return Ok(());
                }
                component = Some(Component::new(variant));
                send(&Response::Started {
                    protocol: PROTOCOL_VERSION,
                    identity: identity(variant),
                    endpoints: endpoints(),
                    channels: channels(),
                    state: StartingState {
                        method: "fresh".to_string(),
                        blake3: fresh_state_blake3(),
                    },
                })?;
            }
            Request::Step { input, request } => {
                let Some(component) = component.as_mut() else {
                    bail!("step before start");
                };
                let ordinal = input.ordinal.context("step input has no ordinal")?;
                let event_type = parse_event_type(&input.event_type)
                    .with_context(|| format!("unknown event type {}", input.event_type))?;
                let body = decode_hex(&input.body_hex)?;
                let outcome = component.step(Input {
                    ordinal,
                    src_actor: input.src_actor,
                    dst_actor: input.dst_actor,
                    channel_id: input.channel_id,
                    event_type,
                    body: &body,
                    request: request
                        .filter(|request| request.src_actor == COMPONENT_ACTOR_ID)
                        .map(|request| (request.channel_id, request.src_seq)),
                });
                let callback = outcome.callback.unwrap_or(ordinal);
                for effect in &outcome.effects {
                    effects += 1;
                    send(&Response::Effect {
                        step: ordinal,
                        callback,
                        effect: WireFrame {
                            ordinal: None,
                            src_actor: COMPONENT_ACTOR_ID,
                            dst_actor: effect.dst_actor,
                            channel_id: effect.channel_id,
                            src_seq: effect.src_seq,
                            event_type: event_type_name(effect.event_type),
                            schema: 1,
                            flags: effect.flags,
                            body_hex: encode_hex(&effect.body),
                        },
                    })?;
                }
                for name in &outcome.progress {
                    send(&Response::Progress {
                        step: ordinal,
                        callback,
                        name: name.to_string(),
                    })?;
                }
                let (status, reason) = match outcome.consumed {
                    Some(Consumed::Yes) | None => (StepStatus::Ok, None),
                    Some(Consumed::No(reason)) => (StepStatus::NotConsumed, Some(reason)),
                    Some(Consumed::Refused(reason)) => (StepStatus::Refused, Some(reason)),
                };
                send(&Response::Stepped {
                    ordinal,
                    status,
                    reason,
                    pending: pending(&component.waits()),
                })?;
            }
            Request::Finish { reason } => {
                let waits = component.as_ref().map(Component::waits).unwrap_or_default();
                send(&Response::Finished {
                    reason,
                    pending: pending(&waits),
                    effects,
                })?;
                return Ok(());
            }
        }
    }
    Ok(())
}

/// The fresh starting state has no bytes; its identity is the hash of the
/// empty string, so a snapshot method can never be confused with it.
pub fn fresh_state_blake3() -> String {
    encode_hex(blake3::hash(b"").as_bytes())
}
