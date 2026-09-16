use std::{env, path::Path};

use anyhow::{Context, Result, bail};
use rook_core::{
    CELL_ACTOR_ID, Envelope, EnvelopeKind, INPUT_CHANNEL_ID, OUTPUT_ACTOR_ID, OUTPUT_CHANNEL_ID,
    ReplayHashes, generate_synthetic_deliveries, replay,
};
use rook_host::abi::{FUEL_BUDGET, HostState, InboxMessage, define_rook_abi, deterministic_engine};
use rook_host::{encode_hex, rmf, rmf_bag};
use wasmtime::{Linker, Module, Store, StoreLimits, StoreLimitsBuilder};

const EVENT_COUNT: u32 = 100_000;
const CELL_MEMORY_PAGES: u64 = 32;
const CELL_MEMORY_BYTES: usize = CELL_MEMORY_PAGES as usize * 64 * 1024;
const CORE_MEMORY_BYTES: usize = 64 * 1024 * 1024;

fn flag_value(arguments: &[String], flag: &str) -> Option<String> {
    arguments
        .iter()
        .position(|argument| argument == flag)
        .and_then(|index| arguments.get(index + 1))
        .cloned()
}

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let first = args
        .next()
        .context("usage: rook-host <echo-count-cell.wasm> [core.wasm] | rook-host rmf <cell.wasm> [--verbose] [--dump <dir>]")?;
    if first == "rmf-bag" {
        let usage = "usage: rook-host rmf-bag <cell.wasm> <dir> [--goldens <file>]";
        let cell_path = args.next().context(usage)?;
        let dir = args.next().context(usage)?;
        let rest: Vec<String> = args.collect();
        let goldens = flag_value(&rest, "--goldens");
        return rmf_bag::run(
            Path::new(&cell_path),
            Path::new(&dir),
            goldens.as_deref().map(Path::new),
        );
    }
    if first == "rmf" {
        let cell_path = args.next().context(
            "usage: rook-host rmf <cell.wasm> [--verbose] [--dump <dir>] [--goldens <file>]",
        )?;
        let rest: Vec<String> = args.collect();
        let verbose = rest.iter().any(|arg| arg == "--verbose");
        let dump_dir = flag_value(&rest, "--dump");
        let goldens = flag_value(&rest, "--goldens");
        return rmf::run(
            Path::new(&cell_path),
            verbose,
            dump_dir.as_deref().map(Path::new),
            goldens.as_deref().map(Path::new),
        );
    }

    let native_hashes = run_cell_replay(Path::new(&first), EVENT_COUNT)?;
    let expected_hashes = replay(&generate_synthetic_deliveries(EVENT_COUNT))
        .map_err(|error| anyhow::anyhow!("native replay failed: {error:?}"))?;
    require_equal("cell replay", expected_hashes, native_hashes)?;

    println!("native_run_hash={}", encode_hex(&native_hashes.run_hash));
    println!(
        "native_output_digest={}",
        encode_hex(&native_hashes.output_digest)
    );

    if let Some(core_path) = args.next() {
        let wasm_hashes = run_core_wasm(Path::new(&core_path), EVENT_COUNT)?;
        require_equal("native versus wasm core", native_hashes, wasm_hashes)?;
        println!("wasm_run_hash={}", encode_hex(&wasm_hashes.run_hash));
        println!(
            "wasm_output_digest={}",
            encode_hex(&wasm_hashes.output_digest)
        );
        println!("run_hash={}", encode_hex(&native_hashes.run_hash));
        println!("output_digest={}", encode_hex(&native_hashes.output_digest));
    }

    Ok(())
}

