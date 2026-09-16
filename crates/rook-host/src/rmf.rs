//! Experiment 2: Open-RMF's blockade moderator as a cell.
//!
//! Wire format, channels, and abort codes are in
//! docs/internals/rmf-blockade-boundary.md. The host runs seven scripted robots
//! against the cell, records that closed-loop run, replays it into fresh
//! instances, and hashes both with `rook_core::hash_observed`.

use std::path::Path;

use anyhow::{Context, Result, bail};
use rook_core::{
    CELL_ACTOR_ID, Envelope, EnvelopeKind, OUTPUT_ACTOR_ID, ReplayHashes, hash_observed,
    scheduling_key,
};
use wasmtime::{Linker, Module, Store, StoreLimitsBuilder};

use crate::abi::{
    EmittedPayload, FUEL_BUDGET, HostState, InboxMessage, define_rook_abi, deterministic_engine,
};
use crate::encode_hex;

const TRACE_DOMAIN: &[u8] = b"rook-trace-v1/rmf-blockade-synthetic@1";
const CELL_MEMORY_PAGES: u64 = 256;
const CELL_MEMORY_BYTES: usize = CELL_MEMORY_PAGES as usize * 64 * 1024;

const CH_SET: u32 = 10;
const CH_READY: u32 = 11;
const CH_REACHED: u32 = 12;
const CH_RELEASE: u32 = 13;
const CH_CANCEL: u32 = 14;
const CH_HEARTBEAT_TIMER: u32 = 15;
const CH_HEARTBEAT: u32 = 20;

const TIMER_ACTOR_ID: u32 = 50;
const FIRST_ROBOT_ACTOR_ID: u32 = 101;
const SCENARIO_TICKS: u64 = 4_000;
const HEARTBEAT_PERIOD: u64 = 100;
/// The ROS node's override of the library default, in radians.
const MIN_CONFLICT_ANGLE: f64 = 15.0 * core::f64::consts::PI / 180.0;

pub fn run(
    cell_path: &Path,
    verbose: bool,
    dump_dir: Option<&Path>,
    goldens_path: Option<&Path>,
) -> Result<()> {
    let recording = record(cell_path, MIN_CONFLICT_ANGLE)?;
    let recorded_hashes = recording.hashes()?;
    if let Some(dir) = dump_dir {
        dump_recording(dir, &recording)?;
    }
    let count = |channel: u32| {
        recording
            .deliveries
            .iter()
            .filter(|d| d.envelope.channel_id == channel)
            .count()
    };
    println!(
        "recorded deliveries={} (set={} ready={} reached={} release={} cancel={} timer={}) heartbeats={} exceptions_logged={} gridlock_heartbeats={}",
        recording.deliveries.len(),
        count(CH_SET),
        count(CH_READY),
        count(CH_REACHED),
        count(CH_RELEASE),
        count(CH_CANCEL),
        count(CH_HEARTBEAT_TIMER),
        recording.emits.len(),
        recording.exceptions_logged,
        recording.gridlock_heartbeats
    );
    if verbose {
        for (tick, level, line) in &recording.log_lines {
            println!("t={tick} log[{level}] {}", String::from_utf8_lossy(line));
        }
    }

    // Replay grade: recorded deliveries in, emits compared, never routed.
    let replayed = replay_recording(cell_path, MIN_CONFLICT_ANGLE, &recording.deliveries)?;
    require_same_run("replay", &recording, &replayed)?;

    let mut reversed = recording.deliveries.clone();
    reversed.reverse();
    let replayed_reversed = replay_recording(cell_path, MIN_CONFLICT_ANGLE, &reversed)?;
    require_same_run(
        "replay with reversed arrival order",
        &recording,
        &replayed_reversed,
    )?;

    // The configuration is a real decision input: a different recorded
    // parameter must give a different output digest for this scenario.
    let other_angle = replay_recording(
        cell_path,
        5.0 * core::f64::consts::PI / 180.0,
        &recording.deliveries,
    )?;
    let other_hashes = other_angle.hashes()?;
    if other_hashes.output_digest == recorded_hashes.output_digest {
        bail!("changing min_conflict_angle did not change the output digest");
    }
    if other_hashes.run_hash == recorded_hashes.run_hash {
        bail!("changing min_conflict_angle did not change the run hash");
    }

    let mut results = std::collections::BTreeMap::new();
    results.insert("rmf_run_hash", encode_hex(&recorded_hashes.run_hash));
    results.insert(
        "rmf_output_digest",
        encode_hex(&recorded_hashes.output_digest),
    );
    results.insert(
        "rmf_output_digest_min_conflict_angle_5deg",
        encode_hex(&other_hashes.output_digest),
    );
    for (key, value) in &results {
        println!("{key}={value}");
    }
    if let Some(goldens_path) = goldens_path {
        crate::rmf_bag::verify_goldens(goldens_path, &results)?;
        println!("goldens: all values match {}", goldens_path.display());
    }
    Ok(())
}

