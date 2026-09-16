//! Replays a recording of the real `rmf_traffic_ros2` blockade node through
//! the unmodified cell and scores conformance.
//!
//! The recording comes from `tools/rook_record.py`: every blockade message
//! the recorder saw, in receive order, with receive timestamps as ticks
//! (`P0_ingest`). The recorded `min_conflict_angle` is read from the
//! fixture's `params` file, not hard-coded here.
//!
//! Conformance is the longest common subsequence of distinct decision
//! states (statuses sorted by participant, consecutive duplicates dropped,
//! starting from the implicit empty state). ADR 0003 uses this metric.
//! It is computed twice: on the raw recording, and after the
//! `P1_lifecycle` reorder (a cancel with no ready/reached of the same
//! participant between it and the preceding set moves before that set),
//! which undoes the set/cancel cross-topic race described in ADR 0003.
//! With `--goldens <file>`, every metric and hash is compared against the
//! committed golden and any drift fails the run.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use rook_core::{CELL_ACTOR_ID, Envelope, EnvelopeKind, ReplayHashes, hash_observed};

use crate::encode_hex;
use crate::rmf::{BagDelivery, replay_bag_deliveries};

const BAG_TRACE_DOMAIN: &[u8] = b"rook-trace-v1/rmf-blockade-bag@1";

const CH_SET: u32 = 10;
const CH_READY: u32 = 11;
const CH_REACHED: u32 = 12;
#[cfg(test)]
const CH_RELEASE: u32 = 13;
const CH_CANCEL: u32 = 14;
const CH_HEARTBEAT_TIMER: u32 = 15;

pub fn run(cell_path: &Path, dir: &Path, goldens_path: Option<&Path>) -> Result<()> {
    let deliveries = read_deliveries(&dir.join("deliveries.bin"))?;
    let recorded_heartbeats = read_heartbeats(&dir.join("heartbeats_recorded.bin"))?;
    let min_conflict_angle = read_min_conflict_angle(&dir.join("params"))?;
    if deliveries.is_empty() {
        bail!("no deliveries in {}", dir.display());
    }
    println!(
        "bag: {} deliveries, {} recorded heartbeats, {:.1} s span",
        deliveries.len(),
        recorded_heartbeats.len(),
        (deliveries.last().unwrap().tick - deliveries[0].tick) as f64 / 1e9
    );

    let mut results = BTreeMap::new();

    // Replay integrity first: two fresh instances must agree byte for byte.
    let raw_emits = replay_bag_deliveries(cell_path, min_conflict_angle, &deliveries)?;
    let raw_again = replay_bag_deliveries(cell_path, min_conflict_angle, &deliveries)?;
    let raw_hashes = bag_hashes(&deliveries, &raw_emits)?;
    if raw_hashes != bag_hashes(&deliveries, &raw_again)? {
        bail!("replay is not deterministic across fresh instances");
    }
    results.insert("bag_run_hash", encode_hex(&raw_hashes.run_hash));
    results.insert("bag_output_digest", encode_hex(&raw_hashes.output_digest));

    // Conformance second: does the cell decide what the real node decided?
    let recorded_states = distinct_states(recorded_heartbeats.iter().map(|h| h.1.as_slice()));
    let raw_states = distinct_states(raw_emits.iter().map(|e| e.1.as_slice()));
    results.insert("recorded_states", recorded_states.len().to_string());
    results.insert("replay_states", raw_states.len().to_string());
    results.insert("replay_heartbeats", raw_emits.len().to_string());
    let heartbeats_identical = recorded_heartbeats
        .iter()
        .map(|(_, payload)| payload.as_slice())
        .eq(raw_emits.iter().map(|(_, payload)| payload.as_slice()));
    results.insert(
        "heartbeats_identical",
        u8::from(heartbeats_identical).to_string(),
    );
    results.insert(
        "raw_lcs",
        lcs_len(&recorded_states, &raw_states).to_string(),
    );

    let (reordered, moved) = reorder_lifecycle(&deliveries);
    let p1_emits = replay_bag_deliveries(cell_path, min_conflict_angle, &reordered)?;
    let p1_hashes = bag_hashes(&reordered, &p1_emits)?;
    let p1_states = distinct_states(p1_emits.iter().map(|e| e.1.as_slice()));
    results.insert("p1_cancels_moved", moved.to_string());
    results.insert("p1_output_digest", encode_hex(&p1_hashes.output_digest));
    results.insert("p1_lcs", lcs_len(&recorded_states, &p1_states).to_string());

    let (per_part_lcs, per_part_total) = per_participant_lcs(&recorded_states, &p1_states)?;
    results.insert("p1_per_participant_lcs", per_part_lcs.to_string());
    results.insert("p1_per_participant_total", per_part_total.to_string());

    for (key, value) in &results {
        println!("{key}={value}");
    }

    if let Some(goldens_path) = goldens_path {
        verify_goldens(goldens_path, &results)?;
        println!("goldens: all values match {}", goldens_path.display());
    }
    Ok(())
}

