# Contributing

This repository maintains one Nav2 timeout experiment. Setup fixes, clearer explanations, and defects in its capture or checks are useful contributions. Discuss a new component or a change to the recording boundary in an issue before implementing it.

## Report a problem

Include your repository revision, host OS and architecture, the command you ran, what you expected, and what happened. A small synthetic reproduction or relevant report helps. Keep private recordings local. Use [the private reporting channel](SECURITY.md#reporting) for suspected vulnerabilities.

## Find the relevant code

The [code tour](docs/code-tour.md) connects the input script, component, adapter, runner, and behavioral check. [The worked result](docs/result.md) shows the output they produce. Start with the part that owns the behavior you want to change.

## Check your change

For a ROS change, run `./run.sh` from a fresh checkout, then `./replay.sh`. This builds the component, runs CTest, checks all captured cases, and replays the retained bundle without networking. [The running guide](docs/running.md) covers prerequisites and failures. The `ros-jazzy` workflow repeats these checks and tests the artifact on a separate runner.

For Rust or documentation changes, install [rustup](https://rustup.rs/), [just](https://github.com/casey/just#installation), Python 3, and your platform's C compiler and linker, then run `just gate`. The repository pins Rust. The gate checks the Rust workspace, committed replay expectations, licenses, prose, and local links. CI compares the bundled Wasm case's hashes on Linux x86_64 and macOS ARM64; that comparison makes no native macOS support claim.

Keep `ROOK_WRITE_FIXTURES` unset. Existing expected outputs, hash domains, the format specification, and the `rook-core` dependency list require explicit maintainer approval for the named files before changes. Never regenerate expectations to make a failed test pass. The full constraints are in [AGENTS.md](AGENTS.md#hard-rules).

The reports in [docs/evidence](docs/evidence/README.md) preserve one historical CI run. Keep those bytes and their provenance intact. New measured results belong in a separately identified example.

## Open a pull request

Explain the problem, changed behavior, and checks you ran. For execution changes, report baseline agreement separately from the candidate's behavioral result. Include the build and platform, and say what remains unverified. Every ROS change needs a passing `ros-jazzy` workflow before merge. Maintainer review is required before merge.

Contributions use Apache-2.0 with no separate contributor agreement. Preserve third-party notices and submit only material you have the right to license.
