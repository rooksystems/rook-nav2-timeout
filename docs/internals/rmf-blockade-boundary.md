# Open-RMF blockade boundary

This document defines the component boundary used by the RMF Wasm cell and its recordings. [ADR 0002](../adr/0002-experiment-2.md) records the extraction experiment; [ADRs 0003](../adr/0003-real-recording-replay.md), [0004](../adr/0004-recorder-tap.md) and [0005](../adr/0005-timer-inputs-close-experiment-2.md) explain how the recording became complete enough to reproduce the running node's decisions.

## Component and state

The cell wraps `rmf_traffic::blockade::Moderator` from [rmf_traffic](https://github.com/open-rmf/rmf_traffic) at `39f09e7971c8e666e12c8e9b12199014f631c0bb`. It grants participants ranges along their requested paths, accounting for path conflicts and gridlock. The surrounding ROS wrapper is `rmf_traffic_ros2::blockade::BlockadeNode`; [the tap provenance](../../cells/rmf-blockade/tap/UPSTREAM.md) identifies its source and recording changes.

The moderator keeps reservations, paths and radii; a FIFO ready queue; assigned ranges and their version; participant status; and the derived blocker, alignment and constraint structures. Ready-queue order affects which participant receives a grant. The wrapper also remembers the last published assignment version.

Replay starts with an empty moderator and the recorded configuration. The component has no external database or parameter lookup after initialization. Robot-side `Participant`, `Rectifier` and fleet-adapter behavior remain outside this boundary.

## Inputs

The ROS wrapper subscribes to five `rmf_traffic_msgs` topics using best-effort delivery and a depth-10 queue. Each invokes the corresponding moderator method, followed by the wrapper's update check.

- `rmf_traffic/blockade_set` carries `BlockadeSet`, including participant and reservation IDs, radius and path checkpoints.
- `rmf_traffic/blockade_ready`, `rmf_traffic/blockade_reached` and `rmf_traffic/blockade_release` carry participant, reservation and checkpoint IDs.
- `rmf_traffic/blockade_cancel` carries participant and reservation IDs and the `all_reservations` flag.

Every checkpoint field affects the interpretation of a path. The conflict calculation uses `map_name` to distinguish maps and `can_hold` to extend conflict brackets over checkpoints where a robot cannot wait.

Two further inputs are required. The ROS wrapper sets `minimum_conflict_angle` to 15 degrees in radians, overriding the library's 5-degree default. The cell reads that recorded value through `param("min_conflict_angle")` during initialization. Each heartbeat-timer callback is recorded as a separate delivery, including firings that produce no heartbeat. The cell reads no live clock.

Input order must describe what the node processed. DDS delivery and QoS drops happen before the callback boundary; an external topic recorder may observe a different cross-topic order. The complete recordings use callback-entry records from a single-threaded executor, including timer callbacks. Replay applies the recorded scheduling order.

## Outputs and comparison

A heartbeat contains a gridlock flag and one status per participant, including reservation, readiness, reached checkpoint and granted range. The wrapper publishes when the assignments version changes and on each one-second heartbeat timer. A version change can leave the granted ranges unchanged, so repeated payloads are meaningful.

The cell sorts statuses by participant ID before serialization. The recording tap also sorts the ROS message's statuses before writing the Rook heartbeat payload. The recorded comparison therefore uses this explicit serialization order; it does not compare the ROS middleware's original wire encoding. Both [cell serialization](../../cells/rmf-blockade/src/cell.cpp) and [recorder serialization](../../cells/rmf-blockade/tap/src/Recorder.cpp) implement that boundary choice.

The complete fit and holdout cases compare ordered heartbeat payloads, including duplicates. Recorded publication timestamps differ legitimately from replay's timer-delivery ticks and are protected separately by recording hashes.

## Other dependencies

The moderator's info and debug loggers write through Rook's log import. The host retains these diagnostics, but execution hashes exclude the log text. Constraint descriptions can depend on internal container iteration order.

The ROS wrapper catches `std::exception`, logs the failure and continues with the moderator's resulting state. The cell preserves that behavior. In particular, `Moderator::reached` can mark a participant with `critical_error` before throwing for a checkpoint beyond its grant.

The moderator uses unordered containers, including pointer-keyed sets, and floating-point geometry through Eigen and the C++ math library. The Wasm build fixes the code executing those operations, and the host enables NaN canonicalization. Native builds can use different library implementations and compiler settings; agreement with one native recording must be measured. The fit and holdout results establish the comparisons described in their ADRs.

## Cell payloads

Payload integers and floating-point values are little-endian, without padding. These layouts are implemented by the cell shim and recording tools.

```text
channel 10, set
  participant u64, reservation u64, radius f64, n u32
  n checkpoints, each x f64, y f64, can_hold u8, name_len u16, name bytes

channel 11, ready; channel 12, reached; channel 13, release
  participant u64, reservation u64, checkpoint u64

channel 14, cancel
  participant u64, all_reservations u8, reservation u64

channel 15, heartbeat_timer
  empty payload

channel 20, heartbeat
  has_gridlock u8, n u32
  n statuses, sorted by participant
  each participant u64, reservation u64, any_ready u8, last_ready u64,
  last_reached u64, assignment_begin u64, assignment_end u64
```

`min_conflict_angle` is an eight-byte IEEE-754 double read once in `rook_init`. A missing parameter aborts initialization.

The shim's abort codes identify failures at the boundary. Code 2 means a missing parameter, 3 an unknown channel, 4 a malformed payload, 5 a host refusal of an emitted payload, and 6 a short inbox read. Code 7 rejects a participant or checkpoint ID at or above 2^32. Code 8 rejects more than 1,337 participants, the limit imposed by a 5-byte header, 49-byte statuses and the ABI's 64 KiB transfer limit. A libc `proc_exit(n)` becomes abort code `100+n`.

## Build requirements

[build.sh](../../cells/rmf-blockade/build.sh) builds the cell using wasi-sdk 33 and Eigen 5.0.1. The host calls the reactor's `_initialize` before `rook_init`. C++ exceptions use standardized exnref/`try_table` encoding, enabled in Wasmtime with its required collector feature.

The shim resolves the C++ runtime's WASI symbols inside the module. Logging is forwarded to Rook, process exit aborts the guest, and the remaining compatibility stubs expose no filesystem or network access. The module imports only from `rook`.

Wasm32's 32-bit `size_t` requires the explicit ID-width guard. The build supplies a missing `<cassert>` include without editing upstream source and uses fixed 16 MiB linear memory, a 1 MiB stack and the flags recorded in the build script. Rebuilding a distributed cell requires checking the resulting artifact identity under the approved toolchain.

These choices establish a boundary around moderator decisions. Physical motion, perception, DDS transport behavior and the rest of the robot's software remain outside the replayed component.
