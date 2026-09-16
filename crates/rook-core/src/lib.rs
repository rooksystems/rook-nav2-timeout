#![no_std]
#![forbid(unsafe_code)]

//! Deterministic replay primitives for experiment 1.
//!
//! This crate has no ambient capabilities: replay results depend only on the
//! supplied envelope bytes and the pinned hash domains below.

extern crate alloc;

use alloc::vec::Vec;

pub const ENVELOPE_SIZE: usize = 72;
pub const MAGIC: u32 = u32::from_le_bytes(*b"RKE1");
pub const VERSION: u16 = 1;
pub const CELL_ACTOR_ID: u32 = 1;
pub const SOURCE_ACTOR_ID: u32 = 2;
pub const OUTPUT_ACTOR_ID: u32 = 3;
pub const INPUT_CHANNEL_ID: u32 = 7;
pub const OUTPUT_CHANNEL_ID: u32 = 8;

const OUTPUT_DOMAIN: &[u8] = b"rook-output-v1";
const RUN_DOMAIN: &[u8] = b"rook-run-v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum EnvelopeKind {
    Deliver = 1,
    Emit = 2,
    Timer = 3,
    Gap = 4,
    Marker = 5,
}

/// The canonical event header hashed by every replay implementation.
///
/// Raw payloads stay outside this structure; `payload_blake3` commits their
/// exact bytes without depending on a payload codec.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Envelope {
    pub kind: EnvelopeKind,
    pub tick: u64,
    pub src_actor: u32,
    pub dst_actor: u32,
    pub channel_id: u32,
    pub payload_len: u32,
    pub src_seq: u64,
    pub payload_blake3: [u8; 32],
}

impl Envelope {
    /// Encodes the sole valid 72-byte wire representation.
    ///
    /// Every field is fixed-width and little-endian so host architecture and
    /// serializer versions cannot affect the hash path.
    pub fn canonical_bytes(&self) -> [u8; ENVELOPE_SIZE] {
        let mut bytes = [0_u8; ENVELOPE_SIZE];
        bytes[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[4..6].copy_from_slice(&VERSION.to_le_bytes());
        bytes[6..8].copy_from_slice(&(self.kind as u16).to_le_bytes());
        bytes[8..16].copy_from_slice(&self.tick.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.src_actor.to_le_bytes());
        bytes[20..24].copy_from_slice(&self.dst_actor.to_le_bytes());
        bytes[24..28].copy_from_slice(&self.channel_id.to_le_bytes());
        bytes[28..32].copy_from_slice(&self.payload_len.to_le_bytes());
        bytes[32..40].copy_from_slice(&self.src_seq.to_le_bytes());
        bytes[40..72].copy_from_slice(&self.payload_blake3);
        bytes
    }
}

/// Digests with deliberately different stability contracts.
///
/// `run_hash` commits every replay event. `output_digest` commits only cell
/// decisions and is therefore the golden that survives internal engine work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplayHashes {
    pub run_hash: [u8; 32],
    pub output_digest: [u8; 32],
}

pub fn generate_synthetic_deliveries(event_count: u32) -> Vec<Envelope> {
    let mut deliveries = Vec::with_capacity(event_count as usize);
    for event_index in 0..event_count {
        deliveries.push(synthetic_delivery(event_index));
    }
    deliveries
}

pub fn synthetic_payload(event_index: u32) -> [u8; 16] {
    let mut payload = [0_u8; 16];
    payload[0..4].copy_from_slice(&event_index.to_le_bytes());
    payload[4..8].copy_from_slice(&event_index.wrapping_mul(2_654_435_761).to_le_bytes());
    payload[8..16].copy_from_slice(&u64::from(event_index).wrapping_mul(1_000_003).to_le_bytes());
    payload
}

fn echo_and_count_payload(event_index: u32, count: u64) -> [u8; 24] {
    let mut output = [0_u8; 24];
    output[..16].copy_from_slice(&synthetic_payload(event_index));
    output[16..].copy_from_slice(&count.to_le_bytes());
    output
}

