#![no_std]
#![forbid(unsafe_code)]

//! Native stream v1: framing, ordering and hashing for recordings of native
//! components. `docs/internals/format-spec.md` is normative; this crate
//! implements it and never the other way round.
//!
//! The stream reuses rook-core's 72-byte envelope under envelope version 2
//! so a native recording can never be read as a Wasm-cell recording. Order
//! is the event ordinal, carried in the envelope's tick field, and the hash
//! chains walk frames in ordinal order, so an input, its effect, a second
//! input and its effect hash differently from the same events grouped as
//! inputs then effects. Bodies are opaque here; only the three markers this
//! crate must check (session, end, gap) are decoded.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

pub use rook_core::EnvelopeKind;
use rook_core::{ENVELOPE_SIZE, MAGIC};

/// Envelope bytes 4..6. Version 1 is the Wasm-cell envelope (rook-core).
pub const NATIVE_ENVELOPE_VERSION: u16 = 2;
/// Carried in the session marker body; bumps when a body layout changes.
pub const STREAM_VERSION: u16 = 1;
pub const OBSERVED_SIZE: usize = 8;
pub const FRAME_HEADER_SIZE: usize = ENVELOPE_SIZE + OBSERVED_SIZE;
pub const EVENT_HEADER_SIZE: usize = 8;
pub const SESSION_ACTOR_ID: u32 = 0;
pub const COMPONENT_ACTOR_ID: u32 = 1;
pub const SESSION_CHANNEL_ID: u32 = 0;

pub const FLAG_ATTEMPTED: u32 = 1;
pub const FLAG_CONFIRMED: u32 = 2;
pub const FLAG_COUNT_UNKNOWN: u32 = 4;
/// Every other flag bit is reserved and must be zero.
pub const FLAGS_DEFINED: u32 = FLAG_ATTEMPTED | FLAG_CONFIRMED | FLAG_COUNT_UNKNOWN;
/// The only event schema this stream version defines.
pub const EVENT_SCHEMA: u16 = 1;

const TRACE_DOMAIN_PREFIX: &[u8] = b"rook-native-trace-v1/";
const RUN_DOMAIN: &[u8] = b"rook-native-run-v1";
const OUTPUT_DOMAIN: &[u8] = b"rook-native-output-v1";

/// Every native event type and the envelope kind it must carry.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
#[repr(u16)]
pub enum EventType {
    Session = 0x0001,
    End = 0x0002,
    Gap = 0x0003,
    StartingState = 0x0004,
    Lifecycle = 0x0005,
    Message = 0x0010,
    Clock = 0x0011,
    Timer = 0x0012,
    ServiceResponse = 0x0013,
    GoalResponse = 0x0014,
    Feedback = 0x0015,
    Result = 0x0016,
    CancelResponse = 0x0017,
    ServiceRequest = 0x0018,
    Publish = 0x0020,
    ServiceCall = 0x0021,
    GoalSend = 0x0022,
    CancelSend = 0x0023,
    ServiceResponseSend = 0x0024,
    LifecycleResult = 0x0025,
    Status = 0x0026,
    TimerControl = 0x0027,
}

impl EventType {
    pub fn from_u16(value: u16) -> Option<Self> {
        use EventType::*;
        const ALL: [EventType; 22] = [
            Session,
            End,
            Gap,
            StartingState,
            Lifecycle,
            Message,
            Clock,
            Timer,
            ServiceResponse,
            GoalResponse,
            Feedback,
            Result,
            CancelResponse,
            ServiceRequest,
            Publish,
            ServiceCall,
            GoalSend,
            CancelSend,
            ServiceResponseSend,
            LifecycleResult,
            Status,
            TimerControl,
        ];
        ALL.iter().copied().find(|event| *event as u16 == value)
    }

