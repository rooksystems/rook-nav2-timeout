# The native contract

This contract defines native recording, adapter behavior, execution comparison and behavioral checks. The [format spec](format-spec.md) defines the bytes. [ADR 0007](../adr/0007-native-contract.md) explains the design, and [Architecture](../../ARCHITECTURE.md) introduces the two execution paths.

Use the [glossary](../glossary.md) for shared terms. Native execution claims are measured; the Wasm path makes the guaranteed claim. Recording grades are Complete, Rebuilt, Inferred and Watch-only. Reports keep `observation`, `grade` and `claim` as separate fields.

The native stream, reference adapter and reusable runner are implemented. The `rook verify CASE` and `rook test CASE --candidate BUILD` commands execute supported local adapters through this contract. Section 15 documents their invocation, build descriptions and reports. Native recorder and Nav2 integrations follow this contract; a protocol extension requires an ADR.

## 1. The two paths

The Wasm scheduler orders deliveries by scheduling key and hashes all deliveries for a tick before its effects. Native recording must preserve the order observed at the component boundary, where callbacks can read clocks and emit effects between inputs. Native streams therefore use ordinal order with effects interleaved.

The conformance fixtures `interleaved.bin` (A, a, B, b) and `grouped.bin` (A, B, a, b) differ on all three execution hashes. Grouping the inputs ahead of their effects would change the recorded execution.

## 2. Events

A native recording contains the following frame kinds.

- An input the component consumed (kind Deliver) includes a message, a clock observation, a service or action response, a service request, a lifecycle request.
- A timer call (kind Timer) records the executor invoking a timer callback.
- An effect the component produced (kind Emit) includes a publish, a service call, a goal send, a cancel send, a service response, a lifecycle result, a timer cancel or reset, or a diagnostic status.
- A marker (kind Marker) identifies the session at offset zero, the starting state or the end of recording.
- A gap (kind Gap) records that events were lost here.

Every frame has an ordinal, the only order there is, and an observed time that no execution hash sees. Effects and diagnostics are different event types on different channels, so a status transition (a BehaviorTree node going RUNNING to FAILURE, say) is never mistaken for a command.

## 3. Identities

Actors and channels are small integers whose meaning is in the recording's endpoint and channel tables, which the adapter declares at `start` (section 8) and the `identity` file repeats (section 9). Actor 0 is the session, actor 1 the component. For a ROS process the stable entity key is the node's fully qualified name, the topic or service or action name, the type name and the creation ordinal within the process; the rmw gid is stored beside it as a cross-check and is never the key, because a gid does not survive a restart. Per-source sequences (`src_seq`, one counter per source and channel) make a missing or duplicated event from one endpoint a refusal at decode time, before any hash is compared.

Generated identities (goal ids, request ids) are recorded raw. The component may generate them however it likes; the recording keeps the bytes, and any correspondence between a recorded id and a replayed one is normalization (section 10), declared and reported separately.

## 4. Callbacks, clocks and timers

A clock observation is an input. The `Clock` body carries the clock (ROS, steady, system), the value, and `callback_ordinal`, the ordinal of the input whose callback consumed the read. It is consumed only by a read from the same callback for the same clock, supplied by the clock endpoint. A steady-clock value never answers a ROS-clock read. Every clock type the component reads is recorded, steady included; ROS-time override controls none of them during replay, the adapter feeds each recorded observation to the read that asked for it. A callback that needs a clock value it has not been given returns to the adapter with a pending clock read (section 8) rather than reading anything live.

A `Timer` frame is the executor calling a timer callback. It carries the timer id, the clock the timer runs on, the expected and actual call times the callback could observe, the timer's state (armed, cancelled, reset) at the call, and the return status the executor saw. A component cancelling or resetting a timer is an effect (`TimerControl`), so a replay knows the timer state without inferring it.

An effect belongs to the callback that produced it. The protocol reports both the step (the input being processed) and the callback (the input that started the callback), because a clock read or a response resumes a callback that started earlier. For each step the runner accepts only the authorized callbacks, which are the input itself, the callback a clock observation names, and the callback that issued the request a response answers; an effect or progress mark claiming any other callback is an error and never becomes evidence.

