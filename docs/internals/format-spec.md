# Format specification

This specification defines the 72-byte envelope, native stream v1 and case format v1. Implementations must follow these layouts. [rook-core](../../crates/rook-core/src/lib.rs) implements the Wasm envelope, [rook-native](../../crates/rook-native/src/lib.rs) implements the native stream, and [rook-verify](../../crates/rook-verify/src/main.rs) reads cases. The [native contract](native-contract.md) defines event meaning and adapter behavior; [ADR 0007](../adr/0007-native-contract.md) records the design decision.

The Wasm envelope and native envelope have different versions and ordering rules. Native stream v1 uses envelope version 2; the outer case format remains version 1. These identifiers describe different layers.

## 1. The 72-byte envelope

Every hashed event, on both paths, is a fixed-width little-endian envelope.

| offset | size | field | notes |
|---|---|---|---|
| 0 | 4 | magic | `RKE1` |
| 4 | 2 | version | 1 = Wasm-cell envelope (rook-core), 2 = native stream |
| 6 | 2 | kind | 1 Deliver, 2 Emit, 3 Timer, 4 Gap, 5 Marker |
| 8 | 8 | tick | Wasm: the scheduler tick. Native: the event ordinal |
| 16 | 4 | src_actor | |
| 20 | 4 | dst_actor | |
| 24 | 4 | channel_id | |
| 28 | 4 | payload_len | |
| 32 | 8 | src_seq | per-source sequence, see each path |
| 40 | 32 | payload_blake3 | BLAKE3 of the payload bytes |

The version field identifies the envelope layout and interpretation. A native decoder requires version 2 and reports `WrongEnvelopeVersion` for another value. Readers must enforce the version they support before interpreting the remaining fields.

## 2. Native stream v1

A native stream is one file, `events.bin`, holding frames back to back.

### 2.1 Frame

| offset | size | field |
|---|---|---|
| 0 | 72 | envelope, version 2 |
| 72 | 8 | observed_ns, u64, the recorder's clock when the frame was written |
| 80 | payload_len | payload |

`observed_ns` records the observation time. It is included in the raw record hash and excluded from the execution hashes defined in section 2.6. Ordinals define native stream order.

### 2.2 Payload

| offset | size | field |
|---|---|---|
| 0 | 2 | event_type, section 2.4 |
| 2 | 2 | schema, 1 for every type in this document |
| 4 | 4 | flags |
| 8 | rest | body, laid out per event type |

Every multi-byte integer in the event header and body is little-endian. Length prefixes exist only where the event layout lists one. A trailing `bytes` or `text` field occupies the remaining body; `Status` text has no additional length prefix.

Flag bit 0 is `ATTEMPTED`, meaning an effect was handed to the transport. Bit 1 is `CONFIRMED`, meaning the transport returned success. Bit 2 is `COUNT_UNKNOWN`, used for a gap with an unknown lost count. Other bits are reserved and must be zero; a set reserved bit is refused with `ReservedFlags`. The schema must be 1 or the decoder reports `UnsupportedSchema`. A body-layout change requires a schema bump and an ADR.

### 2.3 Stream rules

A reader (`rook_native::decode_stream`) refuses the whole stream, naming the first offending ordinal or byte offset, when any rule fails.

- Ordinals count 0, 1, 2, ... in file order (`NonContiguousOrdinal`).
- `src_seq` counts 0, 1, 2, ... per (`src_actor`, `channel_id`) (`SequenceGap`). Session, End and Gap frames are from actor 0 to actor 0 on channel 0; StartingState is from actor 0 to actor 1 on channel 0. Any other tuple on a marker or gap is `MalformedMarker`.
- Actor 0 is the session (the recorder), actor 1 is the component. Channel 0 is the session channel. Other actors and channels are named by the recording's endpoint and channel tables (`native-contract.md`, section 3).
- Frame 0 is a Session marker from actor 0 to actor 0 on channel 0 (`MissingSession`, `UnexpectedSession`, `MalformedMarker`).
- The last frame is an End marker whose uuid equals the session's and whose `frame_count` equals its own ordinal (`MissingEnd`, `EndMismatch`, `UnexpectedEnd` for anything after it).
- `payload_blake3` equals BLAKE3 of the payload (`PayloadHashMismatch`).
- The envelope kind equals the event type's fixed kind (`KindMismatch`).
- The body must follow its section 2.4 layout, with the required length, matching length prefixes where present, and valid values for explicitly enumerated fields (`MalformedBody`). Bodies are checked before any chain is hashed.
- A frame that runs past the end of the file is `Truncated { offset }`.
- Gap frames are accepted by the decoder and judged by the reader (`native-contract.md`, section 7); their `COUNT_UNKNOWN` flag must be set exactly when the body's lost count is the unknown sentinel (`MalformedMarker` otherwise).