/// Replays the generated corpus without materializing it.
///
/// The Wasm core calls this path, so it mirrors `replay`'s tick ordering while
/// keeping the wrapper inside its fixed 64 MiB memory declaration.
pub fn replay_synthetic(event_count: u32) -> ReplayHashes {
    let trace_key = domain_key(b"rook-trace-v1/synthetic@1");
    let run_key = domain_key(RUN_DOMAIN);
    let output_key = domain_key(OUTPUT_DOMAIN);
    let mut trace_hash = [0_u8; 32];

    for event_index in 0..event_count {
        let delivery = synthetic_delivery(event_index);
        trace_hash = extend_chain(&trace_key, trace_hash, &delivery);
    }

    let mut run_hash = keyed_hash(&run_key, &trace_hash);
    let mut output_digest = [0_u8; 32];
    let mut tick_start = 0_u32;
    while tick_start < event_count {
        let tick = u64::from(tick_start / 4);
        let tick_end = event_count.min(tick_start.saturating_add(4));
        for event_index in tick_start..tick_end {
            run_hash = extend_chain(&run_key, run_hash, &synthetic_delivery(event_index));
        }
        for event_index in tick_start..tick_end {
            let emitted = synthetic_emit(event_index, tick);
            run_hash = extend_chain(&run_key, run_hash, &emitted);
            output_digest = extend_chain(&output_key, output_digest, &emitted);
        }
        tick_start = tick_end;
    }

    ReplayHashes {
        run_hash,
        output_digest,
    }
}

fn synthetic_delivery(event_index: u32) -> Envelope {
    let payload = synthetic_payload(event_index);
    Envelope {
        kind: EnvelopeKind::Deliver,
        tick: u64::from(event_index / 4),
        src_actor: SOURCE_ACTOR_ID,
        dst_actor: CELL_ACTOR_ID,
        channel_id: INPUT_CHANNEL_ID,
        payload_len: payload.len() as u32,
        src_seq: u64::from(event_index),
        payload_blake3: *blake3::hash(&payload).as_bytes(),
    }
}

fn synthetic_emit(event_index: u32, tick: u64) -> Envelope {
    let payload = echo_and_count_payload(event_index, u64::from(event_index) + 1);
    Envelope {
        kind: EnvelopeKind::Emit,
        tick,
        src_actor: CELL_ACTOR_ID,
        dst_actor: OUTPUT_ACTOR_ID,
        channel_id: OUTPUT_CHANNEL_ID,
        payload_len: payload.len() as u32,
        src_seq: u64::from(event_index),
        payload_blake3: *blake3::hash(&payload).as_bytes(),
    }
}

/// Refusals for inputs that cannot have one architecture-independent order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplayError {
    DuplicateSchedulingKey,
    TooManyDeliveries,
    EmitOnDeadTick,
    NonMonotonicEmitSequence,
}

