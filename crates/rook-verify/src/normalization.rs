//! Goal correspondence is established at issuance and retained for the whole run.
//! Responses can only use an existing pair whose raw recorded ID names that issuance.
use crate::case::{Identity, entry, execution_mismatch, hash};
use anyhow::{Context, Result, ensure};
use rook_adapter_ref::bodies::CancelResponse;
use rook_native::{EventType, Frame};
use std::collections::{BTreeMap, BTreeSet};

pub const POLICY: &str = "goal-ids-by-issuance@1";
pub fn source_hash() -> String {
    hash(include_bytes!("normalization.rs"))
}
type GoalId = [u8; 16];
#[derive(Clone)]
struct Pair {
    recorded: GoalId,
    replayed: GoalId,
    client: String,
}

pub struct Goals {
    enabled: bool,
    clients: BTreeMap<u32, String>,
    issuances: BTreeMap<String, Vec<(u64, GoalId)>>,
    counts: BTreeMap<String, usize>,
    pairs: BTreeMap<u64, Pair>,
    seen: BTreeSet<(String, GoalId)>,
}
impl Goals {
    pub fn new(recording: &[Frame], identity: &Identity, policy: &str) -> Result<Self> {
        let normalization = entry(identity, "normalization")?;
        let enabled = normalization == POLICY;
        ensure!(
            (normalization == "none@1" && policy == "raw-bytes-in-order@1")
                || (enabled && policy == "goal-ids-in-order@1"),
            "invalid declared normalization contract {normalization} with {policy}"
        );
        if !enabled {
            ensure!(
                entry(identity, "normalization_source_blake3")?
                    == rook_adapter_ref::source_identity()["normalization_source_blake3"],
                "identity drift at normalization_source_blake3"
            );
        }
        let mut clients = BTreeMap::new();
        if enabled {
            ensure!(
                entry(identity, "normalization_source_blake3")? == source_hash(),
                "identity drift at normalization_source_blake3"
            );
            for (key, value) in identity {
                if let Some(channel) = key.strip_prefix("normalization.client.") {
                    ensure!(
                        clients.insert(channel.parse()?, value.clone()).is_none(),
                        "ambiguous normalization client for channel {channel}"
                    );
                }
            }
            ensure!(
                !clients.is_empty(),
                "normalization client/channel configuration is missing"
            );
        }
        let mut goals = Self {
            enabled,
            clients,
            issuances: BTreeMap::new(),
            counts: BTreeMap::new(),
            pairs: BTreeMap::new(),
            seen: BTreeSet::new(),
        };
        let mut seen = BTreeSet::new();
        for frame in recording
            .iter()
            .filter(|f| f.header.event_type == EventType::GoalSend)
        {
            let client = goals.client(frame)?;
            let id = id_at(&frame.body, 0)?;
            ensure!(
                id != [0; 16],
                "zero ID cannot identify an issued goal at {}",
                frame.ordinal
            );
            ensure!(
                seen.insert((client.clone(), id)),
                "duplicate recorded goal identity at {}",
                frame.ordinal
            );
            goals
                .issuances
                .entry(client)
                .or_default()
                .push((frame.ordinal, id));
        }
        Ok(goals)
    }
    fn client(&self, frame: &Frame) -> Result<String> {
        if self.enabled {
            self.clients
                .get(&frame.channel_id)
                .cloned()
                .with_context(|| {
                    format!(
                        "missing originating client for channel {}",
                        frame.channel_id
                    )
                })
        } else {
            Ok(format!("actor-{}", frame.src_actor))
        }
    }
    /// Extra issuance remains an extra effect. No mapping is invented for it.
    pub fn effect(&mut self, raw: &Frame) -> Result<Frame> {
        let mut normalized = raw.clone();
        match raw.header.event_type {
            EventType::GoalSend => {
                let client = self.client(raw)?;
                let id = id_at(&raw.body, 0)?;
                if id == [0; 16] || !self.seen.insert((client.clone(), id)) {
                    return Err(execution_mismatch(format!(
                        "duplicate or zero replay goal identity at {}",
                        raw.ordinal
                    )));
                }
                let count = self.counts.entry(client.clone()).or_default();
                if let Some((ordinal, recorded)) =
                    self.issuances.get(&client).and_then(|v| v.get(*count))
                {
                    self.pairs.insert(
                        *ordinal,
                        Pair {
                            recorded: *recorded,
                            replayed: id,
                            client,
                        },
                    );
                    if self.enabled {
                        normalized.body[..16].copy_from_slice(recorded);
                    }
                }
                *count += 1;
            }
            EventType::CancelSend => {
                let client = self.client(raw)?;
                let id = id_at(&raw.body, 0)?;
                // Zero IDs denote wildcard/timestamp cancellation. The timestamp is
                // untouched for targeted cancellation too; it is never a goal ID.
                if id != [0; 16] {
                    let pair = self
                        .pairs
                        .values()
                        .find(|p| p.client == client && p.replayed == id);
                    if let Some(pair) = pair {
                        if self.enabled {
                            normalized.body[..16].copy_from_slice(&pair.recorded);
                        }
                    } else if !self.seen.contains(&(client, id)) {
                        return Err(execution_mismatch("cancel names an unknown goal identity"));
                    }
                }
            }
            _ => {}
        }
        Ok(normalized)
    }
    /// Check raw response identity against its named request before translating it.
    /// A stale or wrong ID must not be rewritten to the currently expected goal.
    pub fn input(&self, raw: &Frame, request: &Frame) -> Result<Frame> {
        let mut input = raw.clone();
        match raw.header.event_type {
            EventType::GoalResponse | EventType::Feedback | EventType::Result => {
                ensure!(
                    request.header.event_type == EventType::GoalSend,
                    "response does not name a goal issuance"
                );
                let pair = self
                    .pairs
                    .get(&request.ordinal)
                    .context("response needs a missing corresponding goal issuance")?;
                ensure!(
                    id_at(&raw.body, 8)? == pair.recorded,
                    "wrong or stale response identity at recorded ordinal {}",
                    raw.ordinal
                );
                if self.enabled {
                    input.body[8..24].copy_from_slice(&pair.replayed);
                } else {
                    ensure!(
                        pair.recorded == pair.replayed,
                        "response goal identity differs from issued goal"
                    );
                }
            }
            EventType::CancelResponse => {
                ensure!(
                    request.header.event_type == EventType::CancelSend,
                    "cancel response does not name a cancel request"
                );
                let client = self.client(request)?;
                let mut response = CancelResponse::decode(&raw.body)?;
                let mut seen = BTreeSet::new();
                for id in &mut response.goals {
                    ensure!(
                        seen.insert(*id),
                        "duplicate goal identity in cancel response"
                    );
                    let pair = self
                        .pairs
                        .values()
                        .find(|p| p.client == client && p.recorded == *id)
                        .context("unknown goal identity in cancel response")?;
                    if self.enabled {
                        *id = pair.replayed;
                    } else {
                        ensure!(
                            pair.recorded == pair.replayed,
                            "cancel response goal identity differs"
                        );
                    }
                }
                input.body = response.encode()?;
            }
            _ => {}
        }
        Ok(input)
    }
}
fn id_at(bytes: &[u8], offset: usize) -> Result<GoalId> {
    Ok(bytes
        .get(offset..offset + 16)
        .context("missing goal identity")?
        .try_into()?)
}
