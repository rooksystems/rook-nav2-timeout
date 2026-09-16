# ADR 0001 - Four-way hash equality

Accepted on 2026-08-16. Rust 1.95.0 and Wasmtime 38.0.4.

## Decision

Keep Wasmtime as the Wasm host. The first experiment established equal replay hashes for a synthetic echo-and-count cell across three native hosts and the core compiled to Wasm. This was the initial check of the execution and hashing design.

## Evidence

The 48-line cell processed 100,000 synthetic delivery envelopes. Native macOS ARM64, native Linux x86_64, native Linux ARM64, and `rook-core` compiled for `wasm32-unknown-unknown` produced these hashes.

- `run_hash` was `df3d4dcb65edac3dfd0d146e45875a37fdb417f6e56539697029442bedba45a6`.
- `output_digest` was `ef94774fb79a9dac9c84eaa8d192f1d3ff8c27b15bb5bd4fb7a1e381610ff951`.

The macOS host ran macOS 26.5 on Apple ARM64. Linux x86_64 used Ubuntu 22.04 on a Lambda `gpu_1x_a10` in `us-west-1`; Linux ARM64 used Ubuntu 22.04 on a Lambda `gpu_1x_gh200` in `us-east-3`. Wasmtime executed the Wasm core on each host and matched its native result.

Each host ran the gate available at the time, including the cell and core builds, formatting, core tests, Clippy and the replay comparison. These are historical experiment results; current platform support is defined by the release evidence.

## Execution settings

The host enables NaN canonicalization and deterministic relaxed SIMD, disables guest threads, uses fuel instead of epoch interruption, and limits guest memory through `StoreLimits`. The cell has a fixed 32-page memory, or 2 MiB. The core wrapper has 1,024 pages, or 64 MiB. The fuel bound is 10,000,000,000 units and does not enter the hashes.

## What changed during the experiment

Rust's default 1 MiB Wasm stack did not fit the initial one-page cell memory. The final cell uses 2 MiB of memory with that stack size.

The first core wrapper materialized and cloned the corpus, then regenerated it for every returned hash word. It exceeded the memory limit. Streaming the canonically ordered inputs and computing both hashes once removed that overhead. The fuel bound then increased from 1,000,000,000 to 10,000,000,000 so the intended run could finish.

An ordering review found same-tick deliveries and effects alternating in the chain. The intended scheduler hashes all sorted deliveries for a live tick before its effects. Correcting the order changed `run_hash` while leaving `output_digest` unchanged.

The cell initially emitted only a count. It was corrected to emit the original 16-byte payload followed by the 8-byte count before the Linux comparisons. All reported hosts matched the final hashes after these corrections.

## Limit

The experiment covered one synthetic behavior and the tested configurations. Extracting and replaying a third-party decision component remained a separate experiment, recorded in [ADR 0002](0002-experiment-2.md).
