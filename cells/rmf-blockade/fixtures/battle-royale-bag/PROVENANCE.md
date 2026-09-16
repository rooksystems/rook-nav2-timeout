# Recording provenance for battle-royale-bag

Recorded 2026-08-18 on a Lambda `gpu_1x_a10` (Ubuntu 22.04 x86-64), inside a `ros:humble` Docker container, Gazebo Classic 11 headless.

The recorded node used the following components.

- `ros-humble-rmf-traffic` **3.0.3-1jammy.20260304.203604** (the moderator). Its `blockade/` sources are **byte-identical** to the cell's vendored commit `39f09e79` (`git diff 3.0.3..39f09e79 -- blockade` is empty), so no conformance gap is attributable to moderator code drift.
- `ros-humble-rmf-traffic-ros2` **2.1.8-1jammy.20260724.031602** (the `BlockadeNode` wrapper). Delta versus the vendored wrapper source is QoS depth (`SystemDefaultsQoS()` vs `.keep_last(10)`) and log format strings only. They did not change decision logic.
- `ros-humble-rmf-fleet-adapter` 2.1.8 (mock traffic light participants).
- rmf_demos humble @ `d79ac6f03baeedbf8ee572347428b4a817098446`.

The configuration used `min_conflict_angle` = 15°·π/180, hard-coded in 2.1.8's `blockade::make_node` (verified in that tag's source); recorded in `params`. The moderator started empty, before recording began.

The scenario used the `battle_royale` world, `use_traffic_light:=true`, three robots looping `start_{A,C,D}` ↔ `goal_{A,C,D}` via `/loop_requests` for 21.6 min. A fourth loop request was dropped twice by the task layer; tinyRobotB stayed parked. The recorder was `tools/rook_record.py` at the topic boundary.

## Recording limits

ADR 0003 grades this recording Inferred. The recorder observes DDS topics; the node's callback order must be reconstructed across topics. The node's binary identity is supported by package metadata and a source comparison, without a binary hash captured during recording.