    /// The only envelope kind a frame of this type may carry.
    pub fn kind(self) -> EnvelopeKind {
        use EventType::*;
        match self {
            Session | End | StartingState => EnvelopeKind::Marker,
            Gap => EnvelopeKind::Gap,
            Timer => EnvelopeKind::Timer,
            Lifecycle | Message | Clock | ServiceResponse | GoalResponse | Feedback | Result
            | CancelResponse | ServiceRequest => EnvelopeKind::Deliver,
            Publish | ServiceCall | GoalSend | CancelSend | ServiceResponseSend
            | LifecycleResult | Status | TimerControl => EnvelopeKind::Emit,
        }
    }
}

/// The first eight payload bytes of every native frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventHeader {
    pub event_type: EventType,
    pub schema: u16,
    pub flags: u32,
}

impl EventHeader {
    pub fn encode(&self) -> [u8; EVENT_HEADER_SIZE] {
        let mut bytes = [0_u8; EVENT_HEADER_SIZE];
        bytes[0..2].copy_from_slice(&(self.event_type as u16).to_le_bytes());
        bytes[2..4].copy_from_slice(&self.schema.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.flags.to_le_bytes());
        bytes
    }
}

/// One frame: the envelope fields the native stream assigns, the observed
/// recorder time that sits outside every chain hash, and the payload split
/// into its header and opaque body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    pub ordinal: u64,
    pub src_actor: u32,
    pub dst_actor: u32,
    pub channel_id: u32,
    pub src_seq: u64,
    pub observed_ns: u64,
    pub header: EventHeader,
    pub body: Vec<u8>,
}

impl Frame {
    pub fn kind(&self) -> EnvelopeKind {
        self.header.event_type.kind()
    }

    pub fn payload(&self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(EVENT_HEADER_SIZE + self.body.len());
        payload.extend_from_slice(&self.header.encode());
        payload.extend_from_slice(&self.body);
        payload
    }

    /// The 72-byte envelope every chain hashes. Same layout as rook-core's
    /// `Envelope::canonical_bytes`, version 2, ordinal in the tick field.
    pub fn envelope_bytes(&self) -> [u8; ENVELOPE_SIZE] {
        let payload = self.payload();
        let mut bytes = [0_u8; ENVELOPE_SIZE];
        bytes[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[4..6].copy_from_slice(&NATIVE_ENVELOPE_VERSION.to_le_bytes());
        bytes[6..8].copy_from_slice(&(self.kind() as u16).to_le_bytes());
        bytes[8..16].copy_from_slice(&self.ordinal.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.src_actor.to_le_bytes());
        bytes[20..24].copy_from_slice(&self.dst_actor.to_le_bytes());
        bytes[24..28].copy_from_slice(&self.channel_id.to_le_bytes());
        bytes[28..32].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes[32..40].copy_from_slice(&self.src_seq.to_le_bytes());
        bytes[40..72].copy_from_slice(blake3::hash(&payload).as_bytes());
        bytes
    }

    /// Appends envelope, observed time and payload: the frame's only wire form.
    pub fn encode_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.envelope_bytes());
        out.extend_from_slice(&self.observed_ns.to_le_bytes());
        out.extend_from_slice(&self.payload());
    }
}

pub fn encode_stream(frames: &[Frame]) -> Vec<u8> {
    let mut out = Vec::new();
    for frame in frames {
        frame.encode_into(&mut out);
    }
    out
}

/// Body of the session marker at ordinal zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Session {
    pub uuid: [u8; 16],
    pub record_utc_ns: u64,
    pub pid: u32,
}

impl Session {
    pub const BODY_SIZE: usize = 30;

    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(Self::BODY_SIZE);
        body.extend_from_slice(&STREAM_VERSION.to_le_bytes());
        body.extend_from_slice(&self.uuid);
        body.extend_from_slice(&self.record_utc_ns.to_le_bytes());
        body.extend_from_slice(&self.pid.to_le_bytes());
        body
    }

    pub fn decode(body: &[u8]) -> Result<Self, StreamError> {
        if body.len() != Self::BODY_SIZE {
            return Err(StreamError::MalformedMarker { ordinal: 0 });
        }
        let version = u16::from_le_bytes([body[0], body[1]]);
        if version != STREAM_VERSION {
            return Err(StreamError::UnsupportedStreamVersion { found: version });
        }
        let mut uuid = [0_u8; 16];
        uuid.copy_from_slice(&body[2..18]);
        Ok(Session {
            uuid,
            record_utc_ns: u64::from_le_bytes(body[18..26].try_into().unwrap()),
            pid: u32::from_le_bytes(body[26..30].try_into().unwrap()),
        })
    }
}