## 5. Lifecycle, services and actions

Lifecycle transitions requested of the component are inputs; the results it returns are effects. A service call the component makes is an effect; its response is an input carrying `call_ordinal`, the recording's ordinal of the call it answers. A goal send is an effect carrying the raw goal id; the goal response, feedback and result are inputs carrying `goal_send_ordinal` and the raw goal id; a cancel send is an effect and a cancel response an input with `cancel_send_ordinal`. The all-zero goal id in a cancel send is the wildcard and stays a wildcard through every comparison.

Correlation binds a response to its originating client and the request it answers, through those ordinals and ids. Arrival order does not establish it, and neither does "the goal we are currently expecting". The runner resolves the request ordinal in a response body to the recorded request frame and hands the adapter that frame's identity (actor, channel, sequence) beside the input (section 8.1); the adapter binds the response only when that identity is its own issuance on that channel, the raw id matches, and the response comes from the endpoint it sent the request to. A response that binds to nothing is reported `not_consumed` and changes nothing. Before that, the runner checks that this run issued the request the recorded response answers, equivalent in destination, type and bytes; when it did not, the response belongs to a request this run never made, the runner refuses it, and the run stops there. The reference tests feed a response with the right goal id and a wrong request reference, and one from the wrong endpoint; neither is applied.

## 6. Attempted and confirmed effects

An effect frame's flags say whether the component handed it to the transport (`ATTEMPTED`) and whether the transport returned success (`CONFIRMED`). Only the `Publish` body carries the transport's return code; for every other effect type the `CONFIRMED` bit is the whole status, and a failed transport call is an effect with `ATTEMPTED` set and `CONFIRMED` clear. A recorder that writes the frame after the transport returns can set both; a recorder that must never lose an attempted effect writes before the call, sets `ATTEMPTED` only and records the status as unknown. The `identity` file says which (`effect_record_order`). A crash between the transport call and the write loses the frame and the End marker, and the recording is refused as unclosed. It is never a shorter successful recording.

## 7. Gaps and the end of recording

The End marker is the only end-of-recording evidence. Without it the stream is refused (`MissingEnd`); a torn last frame is `Truncated`. A runner that reaches the End marker having delivered every input reports `exhausted`. Reaching the end of what was recorded is not the same as satisfying a case's declared scope, and a report says which happened.

A Gap frame means the recorder lost events at that point, with a reason, a count when it knows one, and a note. A reader never delivers anything past a gap. A recording with a gap is at most Inferred, and a case on it can only complete before the gap; a property whose evidence would have to come from after the gap is inconclusive and names the gap (section 11).

## 8. The adapter interface

An adapter is a program the runner starts. It wraps one component, speaks one JSON object per line, reads on stdin, writes on stdout, constructs the component in its declared starting state, supplies the recorded inputs sent by the runner, emits effects and progress as they occur, and reports pending operations. It never reads a clock, opens a socket, or invents a response.

Run the reference with `rook-adapter-ref serve --component old` (also `fixed`, `noop`, `always-cancel`). The candidate build is the component variant; the adapter around it is the same bytes.

### 8.1 Messages

Runner-to-adapter messages have these fields.

- `start` carries `protocol` (1), `corpus`, `scenario`, `state` (`method` `fresh` or `snapshot`, `blake3` of the state bytes).
- `step` carries `input`, a frame with its recorded `ordinal`, actors, channel, `src_seq`, `event_type` (the name from `format-spec.md`), `schema`, `flags`, `body_hex`; and for a response input, `request`, the recorded frame the body's request ordinal names (`ordinal`, `src_actor`, `dst_actor`, `channel_id`, `src_seq`), which the runner resolves and refuses to send when it is not an earlier effect of the component. The runner sends every recorded frame that is not an Emit, a Session or an End, in ordinal order, and stops at a gap or a refusal.
- `finish` carries `reason` `exhausted` (every input delivered) or `scope` (stopped at the declared scope, a gap or a refusal).

Adapter-to-runner messages have these fields.

