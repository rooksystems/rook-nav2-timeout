# Architecture

The Nav2 experiment runs through a C++ component, a Python adapter, and the Rust Rook runner. The [code tour](docs/code-tour.md) follows one case through those parts and links to their implementations.

The component compiles the old and fixed upstream `BtActionNode` with controlled dependencies. The capture script records inputs and observed effects from the old build. The adapter delivers those inputs again; the runner compares baseline effects and evaluates the candidate's behavioral check separately. The resulting bundle retains the executable environment for offline replay.

For the evidence behind one result, read [the timeout walkthrough](docs/result.md). For exact source identities, substitutions, and uncovered cases, read [the technical reference](native/src/rook_nav2_experiment/README.md). The [native contract](docs/internals/native-contract.md) and [format specification](docs/internals/format-spec.md) own the shared contracts.
