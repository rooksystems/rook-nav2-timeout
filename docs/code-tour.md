# Follow one case through the code

Start with the [recorded timeout result](result.md). The files below produce that result and explain the main boundaries in the implementation.

## 1. Supply the failing inputs

[`capture.py`](../native/src/rook_nav2_experiment/capture.py) starts the old component, submits a goal, and supplies commands and clock observations. Its `timeout` scenario withholds the acknowledgment and advances ROS time to the 20 ms deadline. It captures the component's effects directly, independently of the later replay comparison and behavioral check.

[`component.cpp`](../native/src/rook_nav2_experiment/component.cpp) wraps the compiled upstream class in a process that accepts these commands. [`control.hpp`](../native/src/rook_nav2_experiment/control.hpp) provides the controllable action client, executor, and clocks. This substitution is what lets the script choose callback delivery and clock observations.

The class's decision branches execute from the upstream Nav2 header. [`prepare.py`](../native/src/rook_nav2_experiment/prepare.py) verifies the fetched header bytes and the substitutions listed in [`integration.diff`](../native/src/rook_nav2_experiment/integration.diff). [The technical reference](../native/src/rook_nav2_experiment/README.md#integration-boundary) explains exactly what runs upstream and what is controlled.

## 2. Preserve a case

[`nav2_pack.rs`](../crates/rook-verify/examples/nav2_pack.rs) packages the captured transcript with its original effects, source and executable identities, dependency identities, and behavioral check. It serializes observations; it does not run the component or invent its outputs.

[`integrated.py`](../native/src/rook_nav2_experiment/integrated.py) drives capture and packaging. It then calls the runner repeatedly to check the baseline and the candidate matrix. The same captured input recording is used throughout those comparisons.

## 3. Reproduce the original behavior

[`adapter.py`](../native/src/rook_nav2_experiment/adapter.py) delivers recorded commands, clock reads, and responses to the component. The Rust [`runner.rs`](../crates/rook-verify/src/runner.rs) checks identities and compares the resulting effects with the recording when you run `rook verify`.

Baseline agreement answers whether this build reproduced the captured behavior. It can reproduce a bug perfectly. That is why the candidate's behavioral check is a separate result.

## 4. Check the candidate

[`property.py`](../native/src/rook_nav2_experiment/property.py) owns the cancellation check. In its timeout branch, a passing result requires a goal request, a consumed clock observation at or beyond the deadline, and a valid wildcard cancellation request after expiry. An unconditional cancellation or a component that submits no goal fails.

`rook test` verifies the baseline before evaluating the candidate. The [`normalization.rs`](../crates/rook-verify/src/normalization.rs) policy aligns goal identities by issuance; raw comparison stays visible. The [worked result](result.md#why-the-fixed-report-says-diverged) shows why a fixed candidate can diverge from the old recording and pass its behavioral check.

## 5. Keep the run executable

[`bundle.py`](../native/src/rook_nav2_experiment/bundle.py) retains cases, binaries, supporting source and notices, and packages changed from the base image. [`replay.sh`](../replay.sh) restores this bundle inside a disposable container with networking disabled. It preserves the original paths so the case identities still describe the executed files.

## The rest of the repository

`native/src/rook_nav2_experiment/` contains the ROS experiment. `crates/` contains the Rook runner and shared Rust code. The `cells/`, `capsules/`, and `fixtures/` directories support that workspace's regression tests, including its Wasm path; you can leave them aside while reading the Nav2 example.

The historical ADRs explain earlier Rook decisions. Some refer to a private issue tracker, as [source provenance](../SOURCE.md) describes. They are not prerequisites for running or understanding this example. The [format specification](internals/format-spec.md) and [native contract](internals/native-contract.md) define the underlying bytes and execution rules.
