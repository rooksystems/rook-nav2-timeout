//! The adapter protocol: one JSON object per line, runner to adapter on
//! stdin, adapter to runner on stdout. Field names here are the contract.

use serde::{Deserialize, Serialize};

/// A frame as it crosses the protocol. `ordinal` is the recording's
/// ordinal for inputs the runner sends and is absent on effects, which the
/// runner numbers when it appends them to the replayed stream.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WireFrame {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordinal: Option<u64>,
    pub src_actor: u32,
    pub dst_actor: u32,
    pub channel_id: u32,
    pub src_seq: u64,
    pub event_type: String,
    pub schema: u16,
    pub flags: u32,
    pub body_hex: String,
}

/// The recorded frame a response answers.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RequestRef {
    pub ordinal: u64,
    pub src_actor: u32,
    pub dst_actor: u32,
    pub channel_id: u32,
    pub src_seq: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Identity {
    pub component: String,
    pub variant: String,
    pub adapter: String,
    pub protocol: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StartingState {
    pub method: String,
    pub blake3: String,
}

/// Runner to adapter.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Start {
        protocol: u32,
        corpus: String,
        scenario: String,
        state: StartingState,
    },
    /// `request`, present on response inputs, names the recorded request
    /// frame the response answers, resolved by the runner from the body's
    /// request ordinal so the adapter can bind it to its own issuance
    /// (channel and sequence) without knowing recorded ordinals.
    Step {
        input: WireFrame,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<RequestRef>,
    },
    Finish {
        reason: FinishReason,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// The runner delivered every recorded input.
    Exhausted,
    /// The runner stopped at its declared scope or at a gap or refusal.
    Scope,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Endpoint {
    pub actor: u32,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Channel {
    pub id: u32,
    pub name: String,
    pub direction: String,
    pub event_types: Vec<String>,
}

/// Something the component is still waiting on when a step returns.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Pending {
    /// The callback started by input `callback` is blocked on a clock read;
    /// the next step must be a Clock frame with that callback ordinal.
    ClockRead { callback: u64, clock_id: u16 },
    /// A future the callback is waiting on; the named response resolves it.
    Future { callback: u64, name: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    /// The input was consumed.
    Ok,
    /// The input was valid but nothing was waiting for it; the adapter did
    /// not apply it. Reported, never silently dropped.
    NotConsumed,
    /// The adapter refuses to consume it: the recorded input is no longer
    /// valid after an effect this run emitted that the recording's
    /// environment never saw.
    Refused,
}

/// Adapter to runner.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Started {
        protocol: u32,
        identity: Identity,
        endpoints: Vec<Endpoint>,
        channels: Vec<Channel>,
        state: StartingState,
    },
    Refused {
        reason: String,
    },
    /// An effect, written the moment the component produced it. `step` is
    /// the recorded ordinal of the input being processed; `callback` is the
    /// recorded ordinal of the input whose callback produced the effect,
    /// which differs when a clock read or a response resumed a waiting
    /// callback.
    Effect {
        step: u64,
        callback: u64,
        effect: WireFrame,
    },
    Progress {
        step: u64,
        callback: u64,
        name: String,
    },
    Stepped {
        ordinal: u64,
        status: StepStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        pending: Vec<Pending>,
    },
    Finished {
        reason: FinishReason,
        pending: Vec<Pending>,
        effects: u64,
    },
}
