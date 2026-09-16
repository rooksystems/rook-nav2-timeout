# Evidence for the worked example

These files were copied without modification from the `nav2-replay` artifact produced by [CI run 35153353742](https://github.com/rooksystems/rook-nav2-timeout/actions/runs/35153353742) on September 16, 2026. They contain synthetic, scripted inputs and results from the demonstration. [The walkthrough](../result.md) explains what they show.

The run built repository commit [`2bf22b66da07d8fa945641bc1eef6a7588d9bd10`](https://github.com/rooksystems/rook-nav2-timeout/commit/2bf22b66da07d8fa945641bc1eef6a7588d9bd10) on Linux x86_64, inside ROS 2 Jazzy image `ros@sha256:2589a8fba5257307857890173c069852c2abf913a0be7970f172478baecb09e4`. The original replay archive's SHA-256 is `db9256e2ed8e84bdb37d6f61869e0dcb6ef5f6e3a44226f9ab9cb1b9b7c7b6ff`.

The run passed all 28 expected candidate statuses and replayed the retained bundle on a separate machine with networking disabled. These results describe that source and build. They do not establish that a newer build produces identical binaries, or that a field incident occurred.

## Files

- [timeout-capture.json](timeout-capture.json) contains the scripted inputs and directly captured old-component transcript.
- [timeout-old.json](timeout-old.json) records baseline agreement and the old candidate's failed cancellation check.
- [timeout-fixed.json](timeout-fixed.json) records the fixed candidate's cancellation request and passing check.
- [timely-fixed.json](timely-fixed.json) records the passing timely-acknowledgment check.
- [matrix.json](matrix.json) lists the 28 candidate results and their exit statuses.

Each timeout report carries case ID `7599a7630a75ce55844803d0c7a0a52f8c32494d08c62656033d546eb58fc188`. The full reports preserve their binary, source, and runtime dependency identities. Paths beginning with `/rook` and `/opt/ros/jazzy` refer to the original container; reading these files does not require those paths to exist on your machine.

## Check the copies

From this directory, run `sha256sum -c SHA256SUMS` on Linux or `shasum -a 256 -c SHA256SUMS` on macOS. [SHA256SUMS](SHA256SUMS) identifies the committed report bytes. It checks their integrity against this list; origin is documented by the linked CI run and source revision.

These are preserved example reports, not new test expectations. They are enough to inspect the result, but they are not a complete executable case bundle. To produce and replay your own bundle, follow [the running guide](../running.md). GitHub Actions artifacts expire; the committed reports remain available here.