- `started` carries `protocol`, `identity` (component, variant, adapter, protocol), `endpoints` (actor, name), `channels` (id, name, direction, event types), `state` (method and blake3 as constructed). Or `refused` with a reason, and the adapter exits.
- `effect` carries `step` (the input being processed), `callback` (the input whose callback produced it), `effect` (a frame without an ordinal; the adapter assigns `src_seq` per channel, the runner assigns the ordinal when it appends the effect to the replayed stream).
- `progress` carries `step`, `callback`, `name`. Named progress the property may require so that disabled code or a frozen clock cannot pass vacuously.
- `stepped` carries `ordinal` (echoed), `status`, `reason`, `pending`. Exactly one per step, after every effect and progress line of that step. The `reason` field is optional. `status` is `ok` (consumed), `not_consumed` (valid input, nothing was waiting for it, reported and not applied) or `refused` (section 10; the runner stops). `pending` lists what the component waits on, `clock_read` with the callback and clock, or `future` with the callback and a name. A refused input is dropped from the replayed stream; it was never applied, so it is not evidence.
- `finished` carries `reason` echoed, `pending` at finish, `effects` count.

### 8.2 Returning control while a callback waits

A step returns as soon as the component has nothing to do without another recorded input. A callback blocked on a clock read or a future is reported as pending, and its effects so far have already been written. That is how a bounded property can complete at an emitted effect while the callback that emitted it is still inside its wait, and how the runner never has to invent the response the recording lacks.

### 8.3 A transcript

The `timeout` recording (`fixtures/native-ref/timeout/events.bin`, nine frames, recorded from the `old` component) driven through the `fixed` candidate. Runner lines first.

```
{"type":"start","protocol":1,"corpus":"adapter-ref@1","scenario":"timeout","state":{"method":"fresh","blake3":"af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"}}
{"type":"step","input":{"ordinal":1,"src_actor":0,"dst_actor":1,"channel_id":0,"src_seq":1,"event_type":"StartingState","schema":1,"flags":0,"body_hex":"0100af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"}}
{"type":"step","input":{"ordinal":2,"src_actor":4,"dst_actor":1,"channel_id":10,"src_seq":0,"event_type":"Message","schema":1,"flags":0,"body_hex":"0200676f616c2031"}}
{"type":"step","input":{"ordinal":3,"src_actor":2,"dst_actor":1,"channel_id":11,"src_seq":0,"event_type":"Clock","schema":1,"flags":0,"body_hex":"010000000000000000000200000000000000"}}
{"type":"step","input":{"ordinal":5,"src_actor":2,"dst_actor":1,"channel_id":11,"src_seq":1,"event_type":"Clock","schema":1,"flags":0,"body_hex":"01000065cd1d000000000200000000000000"}}
{"type":"step","input":{"ordinal":6,"src_actor":2,"dst_actor":1,"channel_id":11,"src_seq":2,"event_type":"Clock","schema":1,"flags":0,"body_hex":"0100008c8647000000000200000000000000"}}
{"type":"finish","reason":"exhausted"}
```

The adapter responses follow. The `started` line abbreviates the endpoint and channel arrays with `[...]`; obtain their full values from `identity` or a live reference response. This displayed line is illustrative rather than valid JSON.

