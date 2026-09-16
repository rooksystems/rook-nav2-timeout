//! The decision component under test: a goal client that sends one goal per
//! command, waits for the goal response under a deadline, and reacts. Four
//! variants mirror the flagship's candidate matrix at toy scale. The
//! component never reads a clock itself; it asks, and the adapter feeds the
//! recorded observation, which is the whole point of the contract.

use std::collections::BTreeMap;

use rook_native::{COMPONENT_ACTOR_ID, EventType, FLAG_ATTEMPTED, FLAG_CONFIRMED};

use crate::bodies::{self, CancelSend, Clock, GoalResponse, GoalResult, GoalSend};
use crate::{
    ACTOR_ACTION_SERVER, ACTOR_CLOCK, ACTOR_COMMAND, ACTOR_DIAGNOSTICS, CH_ACTION_CANCEL,
    CH_ACTION_GOAL, CH_ACTION_RESPONSE, CH_CLOCK, CH_GOAL_COMMAND, CH_STATUS, CLOCK_ROS,
    GOAL_RESPONSE_DEADLINE_NS,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Variant {
    /// Times out and reports it, but never cancels the pending goal.
    Old,
    /// Times out and cancels the pending goal.
    Fixed,
    /// Does nothing at all.
    Noop,
    /// Cancels every goal the moment it is sent.
    AlwaysCancel,
}

impl Variant {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "old" => Some(Variant::Old),
            "fixed" => Some(Variant::Fixed),
            "noop" => Some(Variant::Noop),
            "always-cancel" => Some(Variant::AlwaysCancel),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Variant::Old => "old",
            Variant::Fixed => "fixed",
            Variant::Noop => "noop",
            Variant::AlwaysCancel => "always-cancel",
        }
    }
}