/// Writes the recording in the framing `native/driver.cpp` reads:
/// deliveries as (tick u64, channel u32, len u32, bytes), heartbeats as
/// (tick u64, len u32, bytes), all little-endian, scheduling-key order.
fn dump_recording(dir: &Path, recording: &Run) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut ordered: Vec<&Delivery> = recording.deliveries.iter().collect();
    ordered.sort_by_key(|d| scheduling_key(&d.envelope));
    let mut deliveries = Vec::new();
    for d in ordered {
        deliveries.extend_from_slice(&d.envelope.tick.to_le_bytes());
        deliveries.extend_from_slice(&d.envelope.channel_id.to_le_bytes());
        deliveries.extend_from_slice(&(d.payload.len() as u32).to_le_bytes());
        deliveries.extend_from_slice(&d.payload);
    }
    std::fs::write(dir.join("deliveries.bin"), deliveries)?;
    let mut heartbeats = Vec::new();
    for emitted in &recording.emits {
        heartbeats.extend_from_slice(&emitted.tick.to_le_bytes());
        heartbeats.extend_from_slice(&(emitted.payload.len() as u32).to_le_bytes());
        heartbeats.extend_from_slice(&emitted.payload);
    }
    std::fs::write(dir.join("heartbeats.bin"), heartbeats)?;
    Ok(())
}

struct Delivery {
    envelope: Envelope,
    payload: Vec<u8>,
}

impl Clone for Delivery {
    fn clone(&self) -> Self {
        Delivery {
            envelope: self.envelope,
            payload: self.payload.clone(),
        }
    }
}

struct Run {
    deliveries: Vec<Delivery>,
    emits: Vec<EmittedPayload>,
    log_lines: Vec<(u64, i32, Vec<u8>)>,
    exceptions_logged: usize,
    gridlock_heartbeats: usize,
}

impl Run {
    fn hashes(&self) -> Result<ReplayHashes> {
        let deliveries: Vec<Envelope> = self.deliveries.iter().map(|d| d.envelope).collect();
        let emits: Vec<Envelope> = self
            .emits
            .iter()
            .map(|emitted| Envelope {
                kind: EnvelopeKind::Emit,
                tick: emitted.tick,
                src_actor: CELL_ACTOR_ID,
                dst_actor: OUTPUT_ACTOR_ID,
                channel_id: emitted.channel_id,
                payload_len: emitted.payload.len() as u32,
                src_seq: emitted.src_seq,
                payload_blake3: *blake3::hash(&emitted.payload).as_bytes(),
            })
            .collect();
        hash_observed(TRACE_DOMAIN, &deliveries, &emits)
            .map_err(|error| anyhow::anyhow!("hash_observed failed: {error:?}"))
    }
}

fn require_same_run(label: &str, expected: &Run, actual: &Run) -> Result<()> {
    if expected.emits.len() != actual.emits.len() {
        bail!(
            "{label}: emitted {} heartbeats, recording has {}",
            actual.emits.len(),
            expected.emits.len()
        );
    }
    for (index, (a, b)) in expected.emits.iter().zip(&actual.emits).enumerate() {
        if a.tick != b.tick || a.src_seq != b.src_seq || a.payload != b.payload {
            bail!("{label}: heartbeat {index} differs from the recording");
        }
    }
    let expected_hashes = expected.hashes()?;
    let actual_hashes = actual.hashes()?;
    if expected_hashes != actual_hashes {
        bail!("{label}: hash mismatch: expected {expected_hashes:?}, actual {actual_hashes:?}");
    }
    Ok(())
}

