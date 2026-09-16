# ADR 0007 - Native recording and the reference adapter

Accepted on 2026-09-05 when Saketh merged [PR #27](https://github.com/rooksystems/Rook/pull/27) after review. Rust 1.95.0 and Wasmtime 38. The original change preserved the Wasm envelope, scheduling key, existing hash domains, cell, fixtures and goldens.

## Decision

Define the native recording and adapter contracts before independent runner and component integrations depend on them. Native execution reports measured agreement for a tested build and platform under a declared comparison policy.

The [format spec](../internals/format-spec.md) defines the bytes. The [native contract](../internals/native-contract.md) defines adapter behavior, baseline verification, candidate testing and the conclusions a property may report.

## Recording order

Native stream v1 uses the 72-byte envelope with envelope version 2. The tick field carries the event ordinal, and sequence values are per source and channel. A Session marker begins the recording; an End marker supplies explicit end-of-recording evidence.

The domains `rook-native-trace-v1/<corpus>`, `rook-native-run-v1` and `rook-native-output-v1` separate native hashes from the Wasm path. Frames enter the native run chain in ordinal order so input/effect interleaving is preserved. Observed wall time sits beside the envelope and is protected by the raw-record hash.

`rook-native` owns this format because its ordering algorithm differs from the core's per-tick deliveries-then-effects order. Keeping it separate avoided changing the core and the existing Wasm expectations. The crate uses `no_std` and forbids unsafe code.

## Case kinds and verification

The manifest begins with `# rook-capsule-v1 kind=<kind>`. The header belongs there because the verifier already reads the manifest first and `shasum -c` ignores comment lines.

The `wasm-rmf-blockade` kind uses ADR 0006's replay path. For `native-adapter`, the standalone verifier checks file integrity and recording hashes, prints separate claim fields and exits 2 without executing the component. Unknown kinds are refused by name. Changing the kind of an existing case also exposes missing required files.

An End marker contains a UUID and count rather than a chain hash. A chain can be recomputed after a deliberate edit, so the marker detects structural truncation without establishing the recording's authenticity. Published reference hashes remain separate evidence.

## Adapter protocol and candidate testing

Adapters expose `start`, `step` and `finish` over newline-delimited JSON on standard input and output. They emit effects and progress when those occur, before the step reply. Pending clock reads and futures are reported so recorded observations can resume execution.

Baseline verification reproduces the original component. Candidate testing changes the component while preserving the case's adapter, property and comparison configuration. Outputs are compared and the behavioral property is evaluated separately.

The reference adapter declares that a novel cancel invalidates later recorded responses for the same goal. It refuses to consume those responses. A bounded property may finish at the cancel request while the callback remains pending; the report must leave cancellation acknowledgment and goal termination unavailable when the recording does not establish them.

## Reference evidence

The original macOS ARM64 gate passed the existing goldens and verified the RMF case at exit 0 with `bag_run_hash` `99ba8401de4a3aee378e336b6152c8a279fd645a2372d3f197170f86d72ec4e5` and `bag_output_digest` `8265ec615fe456a718d18ffeb8018dde931ec1044f4df2b33dc594cae22aad22`.

The accepted implementation had sixteen sabotage tests, including wrong and unknown kinds, a missing header, native claim reporting, a cut tail and an observed-time edit. Eighteen conformance streams checked native decoding, bytes and hashes, including unknown schemas and reserved flag bits.

The reference adapter records five scripted scenarios with four component variants, covering timeout, timely acknowledgment, a gap, a command gap and a command end. Fresh recordings matched the committed cases. The old component reproduced the baselines through the protocol with equal run hashes.

- In the timeout scenario, fixed passes at the cancel request. Old fails `cancel_request_issued` at ordinal 6, noop fails `goal_submitted`, and always-cancel fails `cancel_after_deadline_expiry`.
- In the timely scenario, old and fixed pass. Noop fails, and always-cancel fails `no_cancel_in_interval` after the adapter refuses the recorded acknowledgment.
- In the gapped scenario, old and fixed are inconclusive and report the gap.

## Findings that shaped the interface

An effect needs both its current step and originating callback because a clock observation can resume an earlier callback. Review also found responses bound only by goal ID, unrelated clock observations consumed for a pending read, refused inputs retained in replay output, and properties using another callback's clock or a later command's goal. The reference implementation was corrected to follow the contract.

The branch also repaired a stale case README hash in the manifest. Documentation shipped inside a case is part of that package's identity even when execution bytes are unchanged.

## Scope

The reference uses small native test components and a scripted environment. Its baseline effects are captured within that environment, and the case metadata states this limitation. It exercises the protocol and property rules; replay of an independently recorded production component still requires its own integration and evidence.