A file that stops before its End marker is refused. A deliberately shortened stream with a matching new End marker can decode successfully. Its changed hashes must be compared with independently obtained expected values; structural validity alone cannot detect the rewrite. The `forged-end.bin` fixture exercises this distinction.

### 2.4 Event types

| type | kind | name | body |
|---|---|---|---|
| 0x0001 | Marker | Session | stream_version u16 (1), uuid [16], record_utc_ns u64, pid u32 |
| 0x0002 | Marker | End | uuid [16], frame_count u64 |
| 0x0003 | Gap | Gap | reason u16, lost u64 (u64::MAX unknown), detail_len u16, detail |
| 0x0004 | Marker | StartingState | method u16 (1 fresh, 2 snapshot, 3 unsupported), state_blake3 [32] |
| 0x0005 | Deliver | Lifecycle | transition u8, request_id u64 |
| 0x0010 | Deliver | Message | serialization u16 (1 CDR, 2 raw, 3 JSON), bytes |
| 0x0011 | Deliver | Clock | clock_id u16 (1 ROS, 2 steady, 3 system), value_ns i64, callback_ordinal u64 |
| 0x0012 | Timer | Timer | timer_id u32, clock_id u16, expected_call_ns i64, actual_call_ns i64, state u8 (1 armed, 2 cancelled, 3 reset), return_status i32 |
| 0x0013 | Deliver | ServiceResponse | call_ordinal u64, status i32, bytes |
| 0x0014 | Deliver | GoalResponse | goal_send_ordinal u64, goal_id [16], accepted u8, stamp_ns i64 |
| 0x0015 | Deliver | Feedback | goal_send_ordinal u64, goal_id [16], bytes |
| 0x0016 | Deliver | Result | goal_send_ordinal u64, goal_id [16], status u8, bytes |
| 0x0017 | Deliver | CancelResponse | cancel_send_ordinal u64, return_code u8, n u16, n goal ids [16] |
| 0x0018 | Deliver | ServiceRequest | request_id u64, bytes |
| 0x0020 | Emit | Publish | transport_status i32, serialization u16, bytes |
| 0x0021 | Emit | ServiceCall | sequence u64, bytes |
| 0x0022 | Emit | GoalSend | goal_id [16], bytes |
| 0x0023 | Emit | CancelSend | goal_id [16] (all zero is the wildcard), stamp_ns i64 |
| 0x0024 | Emit | ServiceResponseSend | request_id u64, bytes |
| 0x0025 | Emit | LifecycleResult | transition u8, result u8 |
| 0x0026 | Emit | Status | text, a diagnostic transition and never a command |
| 0x0027 | Emit | TimerControl | timer_id u32, op u8 (1 cancel, 2 reset) |

Unlisted event types are reserved. The reference adapter uses Session, End, Gap, StartingState, Message, Clock, GoalResponse, Result, CancelResponse, GoalSend, CancelSend and Status; [bodies.rs](../../crates/rook-adapter-ref/src/bodies.rs) implements those bodies. The other layouts support future integrations. Changing a layout requires an ADR and a schema bump.

### 2.5 Session identity at offset zero

The recorder creates `events.bin` with exclusive create, writes the Session marker first, and never reuses a uuid. Two recordings of the same run carry different uuids. The End marker repeats the uuid so a tail from one recording cannot close another.

### 2.6 Hashes

All four values use BLAKE3. An envelope chain extends as `keyed(key, previous || envelope)`, with key `BLAKE3(domain)`. The trace and output chains start from 32 zero bytes. The run chain starts from the keyed trace hash as specified below.

- `raw_record_blake3` is plain BLAKE3 over the whole file, observed times included. This is the raw record hash and is always reported on its own.
- `trace_hash` uses domain `rook-native-trace-v1/<corpus>`, over every frame whose kind is not Emit, in ordinal order.
- `run_hash` uses domain `rook-native-run-v1`, seeded with `keyed_hash(run_key, trace_hash)`, then every frame in ordinal order, effects included. The interleaving of inputs and effects is committed here; input A, effect A, input B, effect B hashes differently from A, B, a, b.
- `output_digest` uses domain `rook-native-output-v1`, over Emit frames only, in ordinal order, ordinals included.

The native domains are distinct from the Wasm domains `rook-trace-v1/...`, `rook-run-v1` and `rook-output-v1`.

### 2.7 Conformance fixtures