/// One delivery from a real recording: bag receive time as the tick.
#[derive(Clone)]
pub struct BagDelivery {
    pub tick: u64,
    pub channel_id: u32,
    pub payload: Vec<u8>,
}

/// Feeds bag deliveries tick by tick to a fresh cell and returns its
/// (tick, heartbeat payload) emissions. Ticks are already strictly
/// increasing, so one tick is one executor callback, like the real node.
pub fn replay_bag_deliveries(
    cell_path: &Path,
    min_conflict_angle: f64,
    deliveries: &[BagDelivery],
) -> Result<Vec<(u64, Vec<u8>)>> {
    let mut cell = instantiate(cell_path, min_conflict_angle, deliveries.len() * 2)?;
    for delivery in deliveries {
        cell.step(
            delivery.tick,
            vec![InboxMessage {
                channel_id: delivery.channel_id,
                payload: delivery.payload.clone(),
            }],
        )?;
    }
    let run = cell.finish();
    Ok(run
        .emits
        .into_iter()
        .map(|emitted| (emitted.tick, emitted.payload))
        .collect())
}

// ---------------------------------------------------------------------------
// Cell instance

struct Cell {
    store: Store<HostState>,
    rook_step: wasmtime::TypedFunc<(), ()>,
}

fn instantiate(cell_path: &Path, min_conflict_angle: f64, emit_limit: usize) -> Result<Cell> {
    let engine = deterministic_engine()?;
    let module = Module::from_file(&engine, cell_path)
        .with_context(|| format!("load cell {}", cell_path.display()))?;
    let mut linker = Linker::new(&engine);
    define_rook_abi(&mut linker)?;
    let limits = StoreLimitsBuilder::new()
        .memory_size(CELL_MEMORY_BYTES)
        .build();
    let mut params = std::collections::BTreeMap::new();
    params.insert(
        b"min_conflict_angle".to_vec(),
        min_conflict_angle.to_le_bytes().to_vec(),
    );
    let mut store = Store::new(
        &engine,
        HostState {
            actor_id: CELL_ACTOR_ID,
            emit_channels: vec![CH_HEARTBEAT],
            emitted_payload_limit: emit_limit,
            params,
            limits,
            ..HostState::default()
        },
    );
    store.limiter(|state| &mut state.limits);
    store.set_fuel(FUEL_BUDGET)?;
    let instance = linker
        .instantiate(&mut store, &module)
        .context("instantiate rmf-blockade cell")?;
    let memory = instance
        .get_memory(&mut store, "memory")
        .context("cell must export memory")?;
    if memory.ty(&store).maximum() != Some(CELL_MEMORY_PAGES) {
        bail!("cell memory must declare a fixed {CELL_MEMORY_PAGES}-page maximum");
    }
    // wasi-sdk reactors run static constructors from `_initialize`.
    if let Ok(initialize) = instance.get_typed_func::<(), ()>(&mut store, "_initialize") {
        initialize.call(&mut store, ())?;
    }
    let rook_init = instance.get_typed_func::<u32, ()>(&mut store, "rook_init")?;
    let rook_step = instance.get_typed_func::<(), ()>(&mut store, "rook_step")?;
    rook_init.call(&mut store, CELL_ACTOR_ID)?;
    Ok(Cell { store, rook_step })
}

impl Cell {
    /// Runs one live tick. Returns the index of the first emit produced by it.
    fn step(&mut self, tick: u64, inbox: Vec<InboxMessage>) -> Result<usize> {
        let first_emit = self.store.data().emitted_payloads.len();
        self.store.data_mut().tick = tick;
        self.store.data_mut().inbox = inbox;
        self.rook_step.call(&mut self.store, ())?;
        self.store.data_mut().inbox.clear();
        Ok(first_emit)
    }

