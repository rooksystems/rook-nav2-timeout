# A timeout with no cancellation request

This is a recorded run of the example, so you can inspect its result before building it. The [Linux CI run from September 16, 2026](https://github.com/rooksystems/rook-nav2-timeout/actions/runs/35153353742) built both versions, captured the old component's behavior, and tested the candidates. The JSON reports below are unmodified copies from that run.

## The input

The script submits a `Wait` goal and withholds the action server's acknowledgment. It supplies clock readings and ticks at 0, 10, and 20 ms. The goal-response deadline is 20 ms. These are scripted clock observations, not measurements of how quickly the test machine runs.

[The capture transcript](evidence/timeout-capture.json) contains the commands, supplied clock values, and the old component's responses. [The input script](../native/src/rook_nav2_experiment/capture.py) creates this scenario.

## Before the fix

The [old-build report](evidence/timeout-old.json) contains these observations in `observed_events`.

```text
Event 3     GoalSend    The component submits the goal.
Event 17    Clock       The component reads ROS time at 20 ms.
Event 18    Status      The component returns FAILURE.
                       No CancelSend event was observed.

Cancellation check     fail
rook test exit         1
```

Rook also replayed the original build and compared its output with the captured effects. `baseline_reproduced` is `true`, and `baseline_comparison.raw_effects_equal` is `true`. That establishes that this build reproduced the recorded behavior on the tested platform, including the missing cancellation request.

## With the upstream fix

The [fixed-build report](evidence/timeout-fixed.json) reaches the same deadline and emits a different event.

```text
Event 3     GoalSend    The component submits the goal.
Event 17    Clock       The component reads ROS time at 20 ms.
Event 18    CancelSend  The component requests cancellation.

Cancellation check     pass
rook test exit         0
```

The cancellation has an all-zero goal ID and timestamp, which this action API uses for a wildcard request. The [check implementation](../native/src/rook_nav2_experiment/property.py) requires a submitted goal, an observed expired deadline, and a valid cancellation request in that order. A candidate that never sends a goal fails; a candidate that cancels before the deadline also fails.

The report records cancellation acknowledgment and goal termination as unavailable. The component is still waiting for a cancellation response when this check completes. Passing establishes that it sent the required request. Server processing and whether the goal stopped need further evidence.

## Why the fixed report says "diverged"

`execution_agreement` compares the candidate's effects with the old recording. `property_result` evaluates the behavior we want from the fix. The fixed build changes those effects and passes the cancellation check, so this run correctly reports both divergence and a pass.

The test deliberately gives the fixed build a different goal ID. Raw comparison first differs at the goal request. After the named goal-ID normalization, `normalized_first_difference` is `3`, the zero-based fourth effect: the old build emits `Status`, while the fixed build emits `CancelSend`. Both comparisons remain in the report.

## A timely acknowledgment still passes

The [timely-acknowledgment report](evidence/timely-fixed.json) checks the other side of the deadline. The fixed build processes an acknowledgment before the deadline, then completes the scheduled ticks through 30 ms without cancelling. Its behavioral check passes with exit 0.

The [full result matrix](evidence/matrix.json) contains 28 candidate results across eight cases. Some cases deliberately fail or report insufficient evidence. The integration script checks each result against its expected status; a successful script does not mean every candidate passed its behavioral check.

## Inspect or reproduce the evidence

[Evidence provenance](evidence/README.md) identifies the source revision, platform, archive hash, and report checksums. To follow the explanation in JSON, start with `observed_events`, `baseline_comparison`, and `property_result` in either timeout report. Event ordinals identify order in the case; they are not elapsed time.

Run the example using [the README commands](../README.md#run-it-yourself), then find your own reports under `native/build/rook_nav2_experiment/integrated/`. Your build has its own binary and dependency identities. [The technical reference](../native/src/rook_nav2_experiment/README.md) explains the controlled class boundary and the evidence this experiment cannot provide.