/// An effect the component produced. `src_seq` is the component's own
/// issuance counter on that channel, which is what a later response must
/// bind to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Effect {
    pub dst_actor: u32,
    pub channel_id: u32,
    pub src_seq: u64,
    pub event_type: EventType,
    pub flags: u32,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Wait {
    ClockRead { callback: u64, clock_id: u16 },
    Future { callback: u64, name: &'static str },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Consumed {
    Yes,
    No(String),
    Refused(String),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Outcome {
    pub consumed: Option<Consumed>,
    /// The recorded ordinal of the input whose callback this step ran or
    /// resumed. A goal command starts its own callback; a clock read or a
    /// response resumes the callback that was waiting for it.
    pub callback: Option<u64>,
    pub effects: Vec<Effect>,
    pub progress: Vec<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum State {
    Idle,
    /// The goal callback started at `callback` needs the current time.
    AwaitClockForSend {
        callback: u64,
        goal: Vec<u8>,
    },
    AwaitGoalResponse {
        callback: u64,
        goal_id: [u8; 16],
        send_seq: u64,
        deadline_ns: i64,
    },
    AwaitResult {
        callback: u64,
        goal_id: [u8; 16],
        send_seq: u64,
    },
    AwaitCancelResponse {
        callback: u64,
        goal_id: [u8; 16],
        cancel_seq: u64,
        cancel_reason: &'static str,
    },
}

pub struct Component {
    variant: Variant,
    state: State,
    goals_issued: u64,
    sequences: BTreeMap<u32, u64>,
}

/// The one input shape the component sees: which event, from whom, whose
/// callback, and for a response, the (channel, sequence) of the request it
/// answers as the runner resolved it from the recording.
pub struct Input<'a> {
    pub ordinal: u64,
    pub src_actor: u32,
    pub dst_actor: u32,
    pub channel_id: u32,
    pub event_type: EventType,
    pub body: &'a [u8],
    pub request: Option<(u32, u64)>,
}

impl Component {
    pub fn new(variant: Variant) -> Self {
        Component {
            variant,
            state: State::Idle,
            goals_issued: 0,
            sequences: BTreeMap::new(),
        }
    }

    pub fn variant(&self) -> Variant {
        self.variant
    }

    /// The reference's generated identity: derived from issuance order, so
    /// two runs that issue the same goals generate the same ids.
    pub fn goal_id(issue_index: u64) -> [u8; 16] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"rook-adapter-ref goal ");
        hasher.update(&issue_index.to_le_bytes());
        hasher.finalize().as_bytes()[..16].try_into().unwrap()
    }

    pub fn waits(&self) -> Vec<Wait> {
        match &self.state {
            State::Idle => vec![],
            State::AwaitClockForSend { callback, .. } => vec![Wait::ClockRead {
                callback: *callback,
                clock_id: CLOCK_ROS,
            }],
            State::AwaitGoalResponse { callback, .. } => vec![
                Wait::Future {
                    callback: *callback,
                    name: "goal_response",
                },
                Wait::ClockRead {
                    callback: *callback,
                    clock_id: CLOCK_ROS,
                },
            ],
            State::AwaitResult { callback, .. } => vec![Wait::Future {
                callback: *callback,
                name: "result",
            }],
            State::AwaitCancelResponse { callback, .. } => vec![Wait::Future {
                callback: *callback,
                name: "cancel_response",
            }],
        }
    }

    pub fn step(&mut self, input: Input<'_>) -> Outcome {
        let mut outcome = Outcome::default();
        if input.dst_actor != COMPONENT_ACTOR_ID {
            outcome.consumed = Some(Consumed::No(format!(
                "input addressed to actor {}, not the component",
                input.dst_actor
            )));
            return outcome;
        }
        match (input.channel_id, input.event_type) {
            (CH_GOAL_COMMAND, EventType::Message) => self.on_goal_command(input, &mut outcome),
            (CH_CLOCK, EventType::Clock) => self.on_clock(input, &mut outcome),
            (CH_ACTION_RESPONSE, EventType::GoalResponse) => {
                self.on_goal_response(input, &mut outcome)
            }
            (CH_ACTION_RESPONSE, EventType::Result) => self.on_result(input, &mut outcome),
            (CH_ACTION_RESPONSE, EventType::CancelResponse) => {
                self.on_cancel_response(input, &mut outcome)
            }
            (_, EventType::StartingState) => outcome.consumed = Some(Consumed::Yes),
            (channel, event_type) => {
                outcome.consumed = Some(Consumed::No(format!(
                    "no handler for {event_type:?} on channel {channel}"
                )))
            }
        }
        outcome
    }

    fn effect(
        &mut self,
        dst_actor: u32,
        channel_id: u32,
        event_type: EventType,
        body: Vec<u8>,
    ) -> Effect {
        let seq = self.sequences.entry(channel_id).or_insert(0);
        let effect = Effect {
            dst_actor,
            channel_id,
            src_seq: *seq,
            event_type,
            flags: FLAG_ATTEMPTED | FLAG_CONFIRMED,
            body,
        };
        *seq += 1;
        effect
    }

    fn status(&mut self, text: &str) -> Effect {
        self.effect(
            ACTOR_DIAGNOSTICS,
            CH_STATUS,
            EventType::Status,
            bodies::status(text),
        )
    }

    fn on_goal_command(&mut self, input: Input<'_>, outcome: &mut Outcome) {
        if input.src_actor != ACTOR_COMMAND {
            outcome.consumed = Some(Consumed::No(format!(
                "goal command from actor {}, not the command endpoint {ACTOR_COMMAND}",
                input.src_actor
            )));
            return;
        }
        if self.variant == Variant::Noop {
            outcome.consumed = Some(Consumed::Yes);
            return;
        }
        if self.state != State::Idle {
            outcome.consumed = Some(Consumed::No("a goal is already in flight".into()));
            return;
        }
        let goal = match bodies::decode_message(input.body) {
            Ok(goal) => goal.to_vec(),
            Err(error) => {
                outcome.consumed = Some(Consumed::No(error.to_string()));
                return;
            }
        };
        self.state = State::AwaitClockForSend {
            callback: input.ordinal,
            goal,
        };
        outcome.callback = Some(input.ordinal);
        outcome.consumed = Some(Consumed::Yes);
    }

    /// A clock observation is consumed only by the read that asked for it:
    /// same callback, same clock, from the clock endpoint.
    fn on_clock(&mut self, input: Input<'_>, outcome: &mut Outcome) {
        let clock = match Clock::decode(input.body) {
            Ok(clock) => clock,
            Err(error) => {
                outcome.consumed = Some(Consumed::No(error.to_string()));
                return;
            }
        };
        let pending = self.waits().into_iter().find_map(|wait| match wait {
            Wait::ClockRead { callback, clock_id } => Some((callback, clock_id)),
            Wait::Future { .. } => None,
        });
        let matches = pending == Some((clock.callback_ordinal, clock.clock_id))
            && input.src_actor == ACTOR_CLOCK;
        if !matches {
            outcome.consumed = Some(Consumed::No(match pending {
                Some((callback, clock_id)) => format!(
                    "clock observation (actor {}, clock {}, callback {}) does not match the pending read (actor {ACTOR_CLOCK}, clock {clock_id}, callback {callback})",
                    input.src_actor, clock.clock_id, clock.callback_ordinal
                ),
                None => format!(
                    "no pending clock read for callback {}",
                    clock.callback_ordinal
                ),
            }));
            return;
        }
        match self.state.clone() {
            State::AwaitClockForSend { callback, goal } => {
                // Only the waiting variants need a deadline; always-cancel
                // never waits, so a huge clock value is still a valid read.
                let deadline_ns = match self.variant {
                    Variant::AlwaysCancel => None,
                    _ => match clock.value_ns.checked_add(GOAL_RESPONSE_DEADLINE_NS) {
                        Some(deadline_ns) => Some(deadline_ns),
                        None => {
                            outcome.consumed = Some(Consumed::No(
                                "clock value leaves no room for the goal response deadline".into(),
                            ));
                            return;
                        }
                    },
                };
                outcome.callback = Some(callback);
                let goal_id = Self::goal_id(self.goals_issued);
                self.goals_issued += 1;
                let send = self.effect(
                    ACTOR_ACTION_SERVER,
                    CH_ACTION_GOAL,
                    EventType::GoalSend,
                    GoalSend { goal_id, goal }.encode(),
                );
                let send_seq = send.src_seq;
                outcome.effects.push(send);
                outcome.progress.push("goal_submitted");
                if self.variant == Variant::AlwaysCancel {
                    let cancel = self.effect(
                        ACTOR_ACTION_SERVER,
                        CH_ACTION_CANCEL,
                        EventType::CancelSend,
                        CancelSend {
                            goal_id,
                            stamp_ns: clock.value_ns,
                        }
                        .encode(),
                    );
                    let cancel_seq = cancel.src_seq;
                    outcome.effects.push(cancel);
                    outcome.progress.push("cancel_requested");
                    self.state = State::AwaitCancelResponse {
                        callback,
                        goal_id,
                        cancel_seq,
                        cancel_reason: "unconditional",
                    };
                } else {
                    self.state = State::AwaitGoalResponse {
                        callback,
                        goal_id,
                        send_seq,
                        deadline_ns: deadline_ns
                            .expect("computed for every variant but always-cancel"),
                    };
                }
                outcome.consumed = Some(Consumed::Yes);
            }
            State::AwaitGoalResponse {
                callback,
                goal_id,
                deadline_ns,
                ..
            } => {
                outcome.callback = Some(callback);
                outcome.consumed = Some(Consumed::Yes);
                if clock.value_ns < deadline_ns {
                    return;
                }
                outcome.progress.push("deadline_expired");
                match self.variant {
                    Variant::Fixed => {
                        let cancel = self.effect(
                            ACTOR_ACTION_SERVER,
                            CH_ACTION_CANCEL,
                            EventType::CancelSend,
                            CancelSend {
                                goal_id,
                                stamp_ns: clock.value_ns,
                            }
                            .encode(),
                        );
                        let cancel_seq = cancel.src_seq;
                        outcome.effects.push(cancel);
                        outcome.progress.push("cancel_requested");
                        self.state = State::AwaitCancelResponse {
                            callback,
                            goal_id,
                            cancel_seq,
                            cancel_reason: "goal response timeout",
                        };
                    }
                    _ => {
                        let status = self.status("goal response timed out; goal left pending");
                        outcome.effects.push(status);
                        self.state = State::Idle;
                    }
                }
            }
            _ => unreachable!("a pending clock read implies one of the two waiting states"),
        }
    }

    /// A response is bound to a request by the runner-resolved (channel,
    /// sequence) of the request plus the raw goal id, and must come from the
    /// action server. Anything else is reported and not applied.
    fn bound(input: &Input<'_>, channel_id: u32, seq: u64) -> bool {
        input.src_actor == ACTOR_ACTION_SERVER && input.request == Some((channel_id, seq))
    }

    fn on_goal_response(&mut self, input: Input<'_>, outcome: &mut Outcome) {
        let response = match GoalResponse::decode(input.body) {
            Ok(response) => response,
            Err(error) => {
                outcome.consumed = Some(Consumed::No(error.to_string()));
                return;
            }
        };
        match self.state.clone() {
            State::AwaitGoalResponse {
                callback,
                goal_id,
                send_seq,
                ..
            } if goal_id == response.goal_id && Self::bound(&input, CH_ACTION_GOAL, send_seq) => {
                outcome.callback = Some(callback);
                outcome.consumed = Some(Consumed::Yes);
                outcome.progress.push("goal_acknowledged");
                if response.accepted {
                    let status = self.status("goal accepted");
                    outcome.effects.push(status);
                    self.state = State::AwaitResult {
                        callback,
                        goal_id,
                        send_seq,
                    };
                } else {
                    let status = self.status("goal rejected");
                    outcome.effects.push(status);
                    self.state = State::Idle;
                }
            }
            State::AwaitCancelResponse {
                goal_id,
                cancel_reason,
                ..
            } if goal_id == response.goal_id => {
                outcome.consumed = Some(Consumed::Refused(format!(
                    "recorded goal_response for this goal is invalid after this run's cancel_send ({cancel_reason}); the recording's environment never saw that cancel"
                )));
            }
            _ => {
                outcome.consumed = Some(Consumed::No(
                    "goal_response is not bound to a goal awaiting a response".into(),
                ));
            }
        }
    }

    fn on_result(&mut self, input: Input<'_>, outcome: &mut Outcome) {
        let result = match GoalResult::decode(input.body) {
            Ok(result) => result,
            Err(error) => {
                outcome.consumed = Some(Consumed::No(error.to_string()));
                return;
            }
        };
        match self.state.clone() {
            State::AwaitResult {
                callback,
                goal_id,
                send_seq,
            } if goal_id == result.goal_id && Self::bound(&input, CH_ACTION_GOAL, send_seq) => {
                outcome.callback = Some(callback);
                outcome.consumed = Some(Consumed::Yes);
                outcome.progress.push("goal_terminated");
                let status = self.status(&format!("goal finished with status {}", result.status));
                outcome.effects.push(status);
                self.state = State::Idle;
            }
            State::AwaitCancelResponse { goal_id, .. } if goal_id == result.goal_id => {
                outcome.consumed = Some(Consumed::Refused(
                    "recorded result for this goal is invalid after this run's cancel_send; the recording's environment never saw that cancel".into(),
                ));
            }
            _ => {
                outcome.consumed = Some(Consumed::No(
                    "result is not bound to a goal awaiting a result".into(),
                ));
            }
        }
    }

    fn on_cancel_response(&mut self, input: Input<'_>, outcome: &mut Outcome) {
        let response = match bodies::CancelResponse::decode(input.body) {
            Ok(response) => response,
            Err(error) => {
                outcome.consumed = Some(Consumed::No(error.to_string()));
                return;
            }
        };
        match self.state.clone() {
            State::AwaitCancelResponse {
                callback,
                goal_id,
                cancel_seq,
                ..
            } if response.goals.contains(&goal_id)
                && Self::bound(&input, CH_ACTION_CANCEL, cancel_seq) =>
            {
                outcome.callback = Some(callback);
                outcome.consumed = Some(Consumed::Yes);
                outcome.progress.push("cancel_acknowledged");
                let status = self.status("cancel acknowledged");
                outcome.effects.push(status);
                self.state = State::Idle;
            }
            _ => {
                outcome.consumed = Some(Consumed::No(
                    "cancel_response is not bound to a pending cancel".into(),
                ));
            }
        }
    }
}

/// The component's own actor id, for adapters that forward its effects.
pub const ACTOR: u32 = COMPONENT_ACTOR_ID;