```
{"type":"started","protocol":1,"identity":{"component":"goal-client","variant":"fixed","adapter":"rook-adapter-ref","protocol":1},"endpoints":[...],"channels":[...],"state":{"method":"fresh","blake3":"af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"}}
{"type":"stepped","ordinal":1,"status":"ok","pending":[]}
{"type":"stepped","ordinal":2,"status":"ok","pending":[{"kind":"clock_read","callback":2,"clock_id":1}]}
{"type":"effect","step":3,"callback":2,"effect":{"src_actor":1,"dst_actor":3,"channel_id":20,"src_seq":0,"event_type":"GoalSend","schema":1,"flags":3,"body_hex":"ac2a682cac99582c360535a142385f26676f616c2031"}}
{"type":"progress","step":3,"callback":2,"name":"goal_submitted"}
{"type":"stepped","ordinal":3,"status":"ok","pending":[{"kind":"future","callback":2,"name":"goal_response"},{"kind":"clock_read","callback":2,"clock_id":1}]}
{"type":"stepped","ordinal":5,"status":"ok","pending":[{"kind":"future","callback":2,"name":"goal_response"},{"kind":"clock_read","callback":2,"clock_id":1}]}
{"type":"effect","step":6,"callback":2,"effect":{"src_actor":1,"dst_actor":3,"channel_id":21,"src_seq":0,"event_type":"CancelSend","schema":1,"flags":3,"body_hex":"ac2a682cac99582c360535a142385f26008c864700000000"}}
{"type":"progress","step":6,"callback":2,"name":"deadline_expired"}
{"type":"progress","step":6,"callback":2,"name":"cancel_requested"}
{"type":"stepped","ordinal":6,"status":"ok","pending":[{"kind":"future","callback":2,"name":"cancel_response"}]}
{"type":"finished","reason":"exhausted","pending":[{"kind":"future","callback":2,"name":"cancel_response"}],"effects":2}
```

The goal command at ordinal 2 starts a callback that needs the time; the clock observation at 3 (value 0, callback 2) resumes it, the goal goes out, and the callback waits on the goal response while polling the clock. At ordinal 6 the clock reads 1.2 s, past the 1 s deadline, the candidate cancels, and the callback is now waiting on a cancel response the recording does not hold. Finish reports that wait. Nothing was invented. The recorded `old` component, after consuming the same clock observation at ordinal 6, emitted a Status line and left the goal pending. That recorded effect at ordinal 7 is compared with the candidate's cancel.

### 8.4 What the runner does with it

The reference driver in `crates/rook-adapter-ref/src/driver.rs` builds the replayed stream from the Session marker, every delivered input, effects at their point of emission, and an End marker. It renumbers frames from zero and sets observed times to zero. The product runner in `crates/rook-verify/src/runner.rs` follows the same rules. It compares effects with the recording under the declared comparison policy (section 9) and computes the three execution hashes of both streams. Recorded input ordinals are never rewritten, so `callback_ordinal` and `goal_send_ordinal` always mean the recording's ordinals. With declared goal-ID normalization, eligible response IDs are translated for the adapter; raw evidence stays in the case, and comparison hashes use the original recorded input bodies.

## 9. Baseline verification and candidate testing

Baseline verification and candidate testing produce separate reports.

Baseline verification (`rook verify CASE`) pins the recorded component's identity manifest, checks raw evidence integrity (the manifest, the raw record hash, the captured effects hash, the recording decodes and is closed), replays the recorded inputs through the adapter around the same component, and compares the independently captured effects with the replayed effects under the case's comparison policy, in order and with multiplicity. It reports the raw record hash apart from the normalized execution hashes (trace, run, output) and apart from any declared normalization. Agreement is measured on the tested build and platform; nothing is assumed about another machine.

Candidate testing (`rook test CASE --candidate BUILD`) permits exactly one identity to change, the component. The adapter, the property program, its input schema and configuration, the normalizer and its configuration, the declared scope and the completion rule keep the identities the case records; a different property or normalizer is a different case and cannot be shown as a passing candidate for this one. The candidate's effects are shown beside the recorded ones, the first difference named, and the property evaluated over the run. The property determines the behavioral result. The report shows output differences separately, since a candidate fix may deliberately change the component's effects.

The identity manifest (`identity`, key=value) names the main executable, the component and its plugins, ROS and RMW versions, the shared libraries that matter, schema and serialization identity, startup configuration, the adapter and its protocol version, the starting-state method and state hash, the property program and input schema, the normalizer and its configuration, the declared scope and completion rule, and content identities for each program, `effect_record_order` (section 6), and the endpoint and channel tables as `endpoint.<actor>` and `channel.<id>` lines. Hashing loaded libraries alone is not an identity. The reference records source content identities (BLAKE3 over the source embedded in its binaries) and names the toolchain; the runner pins the binaries it actually runs.

