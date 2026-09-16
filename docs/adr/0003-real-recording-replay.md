# ADR 0003 - Recording the blockade node from DDS topics

Accepted as a partial result on 2026-08-18. The recording ran on Ubuntu 22.04 x86_64 in a ROS 2 Humble container, with Gazebo Classic 11 and the humble `rmf_demos` scenario. The cell and host came from ADR 0002.

## Decision

Move recording closer to the moderator's input callbacks. The external topic recording reproduced 13.1% of the recorded distinct full states in its observed order. A named reconstruction policy raised that to 73.1%, with 90.77% agreement across participant state sequences. The recording did not establish the callback order required for exact replay.

The recording's grade is Inferred because the reconstruction supplies an order that was not captured. Repeating replay was deterministic, but deterministic execution alone did not establish agreement with the original node.

## Recording and measurement

Three simulated robots followed crossing routes in `battle_royale` through the mock traffic-light adapter. Over a 21.6-minute simulated span, the topic recorder captured 917 deliveries and 2,114 heartbeats. Deliveries comprised 63 `set`, 443 `ready`, 367 `reached` and 44 `cancel` messages. The task layer dropped the fourth robot's loop request twice.

[The fixture provenance](../../cells/rmf-blockade/fixtures/battle-royale-bag/PROVENANCE.md) records the environment and configuration. `tools/rook_record.py` wrote the wire records with receive-time nanoseconds as ticks, under `P0_ingest`. An earlier rosbag2 attempt recorded heartbeats but missed best-effort robot publishers because its subscriptions did not match them.

Two fresh cells replaying the raw recording produced equal hashes.

- `bag_run_hash` was `b3c2d17a00078853a8e39dddc7e6bb1ccb6410baec41c1f51ae8c3f3128f3b2b`.
- `bag_output_digest` was `89ac053efa65accbc8fe5f18686c6f62cccf14c75f5ae36ee1672c4d4e5ed352`.

Conformance used the longest common subsequence of distinct decision states. Participant statuses were sorted, consecutive duplicates collapsed, and both streams included the moderator's initial empty state.

- Raw full-state agreement was 106/807, or 13.1%. Participant agreements were 22.9% for p5, 71.2% for p6 and 63.2% for p7.
- After `P1_lifecycle`, full-state agreement was 590/807, or 73.1%. Participant agreements were 97.7%, 95.8% and 76.3%, respectively, with an aggregate 698/769, or 90.77%.

The gate preserves these values in the [bag golden](../../cells/rmf-blockade/goldens/battle-royale-bag.golden). At the time of this ADR, the fixture hashes had been checked on macOS ARM64. The Linux host had run the earlier synthetic experiments but had not yet run this bag path.

## Why order was lost

The participants reused reservation ID 1 across `set`, `cancel` and the next `set`. Those messages travel on different DDS topics. Ordering within a topic does not establish the moderator's order across topics, so the external recorder could observe a `set` before the `cancel` processed before it by the moderator.

`Moderator::set` rejects a modularly stale reservation. Swapping that pair therefore changes how the rest of the loop's messages are handled. Participant p5's recording begins `set, set, cancel`; correcting that lifecycle order explains the largest improvement.

`P1_lifecycle` moves a `cancel` before the preceding `set` when no intervening `ready` or `reached` belongs to that participant. It moved 36 of 44 cancellations. Finer cross-topic and cross-participant ordering remained unresolved. For p5, the reconstructed replay state sequence was an exact subsequence of the recorded one, with transitions omitted rather than additional states introduced.

The identity investigation found the installed `rmf_traffic` 3.0.3 blockade sources identical to the vendored component. The wrapper differences concerned QoS depth and logging, and the recorded configuration supplied the 15-degree conflict angle. The record-time binary hash and actual callback order remained unavailable. Every `ready` and `reached` evidenced by the recorded moderator heartbeats was present in the input recording; this check did not establish completeness for every possible input class.

## Integration findings

Preparing the recording took about three hours. A clean ROS container avoided the host image's protobuf conflict with Gazebo. Undeclared Python dependencies prevented the fleet manager from starting, and task requests sent before bidding was ready were dropped.

The initial office scenario used schedule negotiation rather than blockade coordination, so its moderator had no clients. The mock traffic-light scenario supplied the component traffic this experiment needed. Before recording, verify both the component boundary and the task API that actually reaches it.

## Consequence

The external recording remains a regression fixture for a known incomplete observation. Its reconstruction policy is explicit and preserves the measured shortfall. [ADR 0004](0004-recorder-tap.md) follows the decision to record callback entry, where the node's processing order can be observed directly.
