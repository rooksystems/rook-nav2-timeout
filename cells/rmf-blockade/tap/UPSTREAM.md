# Upstream wrapper provenance

The pristine files under `upstream/` came from `open-rmf/rmf_ros2` tag `2.1.8`, commit `48f375aff5f8bbc4d88ac2df1a2404b68eb40b52`.

- `rmf_traffic_ros2/include/rmf_traffic_ros2/blockade/Node.hpp`
- `rmf_traffic_ros2/include/rmf_traffic_ros2/StandardNames.hpp`
- `rmf_traffic_ros2/src/rmf_traffic_ros2/blockade/Node.cpp`
- `rmf_traffic_ros2/src/rmf_traffic_blockade/main.cpp`

[The upstream tree](https://github.com/open-rmf/rmf_ros2/tree/48f375aff5f8bbc4d88ac2df1a2404b68eb40b52/rmf_traffic_ros2) contains the originals.

`generate_patch.sh` generates `tap.patch` by comparing each modified pristine file with its build input. `StandardNames.hpp` has no patch hunk because it is unchanged. The tap-only `src/Recorder.hpp` and `src/Recorder.cpp` compare against `/dev/null`.

The tap records the five inbound subscription callbacks introduced in [ADR 0004](../../../docs/adr/0004-recorder-tap.md) and the wall-timer callback added in [ADR 0005](../../../docs/adr/0005-timer-inputs-close-experiment-2.md). At callback entry, it writes each timer firing as an empty channel-15 delivery. It records heartbeats before publishing them. All three output files use exclusive creation, so a new session cannot replace an existing recording.

## Pristine SHA-256 values

- `Node.hpp` has SHA-256 `ef1199b4389f41658742d54e6ff00fa5b3e4319cc738b7495dcfde0c8e03fec0`.
- `StandardNames.hpp` has SHA-256 `18d6be1585b3fa7ac87fcef531752918ed161027bf35084ba4f4137c016fe101`.
- `Node.cpp` has SHA-256 `88398e9d32465975d775d4605566fd25d1d9b97c1b7ff728a0e6b948428d16cb`.
- `main.cpp` has SHA-256 `f90922a738b0983a8ee0163e1bd6cb6cab666616c2c0e0b1ef53e574b37fd452`.