    fn finish(self) -> Run {
        let state = self.store.into_data();
        let exceptions_logged = state
            .log_lines
            .iter()
            .filter(|(_, level, _)| *level == 2)
            .count();
        let gridlock_heartbeats = state
            .emitted_payloads
            .iter()
            .filter(|emitted| emitted.payload.first() == Some(&1))
            .count();
        Run {
            deliveries: Vec::new(),
            emits: state.emitted_payloads,
            log_lines: state.log_lines,
            exceptions_logged,
            gridlock_heartbeats,
        }
    }
}

/// Feeds recorded deliveries to a fresh cell in scheduling-key order.
fn replay_recording(
    cell_path: &Path,
    min_conflict_angle: f64,
    deliveries: &[Delivery],
) -> Result<Run> {
    let mut ordered: Vec<Delivery> = deliveries.to_vec();
    ordered.sort_by_key(|d| scheduling_key(&d.envelope));
    let mut cell = instantiate(cell_path, min_conflict_angle, ordered.len())?;
    let mut index = 0;
    while index < ordered.len() {
        let tick = ordered[index].envelope.tick;
        let mut inbox = Vec::new();
        while index < ordered.len() && ordered[index].envelope.tick == tick {
            inbox.push(InboxMessage {
                channel_id: ordered[index].envelope.channel_id,
                payload: ordered[index].payload.clone(),
            });
            index += 1;
        }
        cell.step(tick, inbox)?;
    }
    let mut run = cell.finish();
    run.deliveries = ordered;
    Ok(run)
}

// ---------------------------------------------------------------------------
// Synthetic recording

#[derive(Clone)]
struct Checkpoint {
    x: f64,
    y: f64,
    map: &'static str,
    can_hold: bool,
}

fn cp(x: f64, y: f64, map: &'static str) -> Checkpoint {
    Checkpoint {
        x,
        y,
        map,
        can_hold: true,
    }
}

struct Robot {
    participant: u64,
    src_actor: u32,
    seq: u64,
    forward: Vec<Checkpoint>,
    radius: f64,
    travel_ticks: u64,
    reservation: u64,
    heading_forward: bool,
    last_reached: u64,
    ready_at: Option<u64>,
    granted_end: u64,
    next_action_tick: u64,
    cycles: u32,
    jumped_the_gun: bool,
    released_once: bool,
    sent_stale_ready: bool,
    cancelled_once: bool,
}

impl Robot {
    fn path(&self) -> Vec<Checkpoint> {
        let mut path = self.forward.clone();
        if !self.heading_forward {
            path.reverse();
        }
        path
    }
}

/// Seven robots. Robots 1 through 4 are the four-way standoff square from
/// rmf_traffic's
/// own tests. 6 runs a straight lane and 5 merges into it at 8.5°, which the
/// ROS node's 15° setting classifies as an alignment (lane sharing) and the
/// library default of 5° classifies as a conflict. 7 is on another map and
/// conflicts with nobody. No robot starts or ends within another's path, so
/// the moderator's gridlock condition is never met by construction.
fn make_robots() -> Vec<Robot> {
    let paths: Vec<(Vec<Checkpoint>, u64)> = vec![
        (
            vec![cp(5.0, 0.0, "L1"), cp(5.0, 5.0, "L1"), cp(5.0, 15.0, "L1")],
            7,
        ),
        (
            vec![
                cp(0.0, 10.0, "L1"),
                cp(5.0, 10.0, "L1"),
                cp(15.0, 10.0, "L1"),
            ],
            9,
        ),
        (
            vec![
                cp(10.0, 15.0, "L1"),
                cp(10.0, 10.0, "L1"),
                cp(10.0, 0.0, "L1"),
            ],
            8,
        ),
        (
            vec![cp(15.0, 5.0, "L1"), cp(10.0, 5.0, "L1"), cp(0.0, 5.0, "L1")],
            11,
        ),
        (
            vec![
                cp(0.0, 33.0, "L1"),
                cp(14.0, 30.9, "L1"),
                cp(20.0, 30.0, "L1"),
                cp(30.0, 30.0, "L1"),
                cp(30.0, 40.0, "L1"),
            ],
            10,
        ),
        (
            vec![
                cp(0.0, 30.0, "L1"),
                cp(10.0, 30.0, "L1"),
                cp(20.0, 30.0, "L1"),
                cp(30.0, 30.0, "L1"),
                cp(40.0, 30.0, "L1"),
            ],
            9,
        ),
        (vec![cp(5.0, 0.0, "L2"), cp(5.0, 15.0, "L2")], 6),
    ];
    paths
        .into_iter()
        .enumerate()
        .map(|(index, (forward, travel_ticks))| Robot {
            participant: index as u64 + 1,
            src_actor: FIRST_ROBOT_ACTOR_ID + index as u32,
            seq: 0,
            forward,
            radius: 0.5,
            travel_ticks,
            reservation: 0,
            heading_forward: true,
            last_reached: 0,
            ready_at: None,
            granted_end: 0,
            next_action_tick: 3 + index as u64,
            cycles: 0,
            jumped_the_gun: false,
            released_once: false,
            sent_stale_ready: false,
            cancelled_once: false,
        })
        .collect()
}

