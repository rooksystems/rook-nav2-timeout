//! The host side of the ten-function cell ABI.
//!
//! Everything a cell can observe comes through `define_rook_abi`. The host
//! state is filled per tick by the caller and drained after `rook_step`.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use wasmtime::{Caller, Config, Engine, Linker, Memory, StoreLimits};

// Resource limits are deterministic safety boundaries, not hash inputs.
// Fuel is intentionally high enough that valid experiment runs never exhaust it.
pub const FUEL_BUDGET: u64 = 10_000_000_000;
pub const MAX_ABI_TRANSFER_BYTES: usize = 64 * 1024;
pub const MAX_EMITTED_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
const MAX_RETAINED_LOG_LINES: usize = 100_000;
const MAX_RETAINED_LOG_BYTES: usize = 4 * 1024 * 1024;

/// Output metadata captured at the ABI boundary, when the emit occurs.
/// Reconstructing it later from deliveries would hide delayed or reordered emits.
pub struct EmittedPayload {
    pub tick: u64,
    pub src_seq: u64,
    pub channel_id: u32,
    pub payload: Vec<u8>,
}

pub struct InboxMessage {
    pub channel_id: u32,
    pub payload: Vec<u8>,
}

#[derive(Default)]
pub struct HostState {
    pub tick: u64,
    pub actor_id: u32,
    pub inbox: Vec<InboxMessage>,
    /// Channels the cell may emit on; anything else is refused with -1.
    pub emit_channels: Vec<u32>,
    pub emitted_payloads: Vec<EmittedPayload>,
    pub emitted_payload_bytes: usize,
    pub emitted_payload_limit: usize,
    pub emit_sequence: u64,
    pub random_draw_ordinal: u64,
    /// Recorded configuration served through `param`. Absent keys return -1.
    pub params: BTreeMap<Vec<u8>, Vec<u8>>,
    /// Log lines (tick, level, bytes) are retained for diagnosis only; they
    /// are not hashed yet.
    pub log_lines: Vec<(u64, i32, Vec<u8>)>,
    pub retained_log_bytes: usize,
    pub limits: StoreLimits,
}

/// Builds the Wasmtime configuration that forms the host-side guarantee.
///
/// Fuel replaces epoch interruption so no wall clock enters execution. Guest
/// memory is bounded separately on each store. The exception-handling
/// proposal is on so C++ cells built with wasi-sdk can `throw`/`catch`; it
/// adds no host capability, only control flow inside the module.
pub fn deterministic_engine() -> Result<Engine> {
    let mut config = Config::new();
    config.cranelift_nan_canonicalization(true);
    config.relaxed_simd_deterministic(true);
    config.wasm_threads(false);
    config.wasm_exceptions(true);
    config.consume_fuel(true);
    Engine::new(&config).context("create deterministic Wasmtime engine")
}

