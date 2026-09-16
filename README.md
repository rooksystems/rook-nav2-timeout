# Reproduce a Nav2 action timeout

Run the old and fixed versions of Nav2's `BtActionNode` against the same scripted inputs, then check whether each sends a cancellation request when a goal acknowledgment times out.

This is an early Rook experiment based on [Navigation2 PR #6373](https://github.com/ros-navigation/navigation2/pull/6373). It executes the upstream class with controlled action callbacks and clock observations. You can reproduce the old behavior, test the fix, and replay the captured cases without a robot.

## Try it

You need Git, Docker Engine or Docker Desktop, and an internet connection for the first build. Start Docker before running the commands. ROS, C++, Python, and Rust run inside the container. The first run downloads dependencies and compiles both versions and the runner; allow several gigabytes of disk space. Build time depends on your machine and connection.

```sh
git clone https://github.com/rooksystems/rook-nav2-timeout.git
cd rook-nav2-timeout
./run.sh
```

The tested environment is Linux x86_64 with ROS 2 Jazzy in a digest-pinned Ubuntu container. Docker Desktop on Apple silicon can emulate this image, but that host configuration has not been verified for this experiment.

When the experiment succeeds, its final summary includes these results.

```text
timeout / old: FAIL (rook test exit 1)
timeout / fixed: PASS (rook test exit 0)
```

The old version's failure is the expected result. The overall script exits successfully only when every integration check returns its expected result. It also checks timely acknowledgments, overlapping goals, invalid responses, and missing evidence. A full JSON report for each check is written under `native/build/rook_nav2_experiment/integrated/`.

## What happened

The script submits a goal and withholds its acknowledgment. It supplies clock observations past the 20 ms goal-response deadline. The old class returns failure without requesting cancellation. The fixed class issues the required wildcard cancellation request. A separate timely-acknowledgment scenario checks that it avoids cancelling when the goal was acknowledged on time.

Rook first captures the old component's effects, then replays the captured inputs in fresh processes and compares the effects. It tests each candidate against the case's behavioral check separately. A fix is allowed to change behavior; passing requires satisfying that check.

The fixed result establishes that the component issued a cancellation request. The experiment cannot establish that a server acknowledged the request or that the goal terminated. [The technical notes](native/src/rook_nav2_experiment/README.md) explain the exact boundary, source identities, comparison policy, and uncovered cases.

## Replay without networking

After the first successful run, use the retained binaries and package archives to repeat all eight cases in a fresh container with networking disabled.

```sh
./replay.sh
```

This verifies the archive's SHA-256 and restores the captured environment inside a disposable container. It does not rebuild or recapture. The output includes JSON reports; exit 0 means all expected baseline and candidate statuses matched. The base image must still be present locally.

Keep `native/build/rook_nav2_experiment/` if you want to preserve a particular run. `./run.sh` refuses to overwrite an existing captured run. Use a fresh checkout to capture another build.

## How this relates to rosbag

This example uses scripted inputs for one class. It does not import your bags or reproduce an arbitrary ROS failure. Applying this approach to another component requires an adapter and enough recorded inputs, clock observations, and starting state to execute that component again.

If rosbag already covers your debugging needs, the useful question here is whether a repeatable test of a component's decisions would help with a specific remaining problem. Feedback about the setup, the result, or the evidence your own component would need is welcome in [an issue](https://github.com/rooksystems/rook-nav2-timeout/issues). Describe the behavior with synthetic examples; keep customer recordings and private logs local.

## Troubleshooting

If Docker cannot connect, start the daemon or Docker Desktop and check `docker info`. If compilation is killed, check Docker's memory limit and available disk space before retrying. Apple silicon builds use amd64 emulation and may take longer.

If a download or package installation fails, retain the command and error output. The build uses a dated, signed ROS snapshot and public upstream archives. It refuses missing dependencies or a source hash mismatch. Ubuntu packages still come from their configured repositories; each run records installed versions and retains changed packages for offline replay.

If you see a result different from the expected summary, include your host OS, architecture, repository revision, command, and the relevant synthetic report in an issue. A passing replay only establishes agreement for the recorded component boundary and build.

## Source and development

[Source provenance](SOURCE.md) explains the supporting Rook code included here. [Contributing](CONTRIBUTING.md) covers tests and changes. The [architecture](ARCHITECTURE.md) introduces the runner and adapter. [Security](SECURITY.md) describes execution boundaries and private reporting.

Rook code is licensed under [Apache-2.0](LICENSE). [NOTICE](NOTICE) and [THIRD-PARTY.md](THIRD-PARTY.md) identify other licenses. Nav2 and BehaviorTree.CPP source is fetched from identified public revisions and retains its upstream notices.
