# Open-RMF replay case

This case tests replay of Open-RMF's blockade moderator, the decision component that grants robots access to shared space. Four simulated robots followed crossing routes in the `battle_royale` scenario. It is an early Rook experiment.

The recording contains 1,516 input deliveries, including 829 timer firings, and 1,495 recorded heartbeats over an 828-second span. The case includes the configuration, prebuilt WebAssembly cell and expected results. [PROVENANCE.md](PROVENANCE.md) describes the recording and component sources; `BINSHA` identifies the binaries used during capture.

## Run the verifier

From a repository checkout, follow the [contributor setup](../../CONTRIBUTING.md#first-checkout), then run the case.

```sh
just verify-capsule
```

This builds the release verifier and checks the bundled case. The first build downloads dependencies. The built verifier can then run with local files, without ROS, a simulator or a network connection.

```sh
./target/release/rook-verify capsules/rmf-blockade-holdout
```

A successful run exits 0 and ends with these lines.

```text
states: 672/672
verified: replay matches the recorded decisions and the expected hashes
```

The verifier checks the listed files against `MANIFEST.sha256`, confirms required files are covered, and compares replayed decision payloads with recorded payloads in order. It also checks the recorded heartbeat file's SHA-256, the expected replay hashes and the distinct-state count. Recording timestamps and replay ticks are not themselves compared as identical payload bytes.

It prints `bag_run_hash` and `bag_output_digest`; compare them with [expected](expected) for this checkout. Exit 1 reports a mismatch. Exit 2 reports a refusal or an input-processing error.

The Wasm guest has no filesystem or network imports. The verifier process retains the permissions of the account running it. Follow [SECURITY.md](../../SECURITY.md) for untrusted cases and the limits of manifest checking.

## Try the tamper check

Run these commands from the repository root in the same shell. Copy the case into a fresh temporary directory, then change a known byte in a recorded decision payload.

```sh
case_copy="$(mktemp -d)"
cp -R capsules/rmf-blockade-holdout/. "$case_copy/"
python3 - "$case_copy/heartbeats_recorded.bin" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
data = bytearray(path.read_bytes())
data[13227] ^= 0xff
path.write_bytes(data)
PY
./target/release/rook-verify "$case_copy"
```

The verifier exits 2 at the manifest check and names `heartbeats_recorded.bin`. Next, update that file's manifest entry in the temporary copy so the changed bytes pass the integrity check.

```sh
python3 - "$case_copy" <<'PY'
from pathlib import Path
import hashlib
import sys

case = Path(sys.argv[1])
name = "heartbeats_recorded.bin"
digest = hashlib.sha256((case / name).read_bytes()).hexdigest()
manifest = case / "MANIFEST.sha256"
lines = manifest.read_text().splitlines()
manifest.write_text("\n".join(
    f"{digest}  {name}" if line.endswith(f"  {name}") else line
    for line in lines
) + "\n")
PY
./target/release/rook-verify "$case_copy"
```

This time replay reaches the comparison and exits 1, reporting `first divergent tick: 1787185222610908825`. The recorded heartbeat file also differs from the SHA-256 in `expected`. The experiment shows which checks catch this particular edit. Someone who replaces all evidence and expectations together can supply a different, internally consistent case.

## What the result establishes

`bag_run_hash` covers canonical input and replayed-effect envelopes, including their ordering. `bag_output_digest` covers the replayed-effect envelopes. The [format spec](../../docs/internals/format-spec.md) defines the bytes and hash domains; [Architecture](../../ARCHITECTURE.md#what-the-result-establishes) explains the separate roles of integrity, replay agreement and behavioral checks.

This case checks reproduction of the recorded moderator decisions. The Wasm guarantee applies to the same complete recording, engine version and cell bytes on supported machines. The recording is graded Complete based on the capture argument in its provenance and [ADR 0005](../../docs/adr/0005-timer-inputs-close-experiment-2.md). The verifier does not independently establish the recording's origin or completeness.

The replay boundary covers moderator inputs and decisions. Perception, physical motion, radio behavior and the rest of the robot stack remain outside it. A successful run also leaves the separate question of whether a proposed component fix prevents a failure to a behavioral check.

## Publication status

This repository is currently private. [#11](https://github.com/rooksystems/Rook/issues/11) owns public packaging, independently published manifest and binary hashes, and the outside verification exercise. Until that is completed, use this as a checkout walkthrough. A hash supplied only inside the case cannot establish agreement with an independently published artifact.