/// Defines the cell's complete capability boundary.
///
/// The guest has no WASI imports. Anything not registered in this ten-function
/// module is physically unavailable at instantiation time.
pub fn define_rook_abi(linker: &mut Linker<HostState>) -> Result<()> {
    linker.func_wrap("rook", "tick", |caller: Caller<'_, HostState>| {
        caller.data().tick as i64
    })?;
    linker.func_wrap("rook", "inbox_len", |caller: Caller<'_, HostState>| {
        caller.data().inbox.len() as i32
    })?;
    linker.func_wrap(
        "rook",
        "inbox_meta",
        |caller: Caller<'_, HostState>, index: i32| -> i64 {
            let Some(message) = caller.data().inbox.get(index as usize) else {
                return 0;
            };
            (u64::from(message.channel_id) << 32 | message.payload.len() as u64) as i64
        },
    )?;
    linker.func_wrap(
        "rook",
        "inbox_read",
        |mut caller: Caller<'_, HostState>,
         index: i32,
         pointer: i32,
         capacity: i32|
         -> Result<i32> {
            let payload = caller
                .data()
                .inbox
                .get(index as usize)
                .context("inbox index out of range")?
                .payload
                .clone();
            if capacity < payload.len() as i32 {
                return Ok(-(payload.len() as i32));
            }
            guest_memory(&mut caller)?.write(&mut caller, pointer as usize, &payload)?;
            Ok(payload.len() as i32)
        },
    )?;
    linker.func_wrap(
        "rook",
        "emit",
        |mut caller: Caller<'_, HostState>,
         channel_id: i32,
         pointer: i32,
         length: i32|
         -> Result<i32> {
            if !caller.data().emit_channels.contains(&(channel_id as u32)) {
                return Ok(-1);
            }
            let (memory, pointer, length) =
                match checked_guest_range(&mut caller, pointer, length, MAX_ABI_TRANSFER_BYTES) {
                    Ok(range) => range,
                    Err(_) => return Ok(-1),
                };
            let next_payload_bytes = match validate_emit_retention(
                caller.data().emitted_payloads.len(),
                caller.data().emitted_payload_bytes,
                length,
                caller.data().emitted_payload_limit,
            ) {
                Some(next_payload_bytes) => next_payload_bytes,
                None => return Ok(-1),
            };
            let src_seq = caller.data().emit_sequence;
            let next_sequence = src_seq.checked_add(1).context("emit sequence overflow")?;
            let tick = caller.data().tick;
            let mut payload = vec![0_u8; length];
            memory.read(&caller, pointer, &mut payload)?;
            let state = caller.data_mut();
            state.emitted_payloads.push(EmittedPayload {
                tick,
                src_seq,
                channel_id: channel_id as u32,
                payload,
            });
            state.emitted_payload_bytes = next_payload_bytes;
            state.emit_sequence = next_sequence;
            Ok(0)
        },
    )?;
    linker.func_wrap(
        "rook",
        "set_timer",
        |_caller: Caller<'_, HostState>, _tick: i64| -> i32 { 0 },
    )?;
    linker.func_wrap(
        "rook",
        "rand",
        |mut caller: Caller<'_, HostState>, pointer: i32, length: i32| -> Result<()> {
            let (memory, pointer, length) =
                checked_guest_range(&mut caller, pointer, length, MAX_ABI_TRANSFER_BYTES)?;
            let state = caller.data();
            let mut input = Vec::with_capacity(32);
            input.extend_from_slice(&[0_u8; 8]);
            input.extend_from_slice(&u64::from(state.actor_id).to_le_bytes());
            input.extend_from_slice(&state.tick.to_le_bytes());
            input.extend_from_slice(&state.random_draw_ordinal.to_le_bytes());
            let mut output = vec![0_u8; length];
            let key = *blake3::hash(b"rook-rand-v1").as_bytes();
            blake3::Hasher::new_keyed(&key)
                .update(&input)
                .finalize_xof()
                .fill(&mut output);
            memory.write(&mut caller, pointer, &output)?;
            caller.data_mut().random_draw_ordinal = caller
                .data()
                .random_draw_ordinal
                .checked_add(1)
                .context("random draw ordinal overflow")?;
            Ok(())
        },
    )?;
    linker.func_wrap(
        "rook",
        "param",
        |mut caller: Caller<'_, HostState>,
         key_pointer: i32,
         key_length: i32,
         output_pointer: i32,
         output_capacity: i32|
         -> Result<i32> {
            let (memory, key_pointer, key_length) = match checked_guest_range(
                &mut caller,
                key_pointer,
                key_length,
                MAX_ABI_TRANSFER_BYTES,
            ) {
                Ok(range) => range,
                Err(_) => return Ok(-1),
            };
            let mut key = vec![0_u8; key_length];
            memory.read(&caller, key_pointer, &mut key)?;
            let Some(value) = caller.data().params.get(&key).cloned() else {
                return Ok(-1);
            };
            let (memory, output_pointer, output_capacity) = match checked_guest_range(
                &mut caller,
                output_pointer,
                output_capacity,
                MAX_ABI_TRANSFER_BYTES,
            ) {
                Ok(range) => range,
                Err(_) => return Ok(-1),
            };
            if value.len() > output_capacity {
                return Ok(-(value.len() as i32));
            }
            memory.write(&mut caller, output_pointer, &value)?;
            Ok(value.len() as i32)
        },
    )?;
    linker.func_wrap(
        "rook",
        "log",
        |mut caller: Caller<'_, HostState>, level: i32, pointer: i32, length: i32| -> Result<()> {
            let (memory, pointer, length) =
                checked_guest_range(&mut caller, pointer, length, MAX_ABI_TRANSFER_BYTES)?;
            let Some(next_log_bytes) = validate_log_retention(
                caller.data().log_lines.len(),
                caller.data().retained_log_bytes,
                length,
            ) else {
                return Ok(());
            };
            let mut line = vec![0_u8; length];
            memory.read(&caller, pointer, &mut line)?;
            let tick = caller.data().tick;
            let state = caller.data_mut();
            state.log_lines.push((tick, level, line));
            state.retained_log_bytes = next_log_bytes;
            Ok(())
        },
    )?;
    linker.func_wrap(
        "rook",
        "abort",
        |mut caller: Caller<'_, HostState>, code: i32, pointer: i32, length: i32| -> Result<()> {
            let mut reason = Vec::new();
            if let Ok((memory, pointer, length)) =
                checked_guest_range(&mut caller, pointer, length, MAX_ABI_TRANSFER_BYTES)
            {
                reason = vec![0_u8; length];
                memory.read(&caller, pointer, &mut reason)?;
            }
            bail!(
                "cell aborted with code {code}: {}",
                String::from_utf8_lossy(&reason)
            )
        },
    )?;
    Ok(())
}