fn record(cell_path: &Path, min_conflict_angle: f64) -> Result<Run> {
    let mut robots = make_robots();
    let mut timer_seq = 0_u64;
    let mut deliveries: Vec<Delivery> = Vec::new();
    // Every inbound message yields at most one heartbeat.
    let emit_limit = (SCENARIO_TICKS as usize) * 8;
    let mut cell = instantiate(cell_path, min_conflict_angle, emit_limit)?;

    for tick in 0..SCENARIO_TICKS {
        let mut inbox: Vec<Delivery> = Vec::new();
        if tick % HEARTBEAT_PERIOD == 0 {
            inbox.push(delivery(
                tick,
                TIMER_ACTOR_ID,
                &mut timer_seq,
                CH_HEARTBEAT_TIMER,
                Vec::new(),
            ));
        }
        for robot in &mut robots {
            inbox.extend(robot_messages(robot, tick));
        }
        if inbox.is_empty() {
            continue;
        }
        inbox.sort_by_key(|d| scheduling_key(&d.envelope));
        let step_inbox = inbox
            .iter()
            .map(|d| InboxMessage {
                channel_id: d.envelope.channel_id,
                payload: d.payload.clone(),
            })
            .collect();
        let first_emit = cell.step(tick, step_inbox)?;
        for emitted in &cell.store.data().emitted_payloads[first_emit..] {
            apply_heartbeat(&mut robots, &emitted.payload)?;
        }
        deliveries.extend(inbox);
    }
    let mut run = cell.finish();
    run.deliveries = deliveries;
    Ok(run)
}

