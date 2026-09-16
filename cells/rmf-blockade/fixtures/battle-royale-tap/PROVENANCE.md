# Recording provenance for battle-royale-tap

Recorded 2026-08-19 on a Lambda `gpu_1x_a10` in `us-west-1` (Ubuntu 22.04 x86-64), inside a `ros:humble` Docker container, Gazebo Classic 11.10.2 headless. The container image was `ros@sha256:75dd3aba34a3838dadbb31a9f7bef769bdfa8713e6cec686fc868db2981b0987` (`RIG_NOTES.md` from the recording rig).

The recorded node used the following components.

- `ros-humble-rmf-traffic` **3.0.3-1jammy.20260304.203604** supplied the moderator decision code through the installed `/opt/ros/humble/lib/x86_64-linux-gnu/librmf_traffic.so`. `BINSHA`, written at record time after resolving the node's dynamic link with `ldd`, gives its SHA-256 as `74d07561e804f0701760f47d994095e9dc8bdf53fd53885df313d37ac7b6eb29`.
- `ros-humble-rmf-traffic-ros2` **2.1.8-1jammy.20260724.031602** supplied the `BlockadeNode` wrapper sources. `rook_blockade_tap` rebuilt those sources with recorder calls at the entry of the five inbound subscription callbacks. The capture-time source delta is preserved in `cells/rmf-blockade/tap/tap.patch` at fixture commit `054ad39c4c27d1431e78983bdb775368ff269f9d`, and `cells/rmf-blockade/tap/UPSTREAM.md` names and hashes the pristine 2.1.8 inputs. The rebuilt node's record-time SHA-256 in `BINSHA` is `62f82e9509f84932252521c684e89c7f19bd9b1232eb7889880acea74c99f8be`.
- `ros-humble-rmf-fleet-adapter` **2.1.8-1jammy.20260726.122542** supplied the mock traffic-light participants.
- `rmf_demos` was checked out at `d79ac6f03baeedbf8ee572347428b4a817098446`.

The installed deb versions match the pins used by ADR 0003. The recorder is `rook_blockade_tap` at `cells/rmf-blockade/tap/`. It ran in a `SingleThreadedExecutor`, making the callback-entry recording order the node's processing order (`cells/rmf-blockade/tap/src/main.cpp`). The recorder encodes each typed inbound message before calling the copied wrapper callback, and encodes each heartbeat at the publication callback (`cells/rmf-blockade/tap/src/Node.cpp` and `src/Recorder.cpp`). `cells/rmf-blockade/tap/build.sh` and `CMakeLists.txt` show that the rebuild links the installed `rmf_traffic` library rather than a modified moderator implementation.

The configuration used `min_conflict_angle` = 15°·π/180, the 2.1.8 wrapper's value, recorded in `params`. The moderator started empty, with a fresh tapped node before the first delivery.

The scenario used the `battle_royale` world with `use_traffic_light:=true`. The pinned mock traffic-light adapter consumes `/loop_requests`; `dispatch_patrol` publishes a different task API and could not command it. `request_loop` drove three robots on the A, C, and D routes. Over a 22.4-minute delivery span, `deliveries.bin` contains 948 callback-entry records from three participants, comprising 73 set, 413 ready, 411 reached, 2 release and 49 cancel. `heartbeats_recorded.bin` contains 2,275 heartbeats. `tools/sim_record_tap.sh` records the launch, traffic commands, binary hashing, and capture checks; the framing and counts above decode from the two fixture binaries. The recording ran an earlier script revision that launched the simulation before the tap; liveness and overlay guards were added later, so the capture checks and record-time `BINSHA`, rather than the current script revision, are the evidence for this fixture's validity.

## Recording limits

- A launch overlay removed the stock moderator node stanza and ran the tapped rebuild in its place. The node under test is therefore the rebuild of the byte-identical 2.1.8 wrapper sources plus `tap.patch`; the moderator decision library is the unmodified installed deb `.so` hashed in `BINSHA`.
- The recorder does not encode the wrapper's one-second heartbeat timer firings as replay inputs. It records the resulting heartbeat bytes. This is the same timer-input limit as the earlier fixture, now isolated because inbound callback order is recorded directly (`cells/rmf-blockade/tap/UPSTREAM.md`).
- Five of 912 recorded distinct states are absent from the raw whole-state LCS. Five timer heartbeats expose the same participant-2 `last_ready: 2 -> 3` status change after `ready(reservation=1, checkpoint=3)` when the assignment version did not change, so delivery-only replay emits no heartbeat at that point. Four windows end at participant 1 reaching checkpoint 3; one ends at participant 1 releasing checkpoint 3. ADR 0004 records the exact state indices, windows, and cause.

ADR 0004 gives the capture-fidelity argument. This file states the recorded facts and the measured residual only.