/// Runs the single-cell discrete-event scheduler.
///
/// Deliveries are ordered only by the settled scheduling key. A duplicate key
/// is refused rather than silently adding a new tie-break rule. Within each
/// tick all deliveries enter the run chain before any cell emits.
pub fn replay(deliveries: &[Envelope]) -> Result<ReplayHashes, ReplayError> {
    if deliveries.len() > u32::MAX as usize {
        return Err(ReplayError::TooManyDeliveries);
    }

    let mut ordered_deliveries = deliveries.to_vec();
    ordered_deliveries.sort_by_key(scheduling_key);
    if ordered_deliveries
        .windows(2)
        .any(|pair| scheduling_key(&pair[0]) == scheduling_key(&pair[1]))
    {
        return Err(ReplayError::DuplicateSchedulingKey);
    }

    let trace_hash = chain_hash(
        &domain_key(b"rook-trace-v1/synthetic@1"),
        [0_u8; 32],
        &ordered_deliveries,
    );
    let run_key = domain_key(RUN_DOMAIN);
    let output_key = domain_key(OUTPUT_DOMAIN);
    let mut run_hash = keyed_hash(&run_key, &trace_hash);
    let mut output_digest = [0_u8; 32];
    let mut emitted_count = 0_u64;

    let mut tick_start = 0_usize;
    while tick_start < ordered_deliveries.len() {
        let tick = ordered_deliveries[tick_start].tick;
        let mut tick_end = tick_start + 1;
        while tick_end < ordered_deliveries.len() && ordered_deliveries[tick_end].tick == tick {
            tick_end += 1;
        }
        for delivery in &ordered_deliveries[tick_start..tick_end] {
            run_hash = extend_chain(&run_key, run_hash, delivery);
        }
        for delivery in &ordered_deliveries[tick_start..tick_end] {
            emitted_count = emitted_count
                .checked_add(1)
                .expect("synthetic event count fits u64");
            let event_index = (emitted_count - 1) as u32;
            let payload = echo_and_count_payload(event_index, emitted_count);
            let emitted = Envelope {
                kind: EnvelopeKind::Emit,
                tick: delivery.tick,
                src_actor: CELL_ACTOR_ID,
                dst_actor: OUTPUT_ACTOR_ID,
                channel_id: OUTPUT_CHANNEL_ID,
                payload_len: payload.len() as u32,
                src_seq: emitted_count - 1,
                payload_blake3: *blake3::hash(&payload).as_bytes(),
            };
            run_hash = extend_chain(&run_key, run_hash, &emitted);
            output_digest = extend_chain(&output_key, output_digest, &emitted);
        }
        tick_start = tick_end;
    }

    Ok(ReplayHashes {
        run_hash,
        output_digest,
    })
}

/// Hashes an observed run from its recorded deliveries and ABI-captured emits.
///
/// It uses `replay`'s chain order. Deliveries sort by scheduling key. At each
/// live tick, the chain includes all deliveries followed by that tick's emits
/// in emit order. `trace_domain` names the corpus. Emits must land on live
/// ticks and have strictly increasing `src_seq`; the host enforces both while
/// capturing them at `emit`.
pub fn hash_observed(
    trace_domain: &[u8],
    deliveries: &[Envelope],
    emits: &[Envelope],
) -> Result<ReplayHashes, ReplayError> {
    if deliveries.len() > u32::MAX as usize {
        return Err(ReplayError::TooManyDeliveries);
    }
    let mut ordered_deliveries = deliveries.to_vec();
    ordered_deliveries.sort_by_key(scheduling_key);
    if ordered_deliveries
        .windows(2)
        .any(|pair| scheduling_key(&pair[0]) == scheduling_key(&pair[1]))
    {
        return Err(ReplayError::DuplicateSchedulingKey);
    }
    if emits
        .windows(2)
        .any(|pair| pair[0].src_seq >= pair[1].src_seq || pair[0].tick > pair[1].tick)
    {
        return Err(ReplayError::NonMonotonicEmitSequence);
    }

    let trace_hash = chain_hash(&domain_key(trace_domain), [0_u8; 32], &ordered_deliveries);
    let run_key = domain_key(RUN_DOMAIN);
    let output_key = domain_key(OUTPUT_DOMAIN);
    let mut run_hash = keyed_hash(&run_key, &trace_hash);
    let mut output_digest = [0_u8; 32];

    let mut emit_index = 0_usize;
    let mut tick_start = 0_usize;
    while tick_start < ordered_deliveries.len() {
        let tick = ordered_deliveries[tick_start].tick;
        let mut tick_end = tick_start + 1;
        while tick_end < ordered_deliveries.len() && ordered_deliveries[tick_end].tick == tick {
            tick_end += 1;
        }
        for delivery in &ordered_deliveries[tick_start..tick_end] {
            run_hash = extend_chain(&run_key, run_hash, delivery);
        }
        while emit_index < emits.len() && emits[emit_index].tick == tick {
            let emitted = &emits[emit_index];
            run_hash = extend_chain(&run_key, run_hash, emitted);
            output_digest = extend_chain(&output_key, output_digest, emitted);
            emit_index += 1;
        }
        if emit_index < emits.len() && emits[emit_index].tick < tick {
            return Err(ReplayError::EmitOnDeadTick);
        }
        tick_start = tick_end;
    }
    if emit_index != emits.len() {
        return Err(ReplayError::EmitOnDeadTick);
    }

    Ok(ReplayHashes {
        run_hash,
        output_digest,
    })
}

