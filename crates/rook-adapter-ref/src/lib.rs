//! The reference adapter: a tiny goal-client component with four variants,
//! the ndjson adapter protocol around it, a scripted environment that
//! records the reference fixtures, a driver that drives any adapter over
//! stdio, and the two reference properties. No ROS anywhere. The contract
//! it demonstrates is docs/internals/native-contract.md; #18's runner and
//! #20's Nav2 adapter implement against the same messages.

pub mod adapter;
pub mod bodies;
pub mod component;
pub mod driver;
pub mod fixture;
pub mod property;
pub mod protocol;

pub const ADAPTER_NAME: &str = "rook-adapter-ref";
pub const PROTOCOL_VERSION: u32 = 1;
pub const CORPUS: &str = "adapter-ref@1";

/// Endpoint actors. 0 and 1 are fixed by the contract; the rest are this
/// adapter's declared endpoint table.
pub const ACTOR_CLOCK: u32 = 2;
pub const ACTOR_ACTION_SERVER: u32 = 3;
pub const ACTOR_COMMAND: u32 = 4;
pub const ACTOR_DIAGNOSTICS: u32 = 5;

/// Channels. Inputs below 20, effects from 20.
pub const CH_GOAL_COMMAND: u32 = 10;
pub const CH_CLOCK: u32 = 11;
pub const CH_ACTION_RESPONSE: u32 = 12;
pub const CH_ACTION_GOAL: u32 = 20;
pub const CH_ACTION_CANCEL: u32 = 21;
pub const CH_STATUS: u32 = 22;

pub const CLOCK_ROS: u16 = 1;
pub const GOAL_RESPONSE_DEADLINE_NS: i64 = 1_000_000_000;

pub fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn decode_hex(text: &str) -> anyhow::Result<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) || !bytes.iter().all(u8::is_ascii_hexdigit) {
        anyhow::bail!("body_hex is not an even-length string of hex digits");
    }
    Ok(bytes
        .chunks(2)
        .map(|pair| u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect())
}

pub fn event_type_name(event_type: rook_native::EventType) -> String {
    format!("{event_type:?}")
}

pub fn parse_event_type(name: &str) -> Option<rook_native::EventType> {
    (1_u16..=0x27)
        .filter_map(rook_native::EventType::from_u16)
        .find(|event_type| event_type_name(*event_type) == name)
}

/// Content identities of the programs whose behavior a case pins: the
/// component and adapter, the property program and the normalizer. They
/// are BLAKE3 over the source embedded in this binary, so the identity a
/// fixture records is checkable from public source. Binary identity is the
/// runner's to pin at run time.
pub fn source_identity() -> std::collections::BTreeMap<&'static str, String> {
    let hash = |sources: &[&str]| {
        let mut hasher = blake3::Hasher::new();
        for source in sources {
            hasher.update(source.as_bytes());
        }
        encode_hex(hasher.finalize().as_bytes())
    };
    let mut identity = std::collections::BTreeMap::new();
    identity.insert(
        "component_source_blake3",
        hash(&[include_str!("component.rs")]),
    );
    identity.insert(
        "adapter_source_blake3",
        hash(&[
            include_str!("lib.rs"),
            include_str!("adapter.rs"),
            include_str!("protocol.rs"),
            include_str!("bodies.rs"),
        ]),
    );
    identity.insert(
        "property_source_blake3",
        hash(&[include_str!("property.rs")]),
    );
    identity.insert(
        "normalization_source_blake3",
        hash(&[include_str!("driver.rs")]),
    );
    identity
}
