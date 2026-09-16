# How the experiment runs

The experiment has a C++ component, a Python adapter, and a Rust runner. [The technical notes](native/src/rook_nav2_experiment/README.md) define the component boundary and the exact upstream source.

`prepare.py` fetches and hashes the old and fixed Nav2 headers, then verifies the dependency substitutions against `integration.diff`. CMake compiles both with the identified BehaviorTree.CPP source. Scripted callbacks and clock observations control the class while its original decision branches execute.

`capture.py` records inputs and independently observed effects from the old component. The `nav2_pack` Rust example packages that transcript into a case with source, binary, and dependency identities. `integrated.py` drives this preparation and the expected-result matrix.

`rook verify` executes the recorded baseline through the adapter and compares effects. `rook test` first verifies the baseline, then executes a candidate and checks its behavior using `property.py`. Raw comparison remains visible beside the named goal-ID normalization. A failed behavioral check and unavailable evidence have distinct results.

`bundle.py` retains the captured cases, binaries, source, notices, and packages changed from the base image. Its generated replay script restores their original paths inside a disposable container. `replay.sh` runs that script with networking disabled.

The Rust workspace also contains the Wasm replay path and its regression cases because the runner shares those crates. The Nav2 experiment uses the native path. Its measured agreement applies to the tested environment and component boundary. It does not establish robot safety or the origin of a field incident.

See the [native contract](docs/internals/native-contract.md), [format specification](docs/internals/format-spec.md), and [glossary](docs/glossary.md) for the underlying contracts. [Source provenance](SOURCE.md) distinguishes this experiment from historical Rook development notes.