/// Executes one cell once per live tick and compares its decisions with
/// the independently generated echo-and-count envelopes.
fn run_cell_replay(cell_path: &Path, event_count: u32) -> Result<ReplayHashes> {
    let engine = deterministic_engine()?;
    let module = Module::from_file(&engine, cell_path)
        .with_context(|| format!("load cell {}", cell_path.display()))?;
    let mut linker = Linker::new(&engine);
    define_rook_abi(&mut linker)?;

    let limits = StoreLimitsBuilder::new()
        .memory_size(CELL_MEMORY_BYTES)
        .build();
    let mut store = Store::new(
        &engine,
        HostState {
            actor_id: CELL_ACTOR_ID,
            emit_channels: vec![OUTPUT_CHANNEL_ID],
            emitted_payload_limit: event_count as usize,
            limits,
            ..HostState::default()
        },
    );
    store.limiter(|state| &mut state.limits);
    store.set_fuel(FUEL_BUDGET)?;
    let instance = linker
        .instantiate(&mut store, &module)
        .context("instantiate echo-count cell")?;
    let memory = instance
        .get_memory(&mut store, "memory")
        .context("cell must export memory")?;
    if memory.ty(&store).maximum() != Some(CELL_MEMORY_PAGES) {
        bail!("cell memory must declare a fixed {CELL_MEMORY_PAGES}-page maximum");
    }
    let rook_init = instance.get_typed_func::<u32, ()>(&mut store, "rook_init")?;
    let rook_step = instance.get_typed_func::<(), ()>(&mut store, "rook_step")?;
    rook_init.call(&mut store, CELL_ACTOR_ID)?;

    let deliveries = generate_synthetic_deliveries(event_count);
    let mut delivery_index = 0_usize;
    while delivery_index < deliveries.len() {
        let live_tick = deliveries[delivery_index].tick;
        let tick_start = delivery_index;
        while delivery_index < deliveries.len() && deliveries[delivery_index].tick == live_tick {
            delivery_index += 1;
        }
        store.data_mut().tick = live_tick;
        store.data_mut().inbox = (tick_start..delivery_index)
            .map(|event_index| InboxMessage {
                channel_id: INPUT_CHANNEL_ID,
                payload: rook_core::synthetic_payload(event_index as u32).to_vec(),
            })
            .collect();
        rook_step.call(&mut store, ())?;
        store.data_mut().inbox.clear();
    }

    let emitted_payloads = &store.data().emitted_payloads;
    if emitted_payloads.len() != event_count as usize {
        bail!(
            "cell emitted {} messages for {event_count} deliveries",
            emitted_payloads.len()
        );
    }
    let mut observed = Vec::with_capacity(deliveries.len() + emitted_payloads.len());
    observed.extend(deliveries);
    for emitted in emitted_payloads {
        observed.push(Envelope {
            kind: EnvelopeKind::Emit,
            tick: emitted.tick,
            src_actor: CELL_ACTOR_ID,
            dst_actor: OUTPUT_ACTOR_ID,
            channel_id: OUTPUT_CHANNEL_ID,
            payload_len: emitted.payload.len() as u32,
            src_seq: emitted.src_seq,
            payload_blake3: *blake3::hash(&emitted.payload).as_bytes(),
        });
    }
    replay_observed(&observed)
}

/// Enforces Replay grade: observed emits are compared with the recording
/// and never routed back into the cell.
fn replay_observed(observed: &[Envelope]) -> Result<ReplayHashes> {
    let deliveries: Vec<Envelope> = observed
        .iter()
        .copied()
        .filter(|envelope| envelope.kind == EnvelopeKind::Deliver)
        .collect();
    let expected = replay(&deliveries)
        .map_err(|error| anyhow::anyhow!("observed replay failed: {error:?}"))?;
    let expected_emits: Vec<Envelope> = generate_expected_emits(&deliveries);
    let actual_emits: Vec<Envelope> = observed
        .iter()
        .copied()
        .filter(|envelope| envelope.kind == EnvelopeKind::Emit)
        .collect();
    if actual_emits != expected_emits {
        bail!("cell output diverged from echo-and-count semantics");
    }
    Ok(expected)
}

fn generate_expected_emits(deliveries: &[Envelope]) -> Vec<Envelope> {
    deliveries
        .iter()
        .enumerate()
        .map(|(event_index, delivery)| {
            let mut payload = [0_u8; 24];
            payload[..16].copy_from_slice(&rook_core::synthetic_payload(event_index as u32));
            payload[16..].copy_from_slice(&(event_index as u64 + 1).to_le_bytes());
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
        .collect()
}

/// Runs the same `rook-core` source compiled to Wasm, providing the fourth
/// implementation target without introducing a second replay algorithm.
fn run_core_wasm(core_path: &Path, event_count: u32) -> Result<ReplayHashes> {
    let engine = deterministic_engine()?;
    let module = Module::from_file(&engine, core_path)
        .with_context(|| format!("load wasm core {}", core_path.display()))?;
    let limits = StoreLimitsBuilder::new()
        .memory_size(CORE_MEMORY_BYTES)
        .build();
    let mut store = Store::new(&engine, limits);
    store.limiter(|limits| limits);
    store.set_fuel(FUEL_BUDGET)?;
    let instance = wasmtime::Instance::new(&mut store, &module, &[])?;
    let replay = instance.get_typed_func::<u32, ()>(&mut store, "replay")?;
    let hash_word = instance.get_typed_func::<(u32, u32), u64>(&mut store, "hash_word")?;
    replay.call(&mut store, event_count)?;
    Ok(ReplayHashes {
        run_hash: read_hash_words(&mut store, &hash_word, 0)?,
        output_digest: read_hash_words(&mut store, &hash_word, 1)?,
    })
}

fn read_hash_words(
    store: &mut Store<StoreLimits>,
    function: &wasmtime::TypedFunc<(u32, u32), u64>,
    hash_kind: u32,
) -> Result<[u8; 32]> {
    let mut hash = [0_u8; 32];
    for word_index in 0..4_u32 {
        let word = function.call(&mut *store, (hash_kind, word_index))?;
        let start = word_index as usize * 8;
        hash[start..start + 8].copy_from_slice(&word.to_le_bytes());
    }
    Ok(hash)
}

fn require_equal(label: &str, expected: ReplayHashes, actual: ReplayHashes) -> Result<()> {
    if expected != actual {
        bail!("{label} hash mismatch: expected {expected:?}, actual {actual:?}");
    }
    Ok(())
}