The reference comparison policy, `raw-bytes-in-order@1`, requires effect frames to be equal by destination, channel, event type, flags and body, in order, with multiplicity, and the run hash equal, which commits the interleaving too.

A native verification report keeps `file_integrity`, `execution_agreement`, `capture_completeness` and `incident_authenticity` on separate lines. The standalone verifier prints the full report only after its data checks pass. A hash mismatch prints `file_integrity: diverged` and exits 1; malformed data is refused with exit 2 before the full report. Recording authenticity requires separate evidence.

## 10. Request-response validity

When a candidate emits an effect the recording's environment never saw, a later recorded response may no longer be a valid input. Each adapter declares which effects invalidate which later inputs, and refuses to consume an invalidated one with `status: refused` and the reason. The runner stops there; what the property makes of it is section 11.

The reference adapter declares that a cancel send for a goal invalidates every later recorded goal response, feedback and result for that goal. In the `timely` scenario the standalone reference driver runs the always-cancel candidate through its novel cancel to the recorded acceptance at ordinal 6 and refuses that acceptance. The `rook test` runner stops earlier, at the demonstrated property failure on the cancel effect at replay ordinal 5, before consuming or refusing the acceptance. The property has already seen the cancel inside the observation interval and fails on it, a demonstrated wrong behavior, which is not the same as running out of evidence.

A novel request has no recorded response. Unless a case names a response model with its own identity and assumptions, the run reports the pending future and stops; the bounded cancellation property needs no such model.

Goal-id normalization, when a case declares it, is one-to-one on issuance within the originating client. The n-th goal issued by a client in the replay corresponds to the n-th in the recording, established at issuance, kept for overlapping and completed goals, reused for acknowledgments, feedback, results and targeted cancellation. Arrival order never establishes it, a current expectation never overwrites an earlier goal's mapping, and an unexpected result is never relabelled as the expected one. Raw ids stay in the evidence; extra, missing or ambiguous correspondence stays visible; the wildcard cancel stays a wildcard. The runner implements this in `crates/rook-verify/src/normalization.rs`; the reference declares `none@1` because its generated ids are a function of issuance order already.

## 11. The property program

A property is an executable, separate from the adapter so that substituting a candidate cannot change the property's content identity. Run the reference with `rook-property-ref evaluate <input.json>`. It prints one `rook-property-result@1` line, with `result` `invalid` when the input could not be evaluated, and exits 0 pass, 1 fail, 2 invalid input, 3 inconclusive.

The `rook-property-input@1` object contains `property`, `scenario`, `scope` (`completion`, `observation_end`), the case parameters the property needs (here `goal_response_deadline_ns`), `events` (the replayed stream, each with `origin` `recorded` or `replayed`, its kind, type, actors, channel, sequence, flags, body; for recorded inputs `recorded_ordinal` and `consumed`; for effects the `callback`), `progress` marks, `evidence` (`end_of_recording`, `gaps`, `refusal`, `pending_at_finish`) and `identities`. The property evaluates event bodies and progress directly. Ordinals refer to the replayed stream, with `recorded_ordinal` retaining the source position for recorded inputs. The `identities` map carries the content identities of the programs used for the evaluation.

The result object contains `result`, the `predicate` that decided a fail or an inconclusive, the `ordinal` it points at, the `completion_ordinal` of a pass, `unavailable` for the stronger claims the pass does not establish, `missing` for the next input an inconclusive needs, and a sentence.

Every property follows these rules.

