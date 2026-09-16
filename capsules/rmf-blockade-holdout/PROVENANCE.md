# Recording provenance for battle-royale-holdout

This holdout window was recorded after the replay code freeze. The first cold replay was recorded as the golden expectation regardless of its result. Subsequent gate runs compare against that frozen expectation.

Recorded on 2026-08-20 UTC on a Lambda `gpu_1x_a10` in `us-west-1` (Ubuntu 22.04 x86-64), inside a `ros:humble` Docker container, Gazebo Classic 11.10.2 headless. The container image was `ros@sha256:75dd3aba34a3838dadbb31a9f7bef769bdfa8713e6cec686fc868db2981b0987` (`RIG_NOTES.md` from the recording rig).

The recorded node used the following components.

- `ros-humble-rmf-traffic` **3.0.3-1jammy.20260304.203604** supplied the moderator decision code through the installed `/opt/ros/humble/lib/x86_64-linux-gnu/librmf_traffic.so`. `BINSHA`, written at record time after resolving the node's dynamic link with `ldd`, gives its SHA-256 as `74d07561e804f0701760f47d994095e9dc8bdf53fd53885df313d37ac7b6eb29`.
- `ros-humble-rmf-traffic-ros2` **2.1.8-1jammy.20260724.031602** supplied the `BlockadeNode` wrapper sources. `rook_blockade_tap` rebuilt those sources with recorder calls at the entry of the five inbound subscription callbacks and the one-second heartbeat wall-timer callback. The source delta is auditable at `cells/rmf-blockade/tap/tap.patch`, and `cells/rmf-blockade/tap/UPSTREAM.md` names and hashes the pristine 2.1.8 inputs. The rebuilt node's record-time SHA-256 in `BINSHA` is `9be4b96ff98fce4b7786866a363e98a7b71c53dfeefe4aa8afdcbd2895153d5b`.
- `ros-humble-rmf-fleet-adapter` **2.1.8-1jammy.20260726.122542** supplied the mock traffic-light participants.
- `rmf_demos` was checked out at `d79ac6f03baeedbf8ee572347428b4a817098446`.

The installed deb versions match the pins used by ADR 0003. The recorder is `rook_blockade_tap` at `cells/rmf-blockade/tap/`. It ran in a `SingleThreadedExecutor`, making callback-entry recording order the node's processing order (`cells/rmf-blockade/tap/src/main.cpp`). The recorder encodes each typed inbound message before calling the copied wrapper callback. Each wall-timer firing is an empty channel-15 delivery recorded before heartbeat construction, and each heartbeat is recorded before publish (`cells/rmf-blockade/tap/src/Node.cpp` and `src/Recorder.cpp`). The delivery, heartbeat, and parameter files use exclusive creation so this session could not overwrite an earlier recording. `cells/rmf-blockade/tap/build.sh` and `CMakeLists.txt` show that the rebuild links the installed `rmf_traffic` library rather than a modified moderator implementation.

The configuration used `min_conflict_angle` = 15°·π/180, the 2.1.8 wrapper's value, recorded in `params`. The moderator started empty, with a fresh tapped node before the first delivery. The fit recording remained untouched at `/home/ubuntu/data/timer-input-20260819-233049/`.

The scenario used the `battle_royale` world with `use_traffic_light:=true`. The pinned mock traffic-light adapter consumes `/loop_requests`; `request_loop` drove four robots on the A, B, C, and D routes with `--delay 2`. One additional A-route loop request was dispatched five minutes into the timed window for extra task churn. The timed window was 13 minutes. From 2026-08-20 00:19:11.610392 UTC through 2026-08-20 00:32:59.617483 UTC, an 828.007-second recorded span, `deliveries.bin` contains 1,516 callback-entry records from four participants, comprising 45 set, 309 ready, 298 reached, 0 release, 35 cancel and 829 heartbeat-timer records. Every channel-15 payload is empty; consecutive timer records are 1.000008539 seconds apart on average (minimum 0.999651153, maximum 1.000377230). `heartbeats_recorded.bin` contains 1,495 heartbeats.

`tools/sim_record_tap.sh` records the launch, traffic commands, binary hashing, and capture checks. The executed holdout script was authored before recording and was byte-identical to the script committed for that recording (SHA-256 `d750c831264f0a4d486663d062a8edb0f8516529842bd548968f4f23cc743a86`). It requires at least four participants, adds the B route, changes every request-loop delay to 2, and performs the mid-run dispatch. Commit `53cc452`'s final timestamp, 00:19:27 UTC, postdates capture start by 16 seconds. The container was stopped and restarted before launch; no Gazebo or ROS processes survived the stop or existed after the cold start before launch. The framing and counts above decode from the two fixture binaries.

## Recording limits

- A launch overlay removed the stock moderator node stanza and ran the tapped rebuild in its place. The node under test is therefore the rebuild of the byte-identical 2.1.8 wrapper sources plus `tap.patch`; the moderator decision library is the unmodified installed deb `.so` hashed in `BINSHA`.
- This is the held-out window for the upstream blockade cell. The replay code, prebuilt Wasm, prior fixtures, and prior goldens were frozen before this window was recorded. The capture was transferred and SHA-256 checked before the one cold replay. No replay, diagnosis, or adjustment preceded it.

ADR 0005 gives the capture-fidelity argument. The one cold replay produced raw whole-state LCS 672/672 (100%) with no reconstruction policy. The complete observed output is pinned in `cells/rmf-blockade/goldens/battle-royale-holdout.golden`.