/// Body of the end marker, the only end-of-recording evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct End {
    pub uuid: [u8; 16],
    pub frame_count: u64,
}

impl End {
    pub const BODY_SIZE: usize = 24;

    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(Self::BODY_SIZE);
        body.extend_from_slice(&self.uuid);
        body.extend_from_slice(&self.frame_count.to_le_bytes());
        body
    }

    pub fn decode(ordinal: u64, body: &[u8]) -> Result<Self, StreamError> {
        if body.len() != Self::BODY_SIZE {
            return Err(StreamError::MalformedMarker { ordinal });
        }
        let mut uuid = [0_u8; 16];
        uuid.copy_from_slice(&body[0..16]);
        Ok(End {
            uuid,
            frame_count: u64::from_le_bytes(body[16..24].try_into().unwrap()),
        })
    }
}

/// Body of a gap frame: the recorder admits it lost events here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Gap {
    pub reason: u16,
    /// `None` when the recorder cannot count what it lost.
    pub lost: Option<u64>,
    pub detail: Vec<u8>,
}

pub const GAP_REASON_OVERRUN: u16 = 1;
pub const GAP_REASON_UNSUPPORTED: u16 = 2;
pub const GAP_REASON_RESTART: u16 = 3;

impl Gap {
    /// `None` when the detail does not fit its u16 length prefix; a
    /// recorder must shorten it rather than emit an undecodable frame.
    pub fn encode(&self) -> Option<Vec<u8>> {
        let detail_len = u16::try_from(self.detail.len()).ok()?;
        let mut body = Vec::with_capacity(12 + self.detail.len());
        body.extend_from_slice(&self.reason.to_le_bytes());
        body.extend_from_slice(&self.lost.unwrap_or(u64::MAX).to_le_bytes());
        body.extend_from_slice(&detail_len.to_le_bytes());
        body.extend_from_slice(&self.detail);
        Some(body)
    }

    pub fn flags(&self) -> u32 {
        if self.lost.is_none() {
            FLAG_COUNT_UNKNOWN
        } else {
            0
        }
    }

    pub fn decode(ordinal: u64, body: &[u8]) -> Result<Self, StreamError> {
        if body.len() < 12 {
            return Err(StreamError::MalformedMarker { ordinal });
        }
        let reason = u16::from_le_bytes([body[0], body[1]]);
        let lost = u64::from_le_bytes(body[2..10].try_into().unwrap());
        let detail_len = u16::from_le_bytes([body[10], body[11]]) as usize;
        if body.len() != 12 + detail_len {
            return Err(StreamError::MalformedMarker { ordinal });
        }
        Ok(Gap {
            reason,
            lost: (lost != u64::MAX).then_some(lost),
            detail: body[12..].to_vec(),
        })
    }
}

/// Refusals. Every one names the first offending offset or ordinal so a
/// report can point at it; none of them is ever a shorter successful replay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamError {
    /// The file ends inside a frame. A recording without its end marker is
    /// `MissingEnd`; this is a torn frame.
    Truncated {
        offset: usize,
    },
    BadMagic {
        ordinal: u64,
    },
    /// Version 1 is a Wasm-cell envelope stream and is never a native one.
    WrongEnvelopeVersion {
        ordinal: u64,
        found: u16,
    },
    UnknownKind {
        ordinal: u64,
        found: u16,
    },
    UnknownEventType {
        ordinal: u64,
        found: u16,
    },
    KindMismatch {
        ordinal: u64,
    },
    PayloadTooShort {
        ordinal: u64,
    },
    PayloadHashMismatch {
        ordinal: u64,
    },
    UnsupportedSchema {
        ordinal: u64,
        found: u16,
    },
    /// The body does not have the layout `format-spec.md` fixes for its
    /// event type: wrong length, or an enumerated field out of range.
    MalformedBody {
        ordinal: u64,
        event_type: EventType,
    },
    ReservedFlags {
        ordinal: u64,
        found: u32,
    },
    NonContiguousOrdinal {
        expected: u64,
        found: u64,
    },
    SequenceGap {
        ordinal: u64,
        expected: u64,
        found: u64,
    },
    MissingSession,
    UnexpectedSession {
        ordinal: u64,
    },
    UnsupportedStreamVersion {
        found: u16,
    },
    MissingEnd,
    UnexpectedEnd {
        ordinal: u64,
    },
    EndMismatch {
        ordinal: u64,
    },
    MalformedMarker {
        ordinal: u64,
    },
}

