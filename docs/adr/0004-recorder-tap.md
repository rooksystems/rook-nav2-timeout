# ADR 0004 - Recording the moderator's inbound callbacks

Accepted on 2026-08-19. The experiment used the ADR 0003 Ubuntu x86_64 recording environment, a ROS Humble container and Gazebo Classic 11.10.2. The cell remained unchanged from ADR 0002 and the bag replay path from ADR 0003.

## Decision

Record each of the node's five inbound subscription callbacks before invoking the moderator. This captures processing order directly. The new recording reached 907/912 distinct full states, or 99.45%, without reconstruction. The remaining five states were exposed by heartbeat-timer firings absent from the input recording.

The grade remains Inferred because an input class is missing. Recording the timer callback is the next requirement for a Complete recording of this boundary.

## How the tap observes the node

The tap rebuilds the `BlockadeNode` 2.1.8 wrapper and links the installed `librmf_traffic.so` 3.0.3 decision library. [The tap provenance](../../cells/rmf-blockade/tap/UPSTREAM.md) identifies pristine wrapper sources from commit `48f375af` and the recorder patch. The recording's `BINSHA` identifies the running binaries, resolved through their dynamic links at capture time.

The node uses a `SingleThreadedExecutor`. Recording at callback entry therefore preserves its processing order. Ticks use wall nanoseconds forced strictly increasing. The existing wire reader accepted the tap output without changes.

This tap is C++ beside the cell because it integrates with rclcpp. Its encoder was compared with the Python writer and Rust reader during development. The committed fixture provides a replay regression check; a dedicated ROS test rig for the recorder remained unimplemented at this point.

## Evidence

The [callback fixture](../../cells/rmf-blockade/fixtures/battle-royale-tap/PROVENANCE.md) contains three simulated participants on crossing A/C/D routes over 22.4 minutes. It has 948 deliveries, comprising 73 `set`, 413 `ready`, 411 `reached`, two `release` and 49 `cancel` messages, plus 2,275 recorded heartbeats. A preceding four-minute check reached 175/175 states.

Two fresh cell instances produced equal replay hashes.

- `bag_run_hash` was `b939a1bbd3a348d733978dd83440ea85b7798d2edfc4c6156d1405feb4b30149`.
- `bag_output_digest` was `eb7735f4b442211c83d1698b60dabaf6243fbfb12ff6c79ae8a4fc6c76b03d56`.

Raw agreement was 907/912 states and 906/911 transitions. Applying `P1_lifecycle` moved 21 cancellations already in processing order and reduced agreement to 905/912 states, with per-participant agreement of 866/867. The result shows why this external-recorder reconstruction policy must not be applied indiscriminately to callback recordings. Both measurements remain in the [tap golden](../../cells/rmf-blockade/goldens/battle-royale-tap.golden).

The full gate ran on the Ubuntu recording host that day and matched the hashes for this fixture and ADR 0003's external-topic fixture.

## The missing timer input

The five unmatched state occurrences were at recorded indices 190, 244, 298, 352 and 586. Each showed participant 2's `last_ready` advancing from 2 to 3. That input did not change the assignments version, so the subscription callback emitted no heartbeat. The node's one-second timer published the state before a later input changed it again.

Replay stepped only on recorded deliveries and skipped those observations. Removing exactly those five occurrences made the recorded and replayed distinct-state sequences byte-identical. The diagnosis predicted that recording timer firings would remove the gap.

The cell already understood the heartbeat-timer channel. Capturing it required a write at timer callback entry and a reader change allowing an empty channel-15 payload. [ADR 0005](0005-timer-inputs-close-experiment-2.md) records that follow-up experiment.

## Recorder defects identified here

The tap source for this recording was kept with its provenance while the next recording was prepared. Two defects needed correction before the next run. Heartbeats were recorded after publication, leaving a crash window in which a subscriber could see an unrecorded frame. Output files were truncated on open, allowing a restarted process to replace evidence.

The next recorder needed record-before-publish and exclusive file creation. This fixture's single-session claim relied on the recording rig's process-liveness checks. A session marker and general recorder continuity support remained separate requirements.

## Practical constraints

The mock fleet accepted `/loop_requests` through `request_loop`; `dispatch_patrol` did not reach it. The rig launched the tap binary directly because `ros2 run` retained SIGINT during shutdown, and cold-cycled the container to clear detached simulator children.

A launch-overlay copy needed symlinks dereferenced so edits would not reach back into the source workspace. ROS setup scripts also required care around `set -u`, and colcon's `--log-base` and `--merge-install` options had to match the expected layout. These findings belong to reproducing this rig, rather than to the replay guarantee.