[Conformance fixtures](../../fixtures/native-conformance/) hold the exact bytes and expected results for corpus `conformance@1`. The tests in [conformance.rs](../../crates/rook-native/tests/conformance.rs) compare generated streams with those committed values. Regeneration requires explicit approval under [AGENTS.md](../../AGENTS.md#hard-rules).

- `interleaved.bin` contains Session, A, a, B, b, End. Its `run_hash` is `16cf8cb9975d25e59b32b91676a26efbb43bd7577c341b9197ab60666c9331cc`.
- `grouped.bin` contains Session, A, B, a, b, End. Its `run_hash` is `2dfd0faa48973d108022ce99ad1f4106be08c33c1f7da6bf3e1fa8248bf2e08e`. Every execution hash differs from `interleaved`.
- `gap.bin` contains a gap frame in a valid stream.
- `reordered.bin` (frames swapped verbatim), `reordered-renumbered.bin`, `omitted.bin`, `extra.bin`, `truncated.bin`, `torn.bin`, `wasm-version.bin`, `bad-schema.bin`, `reserved-flags.bin`, `stray-end.bin`, `gap-flag-mismatch.bin`, `short-body.bin` are refused, each with the error named in `expected`.
- `omitted-renumbered.bin`, `extra-renumbered.bin`, `forged-end.bin` are valid streams whose hashes differ from `interleaved`. A runner detects the change by comparing them against independently obtained expectations.

## 3. MCAP export is planned

`events.bin` is the authoritative native recording. MCAP export is planned for inspection tools; the current verifier reads `events.bin` and has no MCAP input path.

The export mapping needs a separate design decision before implementation. A Rook channel can carry several event types, while an [MCAP channel](https://mcap.dev/spec#channel-op0x04) identifies one schema. MCAP's message `sequence` is a `uint32`, while Rook ordinals are `u64`. The mapping must define how schemas are assigned and how full ordinals are preserved. No fixed mapping is specified here yet.

## 4. Capsule format v1

A case is stored as a directory. `MANIFEST.sha256` begins with `# rook-capsule-v1 kind=<kind>`. File entries use `<sha256>  <name>`, with two spaces between the digest and name. Blank lines and comment lines beginning with `#` are ignored after the header. Each entry must name a regular file directly inside the directory and appear only once. Entries containing path separators or `..` are refused, as are symlinked entries.

These entry rules do not provide filesystem isolation. The current verifier opens the manifest through ordinary filesystem calls and reopens files after hashing. [SECURITY.md](../../SECURITY.md) explains manifest symlinks, concurrent replacement and host permissions.

`rook-verify` checks the header and listed files, selects a supported kind and requires its files before processing it. A missing or malformed header exits 2 with a header diagnostic. An unknown kind exits 2 naming that kind. For a supported kind whose component this build cannot execute, data checks can still report verified file integrity; the command reports execution as unavailable and exits 2 unless an earlier check fails.

### 4.1 `wasm-rmf-blockade`

Required files are `expected`, `deliveries.bin`, `heartbeats_recorded.bin`, `params`, `rmf_blockade_cell.wasm`. The required `expected` keys are `bag_run_hash`, `bag_output_digest`, `recorded_states`, `heartbeats_recorded_sha256`. The execution claim is guaranteed. [ADR 0006](../adr/0006-public-capsule.md) describes the comparison and exit statuses.

### 4.2 `native-adapter`

Required files are `expected`, `events.bin`, `effects_captured.bin`, `identity`, `case`. The execution claim is measured, and `expected` must say so.

The required `expected` keys are `raw_record_blake3`, `effects_captured_blake3`, `trace_hash`, `run_hash`, `output_digest`, `grade` (Complete, Rebuilt, Inferred, Watch-only), `claim` (`measured`).

`effects_captured.bin` stores independently captured effects in receipt order, each as a little-endian `len u32` followed by that many payload bytes, including the event header and body.

`identity` and `case` are key=value files; their keys are in `native-contract.md`, sections 9 and 11. `case` must carry `corpus`.

`rook-verify` processes this kind in order. It verifies the manifest (a failure refuses with exit 2); requires the keys of `expected`, `identity` and `case` listed in `native-contract.md` sections 9 and 11 (a missing key refuses with exit 2); checks the two file hashes against `expected`; checks that `effects_captured.bin` is whole records, each a valid Emit payload under section 2.4 (a torn or non-effect record refuses with exit 2); decodes `events.bin` under section 2.3 (a refusal there exits 2 naming the `StreamError`); recomputes the three execution hashes of the recording and prints them with the raw record hash.

If any hash differs from `expected` it prints each `diverged:` line, then `file_integrity: diverged`, and exits 1 without the other three claim lines. Otherwise it prints `file_integrity`, `execution_agreement`, `capture_completeness` and `incident_authenticity` on separate lines, `claim: measured`, and exits 2 with `not executed: capsule kind native-adapter is not executable by this build`. Execution agreement belongs to the planned `rook verify CASE` command in [#18](https://github.com/rooksystems/Rook/issues/18), running on a host that can execute the case's adapter.

### 4.3 Anything else

Exit 2, `refusing to run: capsule kind <kind> not supported by this build`.
