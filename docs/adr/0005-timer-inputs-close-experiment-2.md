# ADR 0005 - Recording timer inputs completes the replay boundary

Accepted on 2026-08-20. The experiment used the Ubuntu recording host, container and package pins from ADR 0004. The cell and prebuilt Wasm remained unchanged from ADR 0002.

## Decision

Record every heartbeat-timer firing as an input. The fit recording then reproduced all 911 distinct states. With the replay path frozen, a separate held-out recording reproduced all 672 states on its first reported replay. This met experiment 2's requirement of more than 99% conformance on a held-out recording of the running node.

Both recordings are graded Complete for the declared moderator boundary. The recorder captured the five message channels and the timer callback. Here, exact agreement means equal ordered heartbeat payloads, including repeated payloads. Recorded heartbeat timestamps and replay emission ticks are separate observations.

## Changes required

The tap writes an empty channel-15 record at heartbeat-timer callback entry, whether or not that firing emits a heartbeat. It now records heartbeats before publishing them and creates output files exclusively, addressing the crash-window and truncation defects identified in ADR 0004.

The bag reader accepts channel 15 only with an empty payload. Other channels keep their eight-byte minimum. The reconstruction policy ignores timer records, and `BAG_TRACE_DOMAIN` remains unchanged. The cell's synthetic scenario already treated timer firings as inputs.

## Fit recording

The [fit fixture](../../cells/rmf-blockade/fixtures/battle-royale-timer/PROVENANCE.md) uses three simulated participants on the A/C/D routes. It contains 2,317 deliveries, comprising 72 `set`, 415 `ready`, 410 `reached`, one `release`, 49 `cancel` and 1,370 timer firings. The average timer interval is 1.000009 seconds. The 22.8-minute recording contains 2,275 heartbeats.

Raw distinct-state agreement was 911/911.

- `bag_run_hash` was `160c40bbeba1210cf09cea3cbc882f6fa03619f9cb18e79d39fe6d14a14ac532`.
- `bag_output_digest` was `e593b7be00667fe2bd8ad07953ad83f688e500eb30e03ab7f1adc7e448e3ee6a`.

## Held-out recording

The [holdout provenance](../../cells/rmf-blockade/fixtures/battle-royale-holdout/PROVENANCE.md) records the procedure after the fit golden was committed and the replay path frozen. The scenario added a fourth robot and the B route, used request-loop delay 2 instead of 5, and dispatched an additional loop during the run. The first replay result became the golden without adjustment.

The recording contains 1,516 deliveries, comprising 45 `set`, 309 `ready`, 298 `reached`, no `release`, 35 `cancel` and 829 timer firings. It has 1,495 heartbeats from four participants over an approximately 13-minute recording window.

Raw distinct-state agreement on the first reported replay was 672/672.

- `bag_run_hash` was `99ba8401de4a3aee378e336b6152c8a279fd645a2372d3f197170f86d72ec4e5`.
- `bag_output_digest` was `8265ec615fe456a718d18ffeb8018dde931ec1044f4df2b33dc594cae22aad22`.

## What the checks establish

An LCS equal to the recorded state count can still hide extra replay states. Review caught that one-sided check, so the gate also pins `replay_states`, `replay_heartbeats` and `heartbeats_identical`. Both new fixtures reproduce their entire heartbeat payload streams, with 2,275 payloads for fit and 1,495 for holdout.

The older fixtures retain their measured shortfalls. For example, the external recording replays 449 heartbeats against 2,114 recorded heartbeats and has `heartbeats_identical = 0`. The reconstruction policy does not change conformance on either complete recording.

The five timer-visible states missing in ADR 0004 are accounted for by the new input class. The full gate ran on the Ubuntu recording host and matched all five goldens, including the two new fixtures. One-count changes to the new expectations were also observed to fail the gate during the original experiment.

## Limits and follow-up

This result establishes replay of one decision component under the tested recording contract. The experiments compare external-topic recording, callback recording with a missing timer, and complete callback recording. They do not exercise every recording grade or establish the cost of integrating another component.

The holdout procedure is documented, but repository contents alone cannot prove that no earlier replay occurred. Preregistering the recording hash would strengthen a future experiment. An independent wasmi run, a dedicated ROS recorder test rig and general recording-session support remained untested at this point.

The HTML renderer is a diagnostic report for these fixtures. [ADR 0006](0006-public-capsule.md) follows with a standalone verifier and the separate question of whether an outside engineer can check the result.
