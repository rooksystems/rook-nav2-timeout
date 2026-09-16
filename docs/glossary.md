# Glossary

These are the terms we use across Rook's code, issues and reports. [AGENTS.md](../AGENTS.md#a-small-glossary) defines who we mean by "you", "we" and "stranger". [Architecture](../ARCHITECTURE.md) explains how the parts fit together.

## Evidence and cases

**Incident** means a failure that happened in the field. A recording is evidence about an incident; synthetic tests and demonstration runs can also produce recordings.

**Boundary** is the interface around the decision component we replay. The recording contract names the inputs, effects and dependencies that must be accounted for there, including clock observations, timer callbacks and starting state.

**Recording** is the captured evidence of a run, together with its configuration, starting-state information and code identity. Its grade describes what was captured or reconstructed and what remains missing.

**Case** packages a recording with the code identity, configuration and expectations used to evaluate a run. A native case also names its behavioral check and comparison policy. We also call a case a repro file; the current format stores it as a directory and calls it a `capsule`. The case kind and available runtime determine what a tool can verify.

**Grade** describes the recording evidence. The [native contract](internals/native-contract.md#12-claims-and-grades-on-the-native-path) defines four values.

- **Complete** means recording at the declared boundary captured every input class the contract requires, including the clock observations and timer calls the component uses. Native streams also require an End marker.
- **Rebuilt** means reconstructed code reproduces the recorded effects.
- **Inferred** means order, state or configuration was filled in, or the recording contains a gap. The assumptions and gaps must be stated.
- **Watch-only** means there is a timeline to inspect and no replay to run.

A grade needs supporting evidence. The standalone verifier reads the declared grade and checks stream structure; it does not establish capture completeness merely by accepting the file.

## Replay and testing

**Replay** means re-executing decision code with recorded inputs under the recording's boundary contract.

**Baseline verification** checks evidence integrity, replays the original component and compares its effects with the independently recorded effects under the case's comparison policy. It establishes whether that build reproduces the recording within the declared scope.

**Candidate testing** runs a changed component against the case while keeping the adapter, behavioral check, comparison policy and scope fixed. It reports output differences and evaluates the check separately. Changed effects can make later recorded responses invalid, limiting what the test can conclude.

**Candidate build** is the changed component under test. Use the full name when it could be confused with a release candidate.

The included runner implements `rook verify CASE` and `rook test CASE --candidate BUILD`; the standalone `rook-verify` checks native case integrity without executing its component.

**Fork** is the design term for replay with a declared change from a named point. Once that change would affect the recorded environment, continuing requires stated assumptions or a response model. Conclusions apply to that experiment. A general fork command is not available today.

## Checks

**Case check** is the behavioral property the case evaluates, such as whether a pending goal triggers cancellation by its deadline. The native contract calls it a `property`. It can pass, fail or remain inconclusive when the required evidence is unavailable.

**The `rook check` diagnostic** repeats runs of an engineer's stack and reports per-publisher agreement and the first difference. It is installed from the upstream Rook Jazzy workspace (not included in this experiment). Use "case check" or "diagnostic" when "check" alone would be ambiguous.

## Execution claims

**Claim** describes execution agreement. Grade describes recording evidence. Recording reports keep `observation`, `grade` and `claim` as separate fields. The `rook check` diagnostic reports observed topic agreement without creating a recording, so it has no `grade` field.

**Guaranteed** is the Wasm claim. The same complete recording, engine version and cell bytes produce the same result bytes on supported machines.

**Measured** is the native claim. Agreement is established on the tested build and platform under a named comparison policy. It does not establish agreement on another machine.

**Raw agreement** means the effect fields covered by the comparison policy match without normalization, including their order and repeated occurrences.

**Normalized agreement** means agreement after a declared transformation, such as correspondence between generated goal IDs. The report preserves raw differences beside the normalized result.

## Execution parts

**Engine** refers to the deterministic core in `rook-core`, which defines event encoding, ordering and hashing. The host loads and executes Wasm cells.

**Cell** is decision code compiled to WebAssembly that receives inputs and emits effects through Rook's host interface.

**Adapter** is the integration that records and controls a native component's declared boundary for replay.

**Hash domain** is a fixed identifier used to derive a hashing key for a particular purpose and version. It separates hashes used for different purposes. It does not make collisions mathematically impossible or establish a recording's origin. The [format spec](internals/format-spec.md) defines the domains and hashed bytes.

## Releases

**Release candidate** is a named source revision and the artifacts CI built from it, identified in a candidate record. This experiment publishes source and CI evidence; it has no separately promoted binary release.