/// Fails on any golden that is missing, unexpected, or different. A golden
/// changes only when a session was named the file and told to (AGENTS.md).
pub fn verify_goldens(path: &Path, results: &BTreeMap<&str, String>) -> Result<()> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read goldens {}", path.display()))?;
    let mut expected = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .with_context(|| format!("malformed golden line: {line}"))?;
        expected.insert(key.trim().to_string(), value.trim().to_string());
    }
    let mut failures = Vec::new();
    for (key, want) in &expected {
        match results.get(key.as_str()) {
            Some(got) if got == want => {}
            Some(got) => failures.push(format!("{key}: golden {want}, got {got}")),
            None => failures.push(format!("{key}: golden present but value not produced")),
        }
    }
    for key in results.keys() {
        if !expected.contains_key(*key) {
            failures.push(format!("{key}: produced but missing from goldens"));
        }
    }
    if !failures.is_empty() {
        bail!("golden mismatch:\n  {}", failures.join("\n  "));
    }
    Ok(())
}

/// The recorded configuration: `min_conflict_angle_radians = <float>`.
pub fn read_min_conflict_angle(path: &Path) -> Result<f64> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("fixture params missing: {}", path.display()))?;
    for line in text.lines() {
        if let Some((key, value)) = line.split_once('=')
            && key.trim() == "min_conflict_angle_radians"
        {
            return value
                .trim()
                .parse::<f64>()
                .context("parse min_conflict_angle_radians");
        }
    }
    bail!("min_conflict_angle_radians not found in {}", path.display())
}