- It evaluates only inputs and effects within its declared scope and completion rule, which are frozen in the case before any candidate runs. The reference program owns one completion rule per property and evaluates to the end of the recording; a case declaring another scope is refused as invalid input rather than evaluated under the program's own.
- Evidence about what the component observed is only an input the component consumed, bound to the callback in question. The goal's deadline expires on a clock observation its own callback consumed, and its send is the effect that callback produced after the command. A later command's goal never answers for an earlier one.
- Progress preconditions come first, so a component that emits nothing fails `goal_submitted` and a clock that never advances leaves the deadline unexpired and the result inconclusive, never a pass. A missing submission is inconclusive rather than a failure when the command's callback was still waiting on a recorded input at the end of the run, whether the recording stopped at a gap, a refusal, or simply closed; that pending wait is the named missing input.
- An effect counts only when it was attempted (`ATTEMPTED` flag) and went to the endpoint the scenario names. A cancel for the goal must go to the server the goal was sent to.
- A demonstrated failure (the cancel came before the deadline, the cancel never came though the deadline was seen to expire and the recording ran to its end) is a fail with the event named.
- Missing evidence before completion (a gap, a refusal, a recording that ends before the deadline) is inconclusive and names the next missing input.
- Completion at an effect is a pass even if later environmental behavior is unavailable; the pass lists what it does not establish.

The reference implements two properties.

- `goal-response-timeout-cancel@1` requires that a goal was submitted; a clock observation for that callback at or past the deadline was consumed with no acceptance before it; a cancel send for that goal (or the wildcard) followed. Completes at the cancel. It reports `cancel_acknowledged` and `goal_terminated` as unavailable. On the `timeout` recording, fixed passes at ordinal 6 of the replayed stream, old fails `cancel_request_issued` at ordinal 6, noop fails `goal_submitted`, always-cancel fails `cancel_after_deadline_expiry`. On `timeout-gap`, fixed and old are both inconclusive, naming the gap at recorded ordinal 6. On `command-gap` (the recorder lost events right after the command), fixed is inconclusive naming its pending clock read, and noop, which was waiting for nothing, fails `goal_submitted`.
- `timely-ack-no-cancel@1` requires that a goal was submitted; no cancel send for it inside the observation interval; an acceptance was consumed before any clock observation past the deadline; the interval reached the end of the recording. Old and fixed pass, noop fails `goal_submitted`, always-cancel fails `no_cancel_in_interval`.