fn guest_memory(caller: &mut Caller<'_, HostState>) -> Result<Memory> {
    caller
        .get_export("memory")
        .and_then(|export| export.into_memory())
        .context("cell memory export unavailable")
}

/// Validates an untrusted guest pointer and length before host allocation.
/// Store memory limits constrain Wasm pages, not vectors retained by the host.
fn checked_guest_range(
    caller: &mut Caller<'_, HostState>,
    pointer: i32,
    length: i32,
    maximum_length: usize,
) -> Result<(Memory, usize, usize)> {
    let memory = guest_memory(caller)?;
    let (pointer, length) =
        validate_abi_range(memory.data_size(&*caller), pointer, length, maximum_length)
            .context("invalid guest memory range")?;
    Ok((memory, pointer, length))
}

/// Applies both retention budgets before an emitted record is allocated.
/// The record limit matters independently because empty payloads consume no bytes.
fn validate_emit_retention(
    emitted_payload_count: usize,
    emitted_payload_bytes: usize,
    next_payload_length: usize,
    emitted_payload_limit: usize,
) -> Option<usize> {
    if emitted_payload_count >= emitted_payload_limit {
        return None;
    }
    let next_payload_bytes = emitted_payload_bytes.checked_add(next_payload_length)?;
    (next_payload_bytes <= MAX_EMITTED_PAYLOAD_BYTES).then_some(next_payload_bytes)
}

/// Applies count and byte budgets before allocating a retained guest log.
fn validate_log_retention(
    retained_log_count: usize,
    retained_log_bytes: usize,
    next_log_length: usize,
) -> Option<usize> {
    if retained_log_count >= MAX_RETAINED_LOG_LINES {
        return None;
    }
    let next_log_bytes = retained_log_bytes.checked_add(next_log_length)?;
    (next_log_bytes <= MAX_RETAINED_LOG_BYTES).then_some(next_log_bytes)
}

fn validate_abi_range(
    memory_size: usize,
    pointer: i32,
    length: i32,
    maximum_length: usize,
) -> Option<(usize, usize)> {
    let pointer = usize::try_from(pointer).ok()?;
    let length = usize::try_from(length).ok()?;
    if length > maximum_length {
        return None;
    }
    let end = pointer.checked_add(length)?;
    if end > memory_size {
        return None;
    }
    Some((pointer, length))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_range_rejects_negative_and_out_of_bounds_values() {
        assert_eq!(validate_abi_range(64, -1, 1, 64), None);
        assert_eq!(validate_abi_range(64, 0, -1, 64), None);
        assert_eq!(validate_abi_range(64, 60, 8, 64), None);
    }

    #[test]
    fn abi_range_rejects_transfers_above_the_explicit_limit() {
        assert_eq!(validate_abi_range(128, 0, 65, 64), None);
        assert_eq!(validate_abi_range(128, 64, 64, 64), Some((64, 64)));
    }

    #[test]
    fn emit_retention_limits_empty_records_and_total_bytes() {
        assert_eq!(validate_emit_retention(0, 0, 0, 1), Some(0));
        assert_eq!(validate_emit_retention(1, 0, 0, 1), None);
        assert_eq!(
            validate_emit_retention(0, MAX_EMITTED_PAYLOAD_BYTES, 1, 1),
            None
        );
    }

    #[test]
    fn log_retention_limits_line_count_and_total_bytes() {
        assert_eq!(validate_log_retention(0, 0, 1), Some(1));
        assert_eq!(validate_log_retention(MAX_RETAINED_LOG_LINES, 0, 0), None);
        assert_eq!(validate_log_retention(0, MAX_RETAINED_LOG_BYTES, 1), None);
    }
}
