# Contributing

This repository maintains one Nav2 timeout experiment. Open an issue for a setup failure, an unclear result, or a proposed change to its recording boundary. Include the revision, platform, command, expected result, and actual result. Use synthetic inputs and follow [SECURITY.md](SECURITY.md) for suspected vulnerabilities.

## Check a change

Run `./run.sh` from a fresh checkout to build the ROS package, run CTest, capture the cases, and test the candidate matrix. Run `./replay.sh` afterward to check the retained evidence with networking disabled. The `ros-jazzy` workflow runs these checks on Linux x86_64 and repeats offline replay on a separate runner.

For Rust or documentation changes, install [rustup](https://rustup.rs/), [just](https://github.com/casey/just#installation), Python 3, and your platform's C compiler and linker, then run `just gate`. The repository pins the Rust toolchain. The gate checks the runner and its supporting Rust workspace, committed replay expectations, licenses, prose, and local links. The Linux and macOS jobs compare the bundled Wasm case's hashes across architectures; these checks do not establish native agreement on macOS.

Keep `ROOK_WRITE_FIXTURES` unset. Existing expected bytes, hash domains, the format specification, and the `rook-core` dependency list require explicit maintainer approval for the named files before changes. Never regenerate expectations to hide a failed check.

## Submit a change

Keep the change focused and explain the behavior it affects. Include the commands and results you checked. Separate baseline replay agreement from the candidate's behavioral result. State what remains unverified. Every ROS change needs the `ros-jazzy` workflow to pass before merge.

Contributions use Apache-2.0 with no separate contributor agreement. Preserve third-party notices and submit only material you have the right to license. Maintainer review is required before merge.