/// The scripted robot. It advances only within the range the last heartbeat
/// granted, so the recording is a plausible closed-loop history. Two robots
/// misbehave on purpose at fixed ticks to reach the moderator's error paths.
fn robot_messages(robot: &mut Robot, tick: u64) -> Vec<Delivery> {
    let mut out = Vec::new();
    if tick < robot.next_action_tick {
        return out;
    }
    let path = robot.path();
    let last_index = path.len() as u64 - 1;

    // Deliberate misbehavior, once each, at a fixed point in the script.
    if robot.participant == 1
        && tick >= 600
        && !robot.jumped_the_gun
        && robot.reservation != 0
        && robot.last_reached == 0
        && robot.ready_at == Some(0)
        && robot.granted_end == 0
    {
        // Claims to be two checkpoints ahead of any grant it could hold:
        // Moderator::reached throws and marks the participant critical_error;
        // BlockadeNode logs the exception and carries on.
        robot.jumped_the_gun = true;
        let checkpoint = 2;
        out.push(robot_delivery(
            robot,
            tick,
            CH_REACHED,
            ready_like(robot, checkpoint),
        ));
        robot.next_action_tick = tick + 5;
        return out;
    }
    if robot.participant == 3 && tick >= 900 && robot.reservation != 0 && !robot.released_once {
        robot.released_once = true;
        // Backs off from its ready checkpoint, then readies again later.
        out.push(robot_delivery(
            robot,
            tick,
            CH_RELEASE,
            ready_like(robot, robot.last_reached),
        ));
        robot.ready_at = None;
        robot.next_action_tick = tick + 12;
        return out;
    }
    if robot.participant == 2 && tick >= 2_200 && !robot.sent_stale_ready {
        robot.sent_stale_ready = true;
        // Stale reservation id: the moderator must ignore it.
        let stale = robot.reservation.saturating_sub(1);
        let mut payload = Vec::new();
        payload.extend_from_slice(&robot.participant.to_le_bytes());
        payload.extend_from_slice(&stale.to_le_bytes());
        payload.extend_from_slice(&0_u64.to_le_bytes());
        out.push(robot_delivery(robot, tick, CH_READY, payload));
        robot.next_action_tick = tick + 1;
        return out;
    }
    if robot.participant == 4 && tick >= 3_000 && robot.reservation != 0 && !robot.cancelled_once {
        robot.cancelled_once = true;
        // Cancels everything mid-route, then starts a fresh cycle.
        let mut payload = Vec::new();
        payload.extend_from_slice(&robot.participant.to_le_bytes());
        payload.push(1);
        payload.extend_from_slice(&0_u64.to_le_bytes());
        out.push(robot_delivery(robot, tick, CH_CANCEL, payload));
        robot.reservation = 0;
        robot.next_action_tick = tick + 20;
        return out;
    }

    if robot.reservation == 0 {
        if robot.cycles >= 200 {
            return out;
        }
        robot.reservation = robot.cycles as u64 + 1;
        robot.cycles += 1;
        robot.last_reached = 0;
        robot.ready_at = None;
        robot.granted_end = 0;
        out.push(robot_delivery(
            robot,
            tick,
            CH_SET,
            encode_set(robot, &path),
        ));
        robot.next_action_tick = tick + 3;
        return out;
    }
    if robot.last_reached == last_index {
        // Arrived. Turn around after a pause; the next set() supersedes.
        robot.reservation = 0;
        robot.heading_forward = !robot.heading_forward;
        robot.next_action_tick = tick + 15;
        return out;
    }
    if robot.ready_at != Some(robot.last_reached) {
        robot.ready_at = Some(robot.last_reached);
        out.push(robot_delivery(
            robot,
            tick,
            CH_READY,
            ready_like(robot, robot.last_reached),
        ));
        robot.next_action_tick = tick + 2;
        return out;
    }
    if robot.granted_end > robot.last_reached {
        robot.last_reached += 1;
        out.push(robot_delivery(
            robot,
            tick,
            CH_REACHED,
            ready_like(robot, robot.last_reached),
        ));
        robot.next_action_tick = tick + robot.travel_ticks;
        return out;
    }
    // Waiting for a grant; poll again next tick.
    robot.next_action_tick = tick + 1;
    out
}

fn apply_heartbeat(robots: &mut [Robot], payload: &[u8]) -> Result<()> {
    let statuses = decode_heartbeat(payload)?;
    for status in statuses {
        if let Some(robot) = robots
            .iter_mut()
            .find(|r| r.participant == status.participant)
            && robot.reservation == status.reservation
        {
            robot.granted_end = status.assignment_end;
        }
    }
    Ok(())
}

#[derive(Debug)]
struct Status {
    participant: u64,
    reservation: u64,
    assignment_end: u64,
}