/// `P1_lifecycle`: a cancel with no ready/reached of the same participant
/// between it and the preceding set moves to just before that set. Ticks are
/// rewritten strictly increasing afterwards, matching the recorder's rule.
fn reorder_lifecycle(deliveries: &[BagDelivery]) -> (Vec<BagDelivery>, usize) {
    let mut messages: Vec<(BagDelivery, bool)> = deliveries
        .iter()
        .cloned()
        .map(|message| (message, false))
        .collect();
    let mut moved = 0_usize;
    loop {
        let mut last_set_index: BTreeMap<u64, usize> = BTreeMap::new();
        let mut blocked: Vec<u64> = Vec::new();
        let mut action: Option<(usize, usize)> = None;
        for (index, (message, was_moved)) in messages.iter().enumerate() {
            let Some(participant) = participant_of(message) else {
                continue;
            };
            match message.channel_id {
                CH_SET => {
                    last_set_index.insert(participant, index);
                    blocked.retain(|p| *p != participant);
                }
                CH_READY | CH_REACHED if !blocked.contains(&participant) => {
                    blocked.push(participant);
                }
                CH_CANCEL if !was_moved => {
                    if let Some(&set_index) = last_set_index.get(&participant)
                        && !blocked.contains(&participant)
                    {
                        action = Some((index, set_index));
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some((cancel_index, set_index)) = action else {
            break;
        };
        let (cancel, _) = messages.remove(cancel_index);
        messages.insert(set_index, (cancel, true));
        moved += 1;
    }
    let mut messages: Vec<BagDelivery> = messages.into_iter().map(|(message, _)| message).collect();
    let mut previous_tick = 0_u64;
    for message in &mut messages {
        message.tick = message.tick.max(previous_tick + 1);
        previous_tick = message.tick;
    }
    (messages, moved)
}

fn participant_of(message: &BagDelivery) -> Option<u64> {
    if message.channel_id == CH_HEARTBEAT_TIMER {
        return None;
    }
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&message.payload[0..8]);
    Some(u64::from_le_bytes(bytes))
}

/// Longest common subsequence length over whole decision states.
fn lcs_len<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    let mut previous = vec![0_usize; b.len() + 1];
    let mut current = vec![0_usize; b.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            current[j] = if a[i] == b[j] {
                previous[j + 1] + 1
            } else {
                previous[j].max(current[j + 1])
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[0]
}

/// Splits state sequences per participant (dropping the participant id from
/// each status, keeping res/any_ready/last_ready/last_reached/begin/end) and
/// sums LCS across participants. ADR 0003 uses this per-participant metric.
fn per_participant_lcs(recorded: &[Vec<u8>], replayed: &[Vec<u8>]) -> Result<(usize, usize)> {
    let recorded_per = split_per_participant(recorded)?;
    let replayed_per = split_per_participant(replayed)?;
    let mut matched = 0_usize;
    let mut total = 0_usize;
    for (participant, recorded_states) in &recorded_per {
        let empty = Vec::new();
        let replayed_states = replayed_per.get(participant).unwrap_or(&empty);
        matched += lcs_len(recorded_states, replayed_states);
        total += recorded_states.len();
    }
    Ok((matched, total))
}

type PerParticipant = BTreeMap<u64, Vec<Vec<u8>>>;

fn split_per_participant(states: &[Vec<u8>]) -> Result<PerParticipant> {
    let mut sequences: PerParticipant = BTreeMap::new();
    for state in states {
        if state.len() < 5 {
            bail!("short heartbeat state");
        }
        let count = u32::from_le_bytes(state[1..5].try_into()?) as usize;
        let mut offset = 5_usize;
        for _ in 0..count {
            let status = state
                .get(offset..offset + 49)
                .context("truncated status in heartbeat state")?;
            let participant = u64::from_le_bytes(status[0..8].try_into()?);
            let value = status[8..].to_vec();
            let sequence = sequences.entry(participant).or_default();
            if sequence.last() != Some(&value) {
                sequence.push(value);
            }
            offset += 49;
        }
    }
    Ok(sequences)
}

/// A state is a heartbeat payload: gridlock flag plus statuses already sorted
/// by participant. Both streams start from the moderator's empty state.
pub fn distinct_states<'a>(payloads: impl Iterator<Item = &'a [u8]>) -> Vec<Vec<u8>> {
    let empty_state: Vec<u8> = vec![0, 0, 0, 0, 0];
    let mut states = vec![empty_state];
    for payload in payloads {
        if states.last().map(Vec::as_slice) != Some(payload) {
            states.push(payload.to_vec());
        }
    }
    states
}

pub fn bag_hashes(deliveries: &[BagDelivery], emits: &[(u64, Vec<u8>)]) -> Result<ReplayHashes> {
    let delivery_envelopes: Vec<Envelope> = deliveries
        .iter()
        .enumerate()
        .map(|(index, delivery)| Envelope {
            kind: EnvelopeKind::Deliver,
            tick: delivery.tick,
            src_actor: delivery.channel_id,
            dst_actor: CELL_ACTOR_ID,
            channel_id: delivery.channel_id,
            payload_len: delivery.payload.len() as u32,
            src_seq: index as u64,
            payload_blake3: *blake3::hash(&delivery.payload).as_bytes(),
        })
        .collect();
    let emit_envelopes: Vec<Envelope> = emits
        .iter()
        .enumerate()
        .map(|(index, (tick, payload))| Envelope {
            kind: EnvelopeKind::Emit,
            tick: *tick,
            src_actor: CELL_ACTOR_ID,
            dst_actor: rook_core::OUTPUT_ACTOR_ID,
            channel_id: 20,
            payload_len: payload.len() as u32,
            src_seq: index as u64,
            payload_blake3: *blake3::hash(payload).as_bytes(),
        })
        .collect();
    hash_observed(BAG_TRACE_DOMAIN, &delivery_envelopes, &emit_envelopes)
        .map_err(|error| anyhow::anyhow!("hash_observed failed: {error:?}"))
}

pub fn read_deliveries(path: &Path) -> Result<Vec<BagDelivery>> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let mut deliveries = Vec::new();
    let mut cursor = 0_usize;
    while cursor < bytes.len() {
        let header = bytes
            .get(cursor..cursor + 16)
            .context("truncated delivery header")?;
        let tick = u64::from_le_bytes(header[0..8].try_into()?);
        let channel_id = u32::from_le_bytes(header[8..12].try_into()?);
        let length = u32::from_le_bytes(header[12..16].try_into()?) as usize;
        let payload = bytes
            .get(cursor + 16..cursor + 16 + length)
            .context("truncated delivery payload")?
            .to_vec();
        match (channel_id, payload.is_empty()) {
            (CH_HEARTBEAT_TIMER, true) => {}
            (CH_HEARTBEAT_TIMER, false) => {
                bail!("heartbeat timer delivery payload must be empty");
            }
            (_, _) if payload.len() < 8 => {
                bail!("delivery payload shorter than a participant id");
            }
            _ => {}
        }
        if let Some(previous) = deliveries.last() {
            let previous: &BagDelivery = previous;
            if tick <= previous.tick {
                bail!("delivery ticks must be strictly increasing");
            }
        }
        deliveries.push(BagDelivery {
            tick,
            channel_id,
            payload,
        });
        cursor += 16 + length;
    }
    Ok(deliveries)
}

pub fn read_heartbeats(path: &Path) -> Result<Vec<(u64, Vec<u8>)>> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let mut heartbeats = Vec::new();
    let mut cursor = 0_usize;
    while cursor < bytes.len() {
        let header = bytes
            .get(cursor..cursor + 12)
            .context("truncated heartbeat header")?;
        let tick = u64::from_le_bytes(header[0..8].try_into()?);
        let length = u32::from_le_bytes(header[8..12].try_into()?) as usize;
        let payload = bytes
            .get(cursor + 12..cursor + 12 + length)
            .context("truncated heartbeat payload")?
            .to_vec();
        heartbeats.push((tick, payload));
        cursor += 12 + length;
    }
    Ok(heartbeats)
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn read_delivery_records(records: &[(u64, u32, &[u8])]) -> Result<Vec<BagDelivery>> {
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rook-rmf-bag-{}-{sequence}.bin",
            std::process::id()
        ));
        let mut file = std::fs::File::create(&path)?;
        for (tick, channel, payload) in records {
            file.write_all(&tick.to_le_bytes())?;
            file.write_all(&channel.to_le_bytes())?;
            file.write_all(&(payload.len() as u32).to_le_bytes())?;
            file.write_all(payload)?;
        }
        drop(file);
        let result = read_deliveries(&path);
        std::fs::remove_file(path)?;
        result
    }

    #[test]
    fn replay_state_count_exposes_extra_state_hidden_by_full_raw_lcs() {
        let recorded_payloads = [b"state-a".as_slice(), b"state-c".as_slice()];
        let replayed_payloads = [
            b"state-a".as_slice(),
            b"state-b".as_slice(),
            b"state-c".as_slice(),
        ];
        let recorded_states = distinct_states(recorded_payloads.into_iter());
        let replayed_states = distinct_states(replayed_payloads.into_iter());
        let raw_lcs = lcs_len(&recorded_states, &replayed_states);

        assert_eq!(raw_lcs, recorded_states.len());
        assert!(replayed_states.len() > recorded_states.len());
    }

    #[test]
    fn lcs_counts_common_subsequence_not_prefix() {
        let a = [1, 2, 3, 4, 5];
        let b = [9, 1, 3, 5];
        assert_eq!(lcs_len(&a, &b), 3);
        assert_eq!(lcs_len(&a, &[]), 0);
    }

    fn delivery(tick: u64, channel_id: u32, participant: u64) -> BagDelivery {
        let mut payload = participant.to_le_bytes().to_vec();
        payload.extend_from_slice(&[0_u8; 16]);
        BagDelivery {
            tick,
            channel_id,
            payload,
        }
    }

    fn timer_delivery(tick: u64) -> BagDelivery {
        BagDelivery {
            tick,
            channel_id: CH_HEARTBEAT_TIMER,
            payload: Vec::new(),
        }
    }

    #[test]
    fn empty_heartbeat_timer_delivery_is_accepted() {
        let deliveries = read_delivery_records(&[(1, CH_HEARTBEAT_TIMER, &[])])
            .expect("empty timer delivery must be valid");
        assert_eq!(deliveries.len(), 1);
        assert!(deliveries[0].payload.is_empty());
    }

    #[test]
    fn nonempty_heartbeat_timer_delivery_is_rejected() {
        let error = read_delivery_records(&[(1, CH_HEARTBEAT_TIMER, &[0])])
            .err()
            .expect("timer delivery payload must be empty");
        assert_eq!(
            error.to_string(),
            "heartbeat timer delivery payload must be empty"
        );
    }

    #[test]
    fn short_non_timer_delivery_is_rejected() {
        for channel in CH_SET..=CH_CANCEL {
            let error = read_delivery_records(&[(1, channel, &[0; 7])])
                .err()
                .expect("channels 10-14 require a participant id");
            assert_eq!(
                error.to_string(),
                "delivery payload shorter than a participant id"
            );
        }
    }

    #[test]
    fn lifecycle_reorder_moves_racing_cancel_before_its_set() {
        let messages = vec![
            delivery(1, CH_SET, 7),
            delivery(2, CH_SET, 7),
            delivery(3, CH_CANCEL, 7),
        ];
        let (reordered, moved) = reorder_lifecycle(&messages);
        assert_eq!(moved, 1);
        let channels: Vec<u32> = reordered.iter().map(|m| m.channel_id).collect();
        assert_eq!(channels, vec![CH_SET, CH_CANCEL, CH_SET]);
        assert!(reordered.windows(2).all(|w| w[0].tick < w[1].tick));
    }

    #[test]
    fn lifecycle_reorder_skips_interleaved_timer_and_leaves_it_in_place() {
        let messages = vec![
            delivery(1, CH_SET, 7),
            timer_delivery(2),
            delivery(3, CH_SET, 7),
            delivery(4, CH_CANCEL, 7),
        ];
        let (reordered, moved) = reorder_lifecycle(&messages);
        assert_eq!(moved, 1);
        let channels: Vec<u32> = reordered.iter().map(|message| message.channel_id).collect();
        assert_eq!(
            channels,
            vec![CH_SET, CH_HEARTBEAT_TIMER, CH_CANCEL, CH_SET]
        );
        assert_eq!(reordered[1].tick, 2);
        assert!(reordered[1].payload.is_empty());
        assert!(
            reordered
                .windows(2)
                .all(|window| window[0].tick < window[1].tick)
        );
    }

    #[test]
    fn lifecycle_reorder_release_does_not_block_cancel_movement() {
        let messages = vec![
            delivery(1, CH_SET, 7),
            delivery(2, CH_RELEASE, 7),
            delivery(3, CH_CANCEL, 7),
        ];
        let (reordered, moved) = reorder_lifecycle(&messages);
        assert_eq!(moved, 1);
        let channels: Vec<u32> = reordered.iter().map(|m| m.channel_id).collect();
        assert_eq!(channels, vec![CH_CANCEL, CH_SET, CH_RELEASE]);
    }

    #[test]
    fn lifecycle_reorder_leaves_completed_iterations_alone() {
        // A reached between set and cancel means the order was real.
        let messages = vec![
            delivery(1, CH_SET, 7),
            delivery(2, CH_REACHED, 7),
            delivery(3, CH_CANCEL, 7),
        ];
        let (reordered, moved) = reorder_lifecycle(&messages);
        assert_eq!(moved, 0);
        let channels: Vec<u32> = reordered.iter().map(|m| m.channel_id).collect();
        assert_eq!(channels, vec![CH_SET, CH_REACHED, CH_CANCEL]);
    }
}