/// The settled delivery order: tick, then destination, channel, source, sequence.
pub fn scheduling_key(envelope: &Envelope) -> (u64, u32, u32, u32, u64) {
    (
        envelope.tick,
        envelope.dst_actor,
        envelope.channel_id,
        envelope.src_actor,
        envelope.src_seq,
    )
}

fn chain_hash(key: &[u8; 32], initial: [u8; 32], envelopes: &[Envelope]) -> [u8; 32] {
    envelopes.iter().fold(initial, |chain, envelope| {
        extend_chain(key, chain, envelope)
    })
}

/// Commits both the prior chain state and the next canonical envelope.
/// Omitting either would permit event replacement or reordering.
fn extend_chain(key: &[u8; 32], previous: [u8; 32], envelope: &Envelope) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_keyed(key);
    hasher.update(&previous);
    hasher.update(&envelope.canonical_bytes());
    *hasher.finalize().as_bytes()
}

/// Expands a readable domain name into BLAKE3's required 32-byte key.
/// Using distinct keys prevents a digest from one domain being accepted in another.
fn domain_key(domain: &[u8]) -> [u8; 32] {
    *blake3::hash(domain).as_bytes()
}

fn keyed_hash(key: &[u8; 32], bytes: &[u8]) -> [u8; 32] {
    *blake3::keyed_hash(key, bytes).as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_envelope_is_fixed_width_little_endian() {
        let envelope = generate_synthetic_deliveries(1)[0];
        let bytes = envelope.canonical_bytes();
        assert_eq!(bytes.len(), ENVELOPE_SIZE);
        assert_eq!(&bytes[0..4], b"RKE1");
        assert_eq!(&bytes[4..6], &VERSION.to_le_bytes());
        assert_eq!(&bytes[8..16], &0_u64.to_le_bytes());
    }

    #[test]
    fn replay_is_stable_when_input_arrival_order_changes() {
        let ordered = generate_synthetic_deliveries(20);
        let mut reversed = ordered.clone();
        reversed.reverse();
        assert_eq!(replay(&ordered), replay(&reversed));
    }

    #[test]
    fn hash_observed_matches_replay_on_the_experiment_1_corpus() {
        let deliveries = generate_synthetic_deliveries(21);
        let emits: Vec<Envelope> = deliveries
            .iter()
            .enumerate()
            .map(|(event_index, delivery)| {
                let payload = echo_and_count_payload(event_index as u32, event_index as u64 + 1);
                Envelope {
                    kind: EnvelopeKind::Emit,
                    tick: delivery.tick,
                    src_actor: CELL_ACTOR_ID,
                    dst_actor: OUTPUT_ACTOR_ID,
                    channel_id: OUTPUT_CHANNEL_ID,
                    payload_len: payload.len() as u32,
                    src_seq: event_index as u64,
                    payload_blake3: *blake3::hash(&payload).as_bytes(),
                }
            })
            .collect();
        let observed = hash_observed(b"rook-trace-v1/synthetic@1", &deliveries, &emits);
        assert_eq!(observed, replay(&deliveries));
    }

    #[test]
    fn hash_observed_rejects_emits_on_ticks_without_deliveries() {
        let deliveries = generate_synthetic_deliveries(4);
        let mut stray = deliveries[0];
        stray.kind = EnvelopeKind::Emit;
        stray.tick = 7;
        assert_eq!(
            hash_observed(b"rook-trace-v1/synthetic@1", &deliveries, &[stray]),
            Err(ReplayError::EmitOnDeadTick)
        );
    }

    #[test]
    fn replay_rejects_duplicate_scheduling_keys() {
        let delivery = generate_synthetic_deliveries(1)[0];
        let mut duplicate = delivery;
        duplicate.payload_blake3[0] ^= 1;

        assert_eq!(
            replay(&[delivery, duplicate]),
            Err(ReplayError::DuplicateSchedulingKey)
        );
    }
}
