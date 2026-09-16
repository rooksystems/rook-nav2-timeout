# Security

Rook processes recordings and runs decision code supplied with a case. A security failure can expose the machine doing that verification or make an unsupported result look trustworthy. This policy explains the boundaries we rely on and how to report a failure.

## What you are trusting

The verifier, its dependencies and the operating system are part of the trusted environment. Rebuilding the verifier lets you check a result with source you can inspect. It still uses the same replay implementation as the host, so an implementation bug can affect both.

A Wasm cell runs through the registered Rook imports. Those imports supply recorded inputs, configuration and logical time. They provide no access to the host's wall clock, filesystem or network. The host links no WASI imports and disables guest threads. Wasmtime and the Rook host enforce this boundary, including guest memory and execution-fuel limits.

The verifier process retains the permissions of the account running it. It does not install an operating-system sandbox or disable that account's network access. Guest limits also leave case loading and module compilation outside a total process resource budget. For an untrusted case, use an isolated environment with access limited to the files it needs, network access disabled and operating-system resource limits.

Manifest entries must name regular files directly inside the case directory. The verifier rejects path separators, duplicate entries and entries that are symlinks. That protection has limits in the current build. `MANIFEST.sha256` itself is opened through ordinary filesystem calls, and case files are reopened after hashing. Keep the case directory and its parent paths under your control and unchanged throughout verification. The current implementation does not protect against another process replacing files between those reads.

Native adapters and components run as native code with the permissions of their execution environment. An adapter's recording contract describes how to reproduce behavior; operating-system isolation must supply any restriction on that code's access. The standalone verifier currently checks native case data without executing the adapter.

## How far verification reaches

The manifest checks whether listed files match the SHA-256 values supplied with the case and whether every required file is covered. For the supported Wasm case, the verifier also replays the cell and compares decision payloads, expected hashes and the distinct-state count.

For a native case, the verifier checks integrity and framing and recomputes the recording's hashes. It reports execution agreement as unavailable and exits 2. The declared recording grade remains unverified by this build. [ARCHITECTURE.md](ARCHITECTURE.md) explains the execution paths and the separate behavioral check used when testing a fix.

A case supplies its own expectations. Someone who replaces the recording, cell and expectations together can create a different, internally consistent case. To establish agreement with a published artifact, compare against reference hashes obtained from a source you trust independently of the supplied case. Even that agreement leaves the recording's claimed origin and completeness to separate evidence. Replay covers the recorded component boundary; it cannot establish the safety of the whole robot.

## What to report privately

Report a way to make Rook accept evidence that fails its checks, hide an execution difference, or present an unchecked claim as established. Examples include skipping an event that the contract requires hashing, accepting a missing required file, or reporting native execution agreement without executing the component.

Also report a way for case contents or cell execution to expose host data, execute unintended host code, bypass enforced resource limits or cause unintended filesystem or network access. Suspected tampering with distributed binaries, source or build dependencies belongs here too. Include the affected artifact and how you obtained it.

A correctly reported divergence or a clean refusal is expected behavior. A valid case that is rejected is usually an ordinary bug. The exit code alone cannot classify a security problem; a process can refuse after already doing something unsafe. If you suspect a security impact, report it privately even if you are unsure which category fits.

## Reporting

Email [saketh@rookreplay.com](mailto:saketh@rookreplay.com) to report a suspected vulnerability. Keep suspected vulnerabilities out of public issues while we investigate.

Include the commit or release, operating system and architecture, commands, observed result and the result you expected. Explain the security impact and attach the smallest synthetic case that reproduces it if you can. Never send customer recordings, credentials or other private material. Check command output for those too.

We will review the report and explain what we find. We can arrange disclosure and credit with you if a fix is needed. There is no guaranteed response time or bug bounty.