impl core::fmt::Display for StreamError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl core::error::Error for StreamError {}

/// Splits a payload into its event header and body, refusing an unknown
/// type, an unsupported schema, reserved flags, and a body that does not
/// fit its type's layout. Used by the stream decoder and by any reader of
/// payloads stored outside a stream, such as a capsule's captured effects.
pub fn decode_payload(ordinal: u64, payload: &[u8]) -> Result<(EventHeader, &[u8]), StreamError> {
    if payload.len() < EVENT_HEADER_SIZE {
        return Err(StreamError::PayloadTooShort { ordinal });
    }
    let event_type = u16::from_le_bytes([payload[0], payload[1]]);
    let event_type = EventType::from_u16(event_type).ok_or(StreamError::UnknownEventType {
        ordinal,
        found: event_type,
    })?;
    let schema = u16::from_le_bytes([payload[2], payload[3]]);
    if schema != EVENT_SCHEMA {
        return Err(StreamError::UnsupportedSchema {
            ordinal,
            found: schema,
        });
    }
    let flags = u32::from_le_bytes(payload[4..8].try_into().unwrap());
    if flags & !FLAGS_DEFINED != 0 {
        return Err(StreamError::ReservedFlags {
            ordinal,
            found: flags,
        });
    }
    let body = &payload[EVENT_HEADER_SIZE..];
    if !body_fits(event_type, body) {
        return Err(StreamError::MalformedBody {
            ordinal,
            event_type,
        });
    }
    Ok((
        EventHeader {
            event_type,
            schema,
            flags,
        },
        body,
    ))
}

/// The layout table of `format-spec.md` section 2.4: fixed sizes, minimum
/// sizes for types that end in bytes, and the enumerated fields' ranges.
fn body_fits(event_type: EventType, body: &[u8]) -> bool {
    use EventType::*;
    let in_range = |index: usize, low: u8, high: u8| body[index] >= low && body[index] <= high;
    let u16_at = |index: usize| u16::from_le_bytes([body[index], body[index + 1]]);
    match event_type {
        Session => body.len() == 30,
        End => body.len() == 24,
        Gap => body.len() >= 12 && body.len() == 12 + u16_at(10) as usize,
        StartingState => body.len() == 34 && (1..=3).contains(&u16_at(0)),
        Lifecycle => body.len() == 9,
        Message => body.len() >= 2 && (1..=3).contains(&u16_at(0)),
        Clock => body.len() == 18 && (1..=3).contains(&u16_at(0)),
        Timer => body.len() == 27 && (1..=3).contains(&u16_at(4)) && in_range(22, 1, 3),
        ServiceResponse => body.len() >= 12,
        GoalResponse => body.len() == 33,
        Feedback => body.len() >= 24,
        Result => body.len() >= 25,
        CancelResponse => body.len() >= 11 && body.len() == 11 + 16 * u16_at(9) as usize,
        ServiceRequest => body.len() >= 8,
        Publish => body.len() >= 6 && (1..=3).contains(&u16_at(4)),
        ServiceCall => body.len() >= 8,
        GoalSend => body.len() >= 16,
        CancelSend => body.len() == 24,
        ServiceResponseSend => body.len() >= 8,
        LifecycleResult => body.len() == 2,
        Status => true,
        TimerControl => body.len() == 5 && in_range(4, 1, 2),
    }
}