The Nav2 case (#20) states its request property and the stronger goal-termination claim separately, exactly as the reference does.

## 12. Claims and grades on the native path

A native execution claim is always `measured`. A recording is Complete when the recorder captured every input class at the declared boundary, including clock observations and timer calls, with an End marker. It is Rebuilt when reconstructed code reproduces the recorded effects, Inferred when order, state or configuration was filled in or a gap is present, and Watch-only when there is a timeline without replay. The `case` file carries `grade_reason`.

After a native case passes integrity and decoding checks, `rook-verify` prints the four claim fields and `claim: measured`, then exits 2 because this build does not execute the adapter. The declared grade is reported without independently establishing capture completeness. The `rook verify CASE` command measures execution agreement on the tested adapter host.

## 13. Running the reference

```
cargo test -p rook-native -p rook-adapter-ref -p rook-verify
cargo run -p rook-verify -- fixtures/native-ref/timeout        # exit 2, four claims
cargo run -p rook-adapter-ref -- identity                      # content identities
printf '%s\n' '{"type":"start","protocol":1,"corpus":"adapter-ref@1","scenario":"timeout","state":{"method":"fresh","blake3":"af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"}}' '{"type":"finish","reason":"scope"}' | cargo run -q -p rook-adapter-ref -- serve --component fixed
```

Fixture regeneration requires explicit approval for the affected paths and expectations. The reference generator uses `ROOK_WRITE_FIXTURES=1 cargo test -p rook-adapter-ref --test reference -- --test-threads=1` and the same variable for `-p rook-native --test conformance`.

## 14. Reserved and unsupported

Timer frames, service and lifecycle events and the serialization codes other than raw are laid out and unused by the reference; #16 exercises them on a real process. Snapshot starting states are laid out and refused by the reference adapter. A response model for novel requests is not defined; a case that needs one names it and its identity. Nothing here records perception, physics, or a whole process; the component behind the adapter is the boundary.


## 15. Runner commands and local builds

Build both command binaries with `cargo build -p rook-verify --bins`, or install them with `cargo install --path crates/rook-verify`. `rook-verify` retains its existing standalone integrity checks and Wasm behavior. `rook verify` delegates `wasm-rmf-blockade` cases to the sibling `rook-verify` binary. Candidate testing supports `native-adapter` cases; other kinds are refused by name.

```sh
cargo run -p rook-verify --bin rook -- verify fixtures/native-ref/timeout
cargo run -p rook-verify --bin rook -- test fixtures/native-ref/timeout --candidate fixed
cargo run -p rook-verify --bin rook -- test fixtures/native-ref/timely --candidate always-cancel
```

The reference uses source-pinned adapter and property programs compiled into `rook`, each invoked in a separate child process. Its candidate names are `old`, `fixed`, `noop` and `always-cancel`. The runner checks the entire reference identity map against those compiled sources and pins the binary bytes in its report. It also checks the reference property, deadline configuration and scope. The existing reference fixtures and their expected bytes are unchanged.

### 15.1 External build descriptions

An external adapter case includes `build.json` in `MANIFEST.sha256`. It contains `identity`, `adapter`, `component`, `property`, `dependencies`, `adapter_args` and `property_args`. Unknown fields are refused. Each artifact has a `path` and a lowercase BLAKE3 content hash in `blake3`. Relative paths resolve beside the build description. Artifacts must be available locally. The identity map matches the case's entire `identity` file and pins `adapter_binary_blake3`, `component_binary_blake3`, `property_binary_blake3` and each named dependency's hash. The adapter owns which loaded libraries, plugins and configuration files its supported contract requires; those files belong in `dependencies`. The runner checks declared files, and does not discover undeclared dynamic dependencies.

`adapter_args` is a literal argument array containing a `{component}` entry, which the runner replaces with the resolved component artifact path. It invokes the adapter executable with that array and sends the start, step and finish messages from section 8. `property_args` is the fixed argument array for the property executable; the runner appends the path of a temporary `rook-property-input@1` JSON file. The property writes one `rook-property-result@1` line and exits with its matching result code. The runner removes the temporary file after evaluation. Neither invocation uses a shell.

`rook test CASE --candidate candidate-build.json` loads a second build description. Only the component artifact and the `component_variant`, `component_source_blake3` and `component_binary_blake3` identity entries may change. Adapter, property and dependency content identities, arguments, component name and every other identity stay fixed. Byte-identical protected artifacts may be stored at different local paths. The property input always uses the original case declaration. Altering the manifest-covered case, property, configuration or normalizer produces a different `case_id`, computed from the manifest bytes. The candidate description cannot override the case declaration.

The startup response must match the component, variant, adapter, protocol, starting state and complete endpoint and channel tables. The recording must carry exactly one matching starting-state marker before inputs. Effect frames retain order and multiplicity, must belong to an authorized callback and must use valid payloads and contiguous per-channel sequences. An effect confirms that the associated step input was consumed; a subsequent denial of consumption is a protocol refusal. Child response waits are limited to 10 seconds and adapter lines to 1 MiB. A child that cannot execute the protocol is refused with exit 2.

### 15.2 Declared goal-ID normalization

The runner supports `none@1` with `raw-bytes-in-order@1`, and `goal-ids-by-issuance@1` with `goal-ids-in-order@1`. Other combinations are invalid declarations. A normalized case pins `normalization_source_blake3` to BLAKE3 of `crates/rook-verify/src/normalization.rs` and declares `normalization.client.CHANNEL = CLIENT` identity entries. Every goal-issuance and cancellation channel must name its originating client. Separate clients need separate channels in this version. This is an explicit adapter-boundary requirement; the runner cannot recover a client identity absent from the evidence.

The nth issuance for a declared client establishes its correspondence. The mapping retains completed goals and allows overlapping goals. Duplicate issuance IDs are rejected. Responses are checked against the raw recorded ID of their named issuance before translation. Missing mappings and changed request payloads stop delivery of dependent responses. Unknown targeted cancellations are reported as demonstrated execution failures. Zero-ID cancellations keep their zero ID and timestamp, and targeted cancellations retain their timestamp too. The normalizer never changes result status, result payload, response order or request ordinals. Extra goals remain extra effects without invented correspondence.

### 15.3 Bounded evaluation and exit codes

The runner first attempts baseline reproduction and reports that result separately from the candidate property. An established baseline execution mismatch makes candidate testing fail, including a differing effect observed before a later gap. A gap without an established mismatch leaves baseline reproduction unavailable and allows the candidate property to identify its own missing evidence or demonstrated failure. The runner evaluates the candidate property at each emitted effect, so it can send `finish` with reason `scope` before a pending callback returns. Queued effects after completion remain visible but do not extend the completed property's claim.

For the reference timeout case, a valid cancel request completes the property while `cancel_response` remains pending. The report explicitly leaves `cancel_acknowledged` and `goal_terminated` unavailable. A different goal payload invalidates its later recorded acceptance, which is stopped before adapter delivery. An unrelated diagnostic difference stays visible while a fully evidenced property may pass. A no-op candidate fails required goal submission. A timely acknowledgment case rejects always-cancel as an observed failure.

- Exit 0 means supported baseline verification passed, or the candidate's bounded property passed with explicit completion evidence. Baseline verification may pass while the recorded behavior fails its property.
- Exit 1 means file integrity or execution disagreed, or the property demonstrated a failure, including missing required progress when the component was waiting for no additional input.
- Exit 2 means invalid invocation, malformed case, unknown or unexecutable kind, prohibited identity change, invalid normalization declaration, invalid property input or an unexecutable child protocol. A baseline recording with a declared gap and no established mismatch cannot establish execution agreement and returns 2.
- Exit 3 means candidate evidence needed before property completion is missing, unsupported or invalidated. It names the next missing input or dependency. A demonstrated property failure retains exit 1 even when later evidence is unavailable.

The property process translates `pass`, `fail`, `invalid` and `inconclusive` to 0, 1, 2 and 3 respectively. A disagreement between its JSON verdict and process exit is refused with exit 2. A passing result must name a completion ordinal present in observed evidence.

### 15.4 Reports

Every invocation writes one `rook-run-report@1` ndjson object to stdout and a readable local report to stderr. Redirect these streams to retain both reports. Errors also produce a JSON object with `exit_code`, `error` and four separate claim fields. The Wasm dispatch wraps the unchanged standalone report in `report` and its stderr in `diagnostic`.

Native reports include `case_id`, `case_path`, `recording_origin`, `recording_limits`, `platform`, `runner_binary_blake3`, `source_identity`, `candidate_identity`, `baseline_artifacts`, `candidate_artifacts`, `comparison_policy`, `normalization`, `normalization_source_blake3`, `scope`, `scope_completed`, `finish_reason`, `baseline_reproduced`, `baseline_comparison` and `property_result`. The property result carries its predicate, completion ordinal, unavailable later claims and next missing input as applicable. `baseline_reproduced` is null when a gap or refused input prevented a complete baseline run without an already established mismatch.

`file_integrity`, `execution_agreement`, `capture_completeness` and `incident_authenticity` remain separate. Native execution agreement is measured on the reported platform and binary. Capture completeness contains the declared grade, end marker and gap count with `independently_established` false. Incident authenticity remains outside the verifier.

`raw_record_blake3` and `effects_captured_blake3` identify untouched evidence files. `recorded_execution_hashes`, `raw_replay_execution_hashes` and `normalized_replay_execution_hashes` each contain `trace_hash`, `run_hash` and `output_digest`. `raw_effects_equal` compares unnormalized effects. `first_differing_effect` includes the first raw difference's index and both frames, with null for a missing side. `normalized_first_difference` identifies the first normalized effect difference. `execution_agreement` reports measured agreement only when the normalized run hash also agrees, preserving input/effect interleaving. When replay stops at a gap and the normalized effects before it agree, execution agreement is `unavailable`; a differing effect before the gap still reports `diverged`.

`recorded_effects` retains the raw recorded goal IDs. `observed_events` retains raw emitted effects and the inputs actually supplied to the adapter, including translated response IDs when declared. `observed_events` marks consumed inputs; `evidence` records gaps, refused dependencies and pending operations. The original case remains the source for untouched recorded input bodies. A report can therefore show normalized execution agreement while `raw_effects_equal` is false, without claiming raw byte equality.
