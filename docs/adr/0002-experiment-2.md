# ADR 0002 - Running the blockade moderator as a Wasm cell

Accepted on 2026-08-18 for the extraction experiment described here. Rust 1.95.0, Wasmtime 38.0.4, wasi-sdk 33.0 with clang 22.1.0, and Eigen 5.0.1.

## Decision

Open-RMF's `rmf_traffic::blockade::Moderator` can run as a Wasm cell with its decision sources unchanged. Its existing message boundary made it a suitable first integration. Configuration and heartbeat-timer deliveries become explicit inputs, and the host supplies the ten-function Rook interface.

The vendored component comes from `open-rmf/rmf_traffic` at `39f09e79`. [Upstream provenance](../../cells/rmf-blockade/vendor/UPSTREAM.md) identifies the source files; the [boundary map](../internals/rmf-blockade-boundary.md) describes the wrapper, inputs and exclusions.

This experiment used a synthetic scenario driven by the cell. Testing against a recording of the ROS node remained necessary before the larger experiment's held-out conformance requirement could be evaluated.

## Evidence

On macOS 26.5 / Apple ARM64, seven scripted robots ran a 4,000-tick scenario. The recording contained 3,515 deliveries, comprising 659 `set`, 1,407 `ready`, 1,407 `reached`, one `release`, one `cancel` and 40 timer ticks. The cell emitted 3,358 heartbeats, including one after catching the moderator's `std::runtime_error` for a checkpoint beyond its grant.

The host performed four comparisons.

1. A closed-loop run generated the recording while scripted participants advanced according to granted ranges. Deliberate invalid requests exercised release, stale reservation, cancellation and exception handling.
2. A fresh instance replayed the deliveries and reproduced the heartbeat ticks, sequence values and payload bytes.
3. Reversing the deliveries' arrival order produced the same result because replay orders them by scheduling key.
4. Changing the recorded conflict angle from 15 degrees to 5 degrees changed the decision digest. The scenario's 8.5-degree merge distinguished lane sharing from conflict.

The 15-degree run produced `rmf_run_hash` `cf761c7dafe77ca29fb8e29b8f18c248234b516385e5de9a87d5476e828c7a22` and `rmf_output_digest` `cebb8c7a3b1b68a21c7f70fb5a309b66b7ce4ebbd467d208e35d3094bc9713ba`. The 5-degree run produced `rmf_output_digest` `d9edfd1d173aac3c8324450efb5a05dd2f5150682724e81d1798b9a3de4096ea`.

The new `hash_observed` path was checked against the echo-and-count replay from [ADR 0001](0001-experiment-1.md), preserving that experiment's hashes. A separate native build of the shim and moderator, using Apple clang 17 and the platform libraries, produced 1,206,675 heartbeat bytes identical to the Wasm run. That comparison measured agreement on this host and scenario. Linux runs of this experiment had not been performed in this session.

## Why this component was tractable

The moderator was about 2,600 lines of C++ with in-memory state, five input message types and one output type. Its decision code required no clock, filesystem, external store or threads. The ROS wrapper already supplied configuration and timer-driven publication around it.

The reported extraction took about 30 minutes after choosing the component, including the boundary review, toolchain changes and scenario corrections. That timing applies to this unusually simple boundary. Components involving trajectory planning, schedule state or an external store require separate integration evidence.

## Integration constraints

C++ exceptions had to remain available because the node catches a moderator exception and continues. Disabling exceptions would change that behavior. Wasmtime 38 rejected wasi-sdk's default legacy exception encoding; `-mllvm -wasm-use-legacy-eh=false` selected standardized `try_table`/exnref encoding. The host enabled Wasm exceptions and the required `gc-null` feature.

The C++ runtime introduced WASI imports even though the moderator did not use host files. Definitions inside the shim resolved those symbols so the cell imported only from `rook`. The host also calls `_initialize` when the module exports it.

Wasm32 makes `size_t` 32 bits while participant and checkpoint IDs are `uint64_t`. The build allows the upstream narrowing, and the shim aborts on IDs at or above 2^32 to prevent aliasing. A build-level `-include cassert` supplies a missing upstream include while preserving the vendored source bytes.

The scenario needed corrections too. Endpoints placed on another route caused gridlock, and a merge without an endpoint inside the conflict radius could not distinguish the two angle settings. An invalid movement test was made unambiguous by claiming two checkpoints beyond the grant. These were defects in the experiment inputs.

## Boundary limits

The ROS wrapper emits participant statuses in hash-map iteration order; the cell sorts them by participant ID. A comparison with native recorded output must account explicitly for that ordering. The boundary review also confirmed that the moderator reads both `map_name` and `can_hold`.

Diagnostic log text is retained but excluded from execution hashes. `set_timer` and randomness are unused by this component; heartbeat-timer firings arrive as recorded deliveries. The recording must capture what the node actually processed, including any effect of message delivery and callback order.

The robot-side participant, rectifier, fleet adapter and surrounding world remain outside the cell. This experiment established extraction and synthetic replay. [ADR 0003](0003-real-recording-replay.md) records the subsequent comparison with an independently running ROS node.