/// Decodes and checks a whole stream. Ordinals must run 0, 1, 2, ... in
/// file order; `src_seq` must run 0, 1, 2, ... per (source, channel); the
/// first frame must be the session marker and the last the end marker whose
/// uuid and count match. Gaps are accepted and left for the reader to judge.
pub fn decode_stream(bytes: &[u8]) -> Result<Vec<Frame>, StreamError> {
    let mut frames = Vec::new();
    let mut sequences: BTreeMap<(u32, u32), u64> = BTreeMap::new();
    let mut session: Option<Session> = None;
    let mut ended = false;
    let mut cursor = 0_usize;
    while cursor < bytes.len() {
        if ended {
            return Err(StreamError::UnexpectedEnd {
                ordinal: frames.len() as u64 - 1,
            });
        }
        let expected_ordinal = frames.len() as u64;
        let header = bytes
            .get(cursor..cursor + FRAME_HEADER_SIZE)
            .ok_or(StreamError::Truncated { offset: cursor })?;
        if u32::from_le_bytes(header[0..4].try_into().unwrap()) != MAGIC {
            return Err(StreamError::BadMagic {
                ordinal: expected_ordinal,
            });
        }
        let version = u16::from_le_bytes([header[4], header[5]]);
        if version != NATIVE_ENVELOPE_VERSION {
            return Err(StreamError::WrongEnvelopeVersion {
                ordinal: expected_ordinal,
                found: version,
            });
        }
        let kind_value = u16::from_le_bytes([header[6], header[7]]);
        let kind = match kind_value {
            1 => EnvelopeKind::Deliver,
            2 => EnvelopeKind::Emit,
            3 => EnvelopeKind::Timer,
            4 => EnvelopeKind::Gap,
            5 => EnvelopeKind::Marker,
            found => {
                return Err(StreamError::UnknownKind {
                    ordinal: expected_ordinal,
                    found,
                });
            }
        };
        let ordinal = u64::from_le_bytes(header[8..16].try_into().unwrap());
        if ordinal != expected_ordinal {
            return Err(StreamError::NonContiguousOrdinal {
                expected: expected_ordinal,
                found: ordinal,
            });
        }
        let src_actor = u32::from_le_bytes(header[16..20].try_into().unwrap());
        let dst_actor = u32::from_le_bytes(header[20..24].try_into().unwrap());
        let channel_id = u32::from_le_bytes(header[24..28].try_into().unwrap());
        let payload_len = u32::from_le_bytes(header[28..32].try_into().unwrap()) as usize;
        let src_seq = u64::from_le_bytes(header[32..40].try_into().unwrap());
        let payload_blake3: [u8; 32] = header[40..72].try_into().unwrap();
        let observed_ns = u64::from_le_bytes(header[72..80].try_into().unwrap());
        let payload_start = cursor + FRAME_HEADER_SIZE;
        let payload_end = payload_start
            .checked_add(payload_len)
            .ok_or(StreamError::Truncated { offset: cursor })?;
        let payload = bytes
            .get(payload_start..payload_end)
            .ok_or(StreamError::Truncated { offset: cursor })?;
        if blake3::hash(payload).as_bytes() != &payload_blake3 {
            return Err(StreamError::PayloadHashMismatch { ordinal });
        }
        let (header, body) = decode_payload(ordinal, payload)?;
        let event_type = header.event_type;
        let flags = header.flags;
        if event_type.kind() != kind {
            return Err(StreamError::KindMismatch { ordinal });
        }

        let expected_seq = sequences.entry((src_actor, channel_id)).or_insert(0);
        if src_seq != *expected_seq {
            return Err(StreamError::SequenceGap {
                ordinal,
                expected: *expected_seq,
                found: src_seq,
            });
        }
        *expected_seq += 1;

        // Markers and gaps come from the session on the session channel;
        // the starting state is the one marker addressed to the component.
        let session_tuple = (SESSION_ACTOR_ID, SESSION_ACTOR_ID, SESSION_CHANNEL_ID);
        let expected_tuple = match event_type {
            EventType::Session | EventType::End | EventType::Gap => Some(session_tuple),
            EventType::StartingState => {
                Some((SESSION_ACTOR_ID, COMPONENT_ACTOR_ID, SESSION_CHANNEL_ID))
            }
            _ => None,
        };
        if expected_tuple.is_some_and(|tuple| tuple != (src_actor, dst_actor, channel_id)) {
            return Err(StreamError::MalformedMarker { ordinal });
        }
        match event_type {
            EventType::Session => {
                if ordinal != 0 {
                    return Err(StreamError::UnexpectedSession { ordinal });
                }
                session = Some(Session::decode(body)?);
            }
            EventType::End => {
                let end = End::decode(ordinal, body)?;
                let Some(session) = session else {
                    return Err(StreamError::MissingSession);
                };
                if end.uuid != session.uuid || end.frame_count != ordinal {
                    return Err(StreamError::EndMismatch { ordinal });
                }
                ended = true;
            }
            EventType::Gap => {
                let gap = Gap::decode(ordinal, body)?;
                if (flags & FLAG_COUNT_UNKNOWN != 0) != gap.lost.is_none() {
                    return Err(StreamError::MalformedMarker { ordinal });
                }
            }
            _ => {
                if session.is_none() {
                    return Err(StreamError::MissingSession);
                }
            }
        }

        frames.push(Frame {
            ordinal,
            src_actor,
            dst_actor,
            channel_id,
            src_seq,
            observed_ns,
            header,
            body: body.to_vec(),
        });
        cursor = payload_start + payload_len;
    }
    if session.is_none() {
        return Err(StreamError::MissingSession);
    }
    if !ended {
        return Err(StreamError::MissingEnd);
    }
    Ok(frames)
}

