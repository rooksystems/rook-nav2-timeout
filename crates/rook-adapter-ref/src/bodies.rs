//! Typed bodies for the event types this adapter uses. Layouts are the
//! ones in docs/internals/native-contract.md, all little-endian, no padding.

use anyhow::{Result, bail};

pub const SERIALIZATION_RAW: u16 = 2;

/// Message: serialization u16, then the bytes.
pub fn message(bytes: &[u8]) -> Vec<u8> {
    let mut body = SERIALIZATION_RAW.to_le_bytes().to_vec();
    body.extend_from_slice(bytes);
    body
}

pub fn decode_message(body: &[u8]) -> Result<&[u8]> {
    if body.len() < 2 {
        bail!("message body shorter than its serialization field");
    }
    let serialization = u16::from_le_bytes([body[0], body[1]]);
    if serialization != SERIALIZATION_RAW {
        bail!("unsupported message serialization {serialization}; this adapter reads raw (2)");
    }
    Ok(&body[2..])
}

/// Clock: clock_id u16, value_ns i64, callback_ordinal u64.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Clock {
    pub clock_id: u16,
    pub value_ns: i64,
    pub callback_ordinal: u64,
}

impl Clock {
    pub fn encode(&self) -> Vec<u8> {
        let mut body = self.clock_id.to_le_bytes().to_vec();
        body.extend_from_slice(&self.value_ns.to_le_bytes());
        body.extend_from_slice(&self.callback_ordinal.to_le_bytes());
        body
    }

    pub fn decode(body: &[u8]) -> Result<Self> {
        if body.len() != 18 {
            bail!("clock body is {} bytes, expected 18", body.len());
        }
        Ok(Clock {
            clock_id: u16::from_le_bytes([body[0], body[1]]),
            value_ns: i64::from_le_bytes(body[2..10].try_into()?),
            callback_ordinal: u64::from_le_bytes(body[10..18].try_into()?),
        })
    }
}

/// GoalSend: raw goal id [16], then the goal bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalSend {
    pub goal_id: [u8; 16],
    pub goal: Vec<u8>,
}

impl GoalSend {
    pub fn encode(&self) -> Vec<u8> {
        let mut body = self.goal_id.to_vec();
        body.extend_from_slice(&self.goal);
        body
    }

    pub fn decode(body: &[u8]) -> Result<Self> {
        if body.len() < 16 {
            bail!("goal_send body shorter than a goal id");
        }
        Ok(GoalSend {
            goal_id: body[0..16].try_into()?,
            goal: body[16..].to_vec(),
        })
    }
}

/// GoalResponse: goal_send_ordinal u64, raw goal id [16], accepted u8,
/// stamp_ns i64. The ordinal is the recording's ordinal of the GoalSend
/// this answers; the goal id is the correlation key the adapter uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GoalResponse {
    pub goal_send_ordinal: u64,
    pub goal_id: [u8; 16],
    pub accepted: bool,
    pub stamp_ns: i64,
}

impl GoalResponse {
    pub fn encode(&self) -> Vec<u8> {
        let mut body = self.goal_send_ordinal.to_le_bytes().to_vec();
        body.extend_from_slice(&self.goal_id);
        body.push(u8::from(self.accepted));
        body.extend_from_slice(&self.stamp_ns.to_le_bytes());
        body
    }

    pub fn decode(body: &[u8]) -> Result<Self> {
        if body.len() != 33 {
            bail!("goal_response body is {} bytes, expected 33", body.len());
        }
        Ok(GoalResponse {
            goal_send_ordinal: u64::from_le_bytes(body[0..8].try_into()?),
            goal_id: body[8..24].try_into()?,
            accepted: body[24] != 0,
            stamp_ns: i64::from_le_bytes(body[25..33].try_into()?),
        })
    }
}

/// Result: goal_send_ordinal u64, raw goal id [16], status u8, bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalResult {
    pub goal_send_ordinal: u64,
    pub goal_id: [u8; 16],
    pub status: u8,
    pub result: Vec<u8>,
}

pub const RESULT_SUCCEEDED: u8 = 4;
pub const RESULT_CANCELED: u8 = 5;
pub const RESULT_ABORTED: u8 = 6;

impl GoalResult {
    pub fn encode(&self) -> Vec<u8> {
        let mut body = self.goal_send_ordinal.to_le_bytes().to_vec();
        body.extend_from_slice(&self.goal_id);
        body.push(self.status);
        body.extend_from_slice(&self.result);
        body
    }

    pub fn decode(body: &[u8]) -> Result<Self> {
        if body.len() < 25 {
            bail!("result body shorter than its header");
        }
        Ok(GoalResult {
            goal_send_ordinal: u64::from_le_bytes(body[0..8].try_into()?),
            goal_id: body[8..24].try_into()?,
            status: body[24],
            result: body[25..].to_vec(),
        })
    }
}

/// CancelSend: target raw goal id [16] (all zero is the wildcard), stamp_ns i64.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CancelSend {
    pub goal_id: [u8; 16],
    pub stamp_ns: i64,
}

impl CancelSend {
    pub fn encode(&self) -> Vec<u8> {
        let mut body = self.goal_id.to_vec();
        body.extend_from_slice(&self.stamp_ns.to_le_bytes());
        body
    }

    pub fn decode(body: &[u8]) -> Result<Self> {
        if body.len() != 24 {
            bail!("cancel_send body is {} bytes, expected 24", body.len());
        }
        Ok(CancelSend {
            goal_id: body[0..16].try_into()?,
            stamp_ns: i64::from_le_bytes(body[16..24].try_into()?),
        })
    }
}

/// CancelResponse: cancel_send_ordinal u64, return_code u8, n u16, n goal ids.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancelResponse {
    pub cancel_send_ordinal: u64,
    pub return_code: u8,
    pub goals: Vec<[u8; 16]>,
}

impl CancelResponse {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let count = u16::try_from(self.goals.len())
            .map_err(|_| anyhow::anyhow!("cancel_response holds more than 65535 goals"))?;
        let mut body = self.cancel_send_ordinal.to_le_bytes().to_vec();
        body.push(self.return_code);
        body.extend_from_slice(&count.to_le_bytes());
        for goal in &self.goals {
            body.extend_from_slice(goal);
        }
        Ok(body)
    }

    pub fn decode(body: &[u8]) -> Result<Self> {
        if body.len() < 11 {
            bail!("cancel_response body shorter than its header");
        }
        let count = u16::from_le_bytes([body[9], body[10]]) as usize;
        if body.len() != 11 + 16 * count {
            bail!("cancel_response goal count does not match its length");
        }
        Ok(CancelResponse {
            cancel_send_ordinal: u64::from_le_bytes(body[0..8].try_into()?),
            return_code: body[8],
            goals: body[11..]
                .chunks(16)
                .map(|chunk| chunk.try_into().unwrap())
                .collect(),
        })
    }
}

/// StartingState: method u16, state blake3 [32].
pub const STATE_METHOD_FRESH: u16 = 1;

pub fn starting_state(method: u16, state_blake3: [u8; 32]) -> Vec<u8> {
    let mut body = method.to_le_bytes().to_vec();
    body.extend_from_slice(&state_blake3);
    body
}

/// Status: diagnostic text, never an action command.
pub fn status(text: &str) -> Vec<u8> {
    text.as_bytes().to_vec()
}
