# Nav2 timeout technical notes

Start with the [walkthrough](../../../README.md) to build and run the experiment. These notes explain the source and evidence behind the result. The cases recreate an upstream issue with scripted inputs; they carry no field-incident claim.

## Exact source

The old source is navigation2 `a3a97043ee93d92fe8d70ec56d183933169beb1c`, the immediate parent of the fix. The fixed source is `0f2777630471d03c65d73914c4b4a82224687a2c`, the squash merge of [upstream PR #6373](https://github.com/ros-navigation/navigation2/pull/6373). Both compile `nav2_behavior_tree/include/nav2_behavior_tree/bt_action_node.hpp`. Their SHA-256 values are `56696fb81ffcebff8103713fda84385565242954dd5d5b27108f62a5817bfbf8` and `14d46b024fb5ef73732ddd192d8a8f2f777e857044432818ad8777a05faf5115`, respectively. `prepare.py` checks these bytes before generating each translation unit's header.

BehaviorTree.CPP is compiled from commit `3ff6a32ba0497a08519c77a1436e3b81eff1bcd6` (4.9.0), archive SHA-256 `84486d12ce7249a611726dd4fbf4a43a9b2f3abf0b00448b6d910a3a54d9a281`. Its real action base class, status handling, blackboard, ports and wake-up implementation execute in both builds. The Jazzy environment supplies the real rclcpp node, callback group, time/duration arithmetic, action result structures and `nav2_msgs::action::Wait` message types.

The private BT dependency is excluded from the workspace install so it cannot override other packages' BehaviorTree.CPP dependency. This uses CMake 3.28 from the pinned Ubuntu Noble image. CTest selects `rmw_fastrtps_cpp` explicitly; middleware activity from node construction remains outside the comparison boundary.

The source header retains Intel's copyright and Apache-2.0 license notice. The build adds a modification notice. BehaviorTree.CPP and its bundled dependencies retain their upstream license files in the fetched source tree. The replay bundle retains that source and its notices alongside the executables. System package archives retain their installed copyright files. Rook's license, notices and dependency list are included with the runner.

## Integration boundary

`integration.diff` is the exact diff to review for both versions. Generation fails if that diff changes. It substitutes the lifecycle node wrapper, action client, goal handle and class-owned executor types, and removes unrelated Nav2 serializer includes that require newer ROS messages. The lookup helper preserves upstream port-first, blackboard-second behavior. JSON port parsing, lifecycle transitions and plugin loading are outside this experiment. The decision branches, predicates, elapsed-time arithmetic, callback bodies, tick and halt methods are the upstream source.

The controlled executor replaces the class-owned executor at its dependency boundary. It dispatches queued action callbacks and waits on real `std::shared_future` objects. Its clock observations come from the script. A separate worker retains the upstream call stack during a wait; a condition-variable handoff gives the caller exclusive access while the worker is suspended. A missing ROS or steady-clock observation leaves the corresponding read pending. A future completes only through a supplied acknowledgment callback, or times out after supplied steady-clock observations cross its wait deadline. No sleep or wall-time timeout drives the scenarios. Stopping the experiment unwinds a pending call without fulfilling its promise.

The timed-wait ordering follows Jazzy's [`Executor::spin_until_future_complete_impl`](https://github.com/ros2/rclcpp/blob/3aa906a2c7ad13d1623b31a55731993f56538e72/rclcpp/src/rclcpp/executor.cpp#L255) at source revision `3aa906a2c7ad13d1623b31a55731993f56538e72`. An already-ready future completes before reading steady time. Otherwise each iteration dispatches one eligible callback, checks readiness, then checks the deadline. The seam supports positive wait durations, monotonic nonnegative clock observations and an active ROS context. Shutdown interruption, recursive spinning and negative/infinite waits are unsupported. Scripted tick timestamps label the observation interval even when the class no longer reads time after acknowledgment.

This demonstrates substitutability and control of the class's dependencies. It does not demonstrate control of an unmodified rclcpp executor, DDS, ROS discovery or Nav2's action-client implementation. The action-client seam models the API's promises and callback delivery, and emits every goal/cancel request as the class calls it. Server availability is an explicit constant `true`. Goal IDs are supplied as distinct 16-byte values at issuance and retained with each promise. Transport success, server processing and goal termination are outside this boundary. A separate `cancel-ack` case supplies and checks an eligible cancellation response at the client API boundary. The existing ROS logging and BT wake-up internals are outside the comparison; their wall-clock metadata is not evidence.

## What the checks establish

The timeout scenario submits a goal, withholds its acknowledgment, advances both requested clock domains and observes the required wildcard cancellation on the fixed build while its cancellation-response wait remains pending. The old build returns failure without that request. The timely scenario processes an acknowledgment before the 20 ms deadline and observes ticks through 30 ms without a timeout cancellation. The fixed build must satisfy both checks. The no-op subclass disables submission through the existing `on_tick` hook and fails `goal_submitted`. The always-cancel subclass emits a wildcard cancellation unconditionally through the same hook and fails the timing and no-cancel checks. Both subclasses retain the compiled upstream class.

Additional checks cover the updated-goal timeout branch, overlapping goals with distinct supplied IDs, stale results while acknowledgment is pending, stale and wrong IDs after acknowledgment, a matching result, missing clock evidence and frozen steady time. They retain each issuance independently. The standalone checks do not apply goal-ID normalization. The integrated runner reports raw comparison beside its named normalization policy. Repeated execution compares the observed request bytes within the experiment; those observations are not independently recorded baseline effects for product verification.

The timeout property completes at a valid required cancel request. Its later cancellation acknowledgment and goal termination remain unavailable. The experiment's CTest exit status only indicates whether all expected behaviors were observed, including the broken build's expected failure. The integrated checks below use the runner's product exit codes 0/1/2/3 and report the recording grade separately.

## Labelled limitations

[Upstream #6426](https://github.com/ros-navigation/navigation2/issues/6426) describes halt-before-ack. Its author tested the fix and the maintainer confirmed that this path remains uncovered. The executable limitation check proves that both versions submit a goal and then halt without sending a cancellation while the goal handle is absent. This is separate from the tick timeout fixed by #6373.

PR #6373 also describes the cancel-before-goal server ordering limitation and the risk of cancelling other clients' goals. An all-zero ID is a wildcard. A request can leave the client before its goal is processed by the server, so observing the request cannot establish goal termination. The `cancel-before-goal` case uses a separate immutable termination property. The fixed build returns exit 3 naming the absent server ordering, cancellation acknowledgment and terminal result evidence. It does not simulate the server or claim that a particular server ordering was observed. The timeout request case can pass while this stronger claim remains unavailable.

## Reproduce in the shared Jazzy workspace

Run `./run.sh` from the repository root. It builds the package and runner in one disposable Jazzy container, runs CTest, and checks the eight captured cases. See [the workspace setup](../../README.md) for package sources and build output paths.

## Inspect the runner results

The integration directory contains `timeout-verify-0.json` for baseline agreement, `timeout-old.json` for the original behavior, and `timeout-fixed.json` for the candidate result. `matrix.json` collects the expected and observed candidate results from the successful integration run.

The timeout check expects `rook test` to return 1 for the old component and 0 for the fixed component. The fixed report still lists cancellation acknowledgment and goal termination as unavailable. Cases with missing required evidence use exit 3; a refusal is distinct from a passing behavioral check.

## Replay the retained evidence offline

Run `./replay.sh` from the repository root after a successful capture. The retained archive contains the exact runner and component executables, cases, adapter and property scripts, supporting source and notices, and package archives needed beyond the pinned base image. Replay restores their original paths inside a disposable container, preserving the case identities.

The `ros-jazzy` workflow also uploads a `nav2-replay` artifact and checks it on a separate runner. Actions artifacts expire; keep the bundle locally when preserving a particular result. The repository's source-build command is the primary walkthrough.
