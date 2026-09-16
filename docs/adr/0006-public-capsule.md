# ADR 0006 - A standalone case verifier

Accepted on 2026-08-20. Rust 1.95.0 and Wasmtime 38. The cell, four fixtures, five goldens and existing hash domains were unchanged by the original implementation.

## Decision

Package the held-out recording with a standalone verifier so an outside engineer can reproduce the replay result and locate a difference in a tampered copy. The implementation and CI checks were completed in this experiment. An outside engineer following the instructions remained a separate, unperformed test.

The [case directory](../../capsules/rmf-blockade-holdout/) combines the held-out inputs, recorded heartbeats, configuration, capture identities and provenance with the prebuilt Wasm cell, expected results and a SHA-256 manifest. The gate byte-compares the mirrored evidence with the original fixture.

`rook-verify` checks the manifest and required files, replays through the shared host implementation, and compares results. Exit 0 means the supported case passed those checks. Exit 1 reports a comparison mismatch. Exit 2 reports refusal or an input-processing error. A separate refusal code lets callers distinguish an input that could not be checked from a completed comparison that disagrees.

## Why a shared verifier

Exposing the host replay implementation as a library lets the standalone command reproduce the same execution used by the gate. Existing goldens checked that this refactor preserved behavior. The verifier adds SHA-256 support through `sha2` for case integrity.

Rebuilding the verifier from source gives a reader an inspectable verification path. It still shares implementation with the host, so a common replay bug can affect both. The case supplies its own expectations; agreement with a published artifact requires reference hashes obtained independently of the case being checked.

## Historical cross-architecture evidence

CI run `32434773682` on revision `2da7f81` passed both platform jobs and the cross-architecture comparison. Linux x86_64 and macOS ARM64 produced the same values as the local run and ADR 0005.

- `bag_run_hash` was `99ba8401de4a3aee378e336b6152c8a279fd645a2372d3f197170f86d72ec4e5`.
- `bag_output_digest` was `8265ec615fe456a718d18ffeb8018dde931ec1044f4df2b33dc594cae22aad22`.
- The distinct-state comparison was 672/672.

The CI jobs ran the gate and release verification recipe, asserted their machine architectures with `uname -m`, uploaded the platform verifier binaries and compared their hash output. The Rust toolchain action was pinned to a reviewed commit and explicitly installed rustfmt and clippy.

## How a difference is reported

The verifier compares recorded and replayed heartbeat payloads in order. A changed recorded output is reported at its recorded tick. A changed delivery is reported at the first decision that differs as a consequence. An integrity or metadata mismatch can have no divergent decision tick to report.

Replay emits at the timer delivery tick, while the recording timestamps the later published heartbeat. Those timestamps are not compared for equality. Instead, the expected SHA-256 of the full recorded-heartbeat file protects its timestamp bytes as well as its payloads.

The original six sabotage tests exercised a recorded payload edit, a delivery payload edit, an expected-hash edit, missing manifest coverage, a symlinked manifest entry, and a heartbeat timestamp edit with a repaired manifest. The timestamp test caught an earlier implementation that accepted the changed recording because its decision payloads were unchanged.

## Corrections found during implementation

Review added required-file coverage, duplicate-entry refusal and regular-file checks. It also corrected lax expected-value parsing, a non-UTF-8 argument panic and a misleading divergence message.

The first delivery tamper changed a checkpoint ID's high byte and triggered the shim's 32-bit ID guard. Changing the low byte reached the intended replay comparison. This distinction is useful when constructing a test for divergence rather than an input refusal.

## Isolation correction, 2026-09-07

The original wording overstated filesystem and network isolation. The Wasm guest has no WASI imports, but the verifier process retains its account's permissions and does not install an operating-system sandbox. It also opens `MANIFEST.sha256` through ordinary filesystem calls and reopens case files after hashing. Rejecting symlinks among manifest entries does not cover a symlinked manifest or concurrent file replacement.

[SECURITY.md](../../SECURITY.md) records these limits and the execution environment required for untrusted cases. This is a correction to the documented claim; this documentation pass does not add isolation to the verifier.

## Remaining experiment

The original local macOS gate, tamper walkthrough and cross-architecture CI checks passed. Outside usability had not been tested. [#11](https://github.com/rooksystems/Rook/issues/11) owns public packaging and that outside exercise. Publication requires Saketh's separate decision.
