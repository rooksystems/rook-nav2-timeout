# Recording provenance for battle-royale-timer

Recorded from 2026-08-19 through 2026-08-20 UTC on a Lambda `gpu_1x_a10` in `us-west-1` (Ubuntu 22.04 x86-64), inside a `ros:humble` Docker container, Gazebo Classic 11.10.2 headless. The container image was `ros@sha256:75dd3aba34a3838dadbb31a9f7bef769bdfa8713e6cec686fc868db2981b0987` (`RIG_NOTES.md` from the recording rig).

The recorded node used the following components.

- `ros-humble-rmf-traffic` **3.0.3-1jammy.20260304.203604** supplied the moderator decision code through the installed `/opt/ros/humble/lib/x86_64-linux-gnu/librmf_traffic.so`. `BINSHA`, written at record time after resolving the node's dynamic link with `ldd`, gives its SHA-256 as `74d07561e804f0701760f47d994095e9dc8bdf53fd53885df313d37ac7b6eb29`.
- `ros-humble-rmf-traffic-ros2` **2.1.8-1jammy.20260724.031602** supplied the `BlockadeNode` wrapper sources. `rook_blockade_tap` rebuilt those sources with recorder calls at the entry of the five inbound subscription callbacks and the one-second heartbeat wall-timer callback. The source delta is auditable at `cells/rmf-blockade/tap/tap.patch`, and `cells/rmf-blockade/tap/UPSTREAM.md` names and hashes the pristine 2.1.8 inputs. The rebuilt node's record-time SHA-256 in `BINSHA` is `9be4b96ff98fce4b7786866a363e98a7b71c53dfeefe4aa8afdcbd2895153d5b`.
- `ros-humble-rmf-fleet-adapter` **2.1.8-1jammy.20260726.122542** supplied the mock traffic-light participants.
- `rmf_demos` was checked out at `d79ac6f03baeedbf8ee572347428b4a817098446`.

The installed deb versions match the pins used by ADR 0003. The recorder is `rook_blockade_tap` at `cells/rmf-blockade/tap/`. It ran in a `SingleThreadedExecutor`, making callback-entry recording order the node's processing order (`cells/rmf-blockade/tap/src/main.cpp`). The recorder encodes each typed inbound message before calling the copied wrapper callback. Each wall-timer firing is an empty channel-15 delivery recorded before heartbeat construction, and each heartbeat is recorded before publish (`cells/rmf-blockade/tap/src/Node.cpp` and `src/Recorder.cpp`). The delivery, heartbeat, and parameter files use exclusive creation so this session could not overwrite an earlier recording. `cells/rmf-blockade/tap/build.sh` and `CMakeLists.txt` show that the rebuild links the installed `rmf_traffic` library rather than a modified moderator implementation.

The configuration used `min_conflict_angle` = 15°·π/180, the 2.1.8 wrapper's value, recorded in `params`. The moderator started empty, with a fresh tapped node before the first delivery.

The scenario used the `battle_royale` world with `use_traffic_light:=true`. The pinned mock traffic-light adapter consumes `/loop_requests`; `request_loop` drove three robots on the A, C, and D routes. From 2026-08-19 23:38:00.715436 UTC through 2026-08-20 00:00:49.727118 UTC, a 1,369.012-second span, `deliveries.bin` contains 2,317 callback-entry records from three participants, comprising 72 set, 415 ready, 410 reached, 1 release, 49 cancel and 1,370 heartbeat-timer records. Every channel-15 payload is empty; consecutive timer records are 1.000008515 seconds apart on average (minimum 0.999478840, maximum 1.000362556). `heartbeats_recorded.bin` contains 2,275 heartbeats. `tools/sim_record_tap.sh` records the launch, traffic commands, binary hashing, and capture checks; the script committed for that recording was byte-identical to the copy that ran on the rig (SHA-256 `b5142f12b959ab660fd9c79173405fa197fd6eb942daacad0ad756c83b8aa875`). The framing and counts above decode from the two fixture binaries.

## Recording limits

- A launch overlay removed the stock moderator node stanza and ran the tapped rebuild in its place. The node under test is therefore the rebuild of the byte-identical 2.1.8 wrapper sources plus `tap.patch`; the moderator decision library is the unmodified installed deb `.so` hashed in `BINSHA`.
- This is the fit window for the upstream blockade cell. The separate held-out window was neither recorded nor observed in this session.

ADR 0005 gives the capture-fidelity argument. This file states the recorded facts. The measured fit result is a raw whole-state LCS of 911/911 with no reconstruction policy.