/// The three normalized execution hashes of a native stream. None of them
/// sees `observed_ns`; the raw record hash does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeHashes {
    /// Every non-effect frame in ordinal order under the corpus domain.
    pub trace_hash: [u8; 32],
    /// Seeded from the trace hash, then every frame in ordinal order, so
    /// the interleaving of inputs and effects is committed.
    pub run_hash: [u8; 32],
    /// Only effect frames, in ordinal order, with their ordinals.
    pub output_digest: [u8; 32],
}

/// `corpus` names the recording family, as in `rook-trace-v1/<corpus>` on
/// the Wasm path; it is appended to `rook-native-trace-v1/`.
pub fn hash_frames(corpus: &[u8], frames: &[Frame]) -> NativeHashes {
    let mut trace_domain = Vec::with_capacity(TRACE_DOMAIN_PREFIX.len() + corpus.len());
    trace_domain.extend_from_slice(TRACE_DOMAIN_PREFIX);
    trace_domain.extend_from_slice(corpus);
    let trace_key = domain_key(&trace_domain);
    let run_key = domain_key(RUN_DOMAIN);
    let output_key = domain_key(OUTPUT_DOMAIN);

    let mut trace_hash = [0_u8; 32];
    for frame in frames
        .iter()
        .filter(|frame| frame.kind() != EnvelopeKind::Emit)
    {
        trace_hash = extend_chain(&trace_key, trace_hash, frame);
    }
    let mut run_hash = *blake3::keyed_hash(&run_key, &trace_hash).as_bytes();
    let mut output_digest = [0_u8; 32];
    for frame in frames {
        run_hash = extend_chain(&run_key, run_hash, frame);
        if frame.kind() == EnvelopeKind::Emit {
            output_digest = extend_chain(&output_key, output_digest, frame);
        }
    }
    NativeHashes {
        trace_hash,
        run_hash,
        output_digest,
    }
}

/// Plain BLAKE3 of the stream bytes, observed times included. Reported as
/// the raw record hash, never mixed with the execution hashes above.
pub fn raw_record_hash(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

fn extend_chain(key: &[u8; 32], previous: [u8; 32], frame: &Frame) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_keyed(key);
    hasher.update(&previous);
    hasher.update(&frame.envelope_bytes());
    *hasher.finalize().as_bytes()
}

fn domain_key(domain: &[u8]) -> [u8; 32] {
    *blake3::hash(domain).as_bytes()
}