fn decode_heartbeat(payload: &[u8]) -> Result<Vec<Status>> {
    const STATUS_BYTES: usize = 49;

    let mut cursor = 0_usize;
    let mut take = |n: usize| -> Result<&[u8]> {
        let slice = payload
            .get(cursor..cursor + n)
            .context("short heartbeat payload")?;
        cursor += n;
        Ok(slice)
    };
    let _has_gridlock = take(1)?[0];
    let count = u32::from_le_bytes(take(4)?.try_into()?);
    let required_payload_bytes = (count as usize)
        .checked_mul(STATUS_BYTES)
        .and_then(|status_bytes| 5_usize.checked_add(status_bytes))
        .context("heartbeat status count overflow")?;
    if payload.len() < required_payload_bytes {
        bail!("short heartbeat payload");
    }
    let mut statuses = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let participant = u64::from_le_bytes(take(8)?.try_into()?);
        let reservation = u64::from_le_bytes(take(8)?.try_into()?);
        let _any_ready = take(1)?[0];
        let _last_ready = take(8)?;
        let _last_reached = take(8)?;
        let _assignment_begin = take(8)?;
        let assignment_end = u64::from_le_bytes(take(8)?.try_into()?);
        statuses.push(Status {
            participant,
            reservation,
            assignment_end,
        });
    }
    if cursor != payload.len() {
        bail!("trailing bytes in heartbeat payload");
    }
    Ok(statuses)
}

fn ready_like(robot: &Robot, checkpoint: u64) -> Vec<u8> {
    let mut payload = Vec::with_capacity(24);
    payload.extend_from_slice(&robot.participant.to_le_bytes());
    payload.extend_from_slice(&robot.reservation.to_le_bytes());
    payload.extend_from_slice(&checkpoint.to_le_bytes());
    payload
}

fn encode_set(robot: &Robot, path: &[Checkpoint]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&robot.participant.to_le_bytes());
    payload.extend_from_slice(&robot.reservation.to_le_bytes());
    payload.extend_from_slice(&robot.radius.to_le_bytes());
    payload.extend_from_slice(&(path.len() as u32).to_le_bytes());
    for checkpoint in path {
        payload.extend_from_slice(&checkpoint.x.to_le_bytes());
        payload.extend_from_slice(&checkpoint.y.to_le_bytes());
        payload.push(u8::from(checkpoint.can_hold));
        payload.extend_from_slice(&(checkpoint.map.len() as u16).to_le_bytes());
        payload.extend_from_slice(checkpoint.map.as_bytes());
    }
    payload
}

fn robot_delivery(robot: &mut Robot, tick: u64, channel_id: u32, payload: Vec<u8>) -> Delivery {
    delivery(tick, robot.src_actor, &mut robot.seq, channel_id, payload)
}

fn delivery(
    tick: u64,
    src_actor: u32,
    seq: &mut u64,
    channel_id: u32,
    payload: Vec<u8>,
) -> Delivery {
    let envelope = Envelope {
        kind: EnvelopeKind::Deliver,
        tick,
        src_actor,
        dst_actor: CELL_ACTOR_ID,
        channel_id,
        payload_len: payload.len() as u32,
        src_seq: *seq,
        payload_blake3: *blake3::hash(&payload).as_bytes(),
    };
    *seq += 1;
    Delivery { envelope, payload }
}

#[cfg(test)]
mod heartbeat_tests {
    use std::path::PathBuf;

    use super::{BagDelivery, decode_heartbeat, replay_bag_deliveries};

    #[test]
    fn heartbeat_count_cannot_allocate_beyond_the_payload() {
        let payload = [0, 0xff, 0xff, 0xff, 0xff];
        let error = decode_heartbeat(&payload).expect_err("inflated heartbeat count must fail");
        assert_eq!(error.to_string(), "short heartbeat payload");
    }

    #[test]
    fn cell_rejects_checkpoint_count_before_allocating() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&1_u64.to_le_bytes());
        payload.extend_from_slice(&1_u64.to_le_bytes());
        payload.extend_from_slice(&0.5_f64.to_le_bytes());
        payload.extend_from_slice(&u32::MAX.to_le_bytes());
        let delivery = BagDelivery {
            tick: 1,
            channel_id: 10,
            payload,
        };
        let cell = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../cells/rmf-blockade/prebuilt/rmf_blockade_cell.wasm");
        let error = replay_bag_deliveries(&cell, 15_f64.to_radians(), &[delivery])
            .expect_err("inflated checkpoint count must abort the cell");
        assert!(
            format!("{error:#}")
                .contains("cell aborted with code 4: checkpoint count exceeds payload")
        );
    }
}
