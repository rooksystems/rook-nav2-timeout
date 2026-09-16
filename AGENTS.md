# Working on this experiment

Read the README and technical notes before editing. Keep changes within the Nav2 experiment and its required runner. Preserve unrelated local changes. Run `just gate` and the ROS workflow for relevant changes before claiming completion.

## A small glossary

"You" means the agent working in this repository. "We" means the maintainers. "Stranger" means a reader checking a result from public source and evidence without relying on our assurance. A case is the recording, code identity, expected result, and behavioral check. A claim describes execution agreement; a grade describes recording evidence.

## Hard rules

Customer recordings and private logs stay out of Git and hosted model sessions. Use synthetic inputs. Never publish private source history.

Existing expected outputs, hash-domain strings, `docs/internals/format-spec.md`, and the `rook-core` dependency list change only when Saketh names the affected files and requests the specific change. Never regenerate expectations merely to make a test pass. A hash-domain change requires its own commit and ADR.

Preserve byte-exact Wasm replay. Keep clocks, filesystem access, networking, process state, unsafe code, hash-based iteration, platform math, and formatting-based hashes out of the deterministic core. Native results are measured on a named build and platform; report raw and normalized comparisons separately.

Preserve the recording boundary and report unavailable evidence. A candidate may change decisions; evaluate its behavioral check separately from baseline agreement. Do not raise a grade or loosen a check to conceal a failure.

Keep third-party attribution and license texts. Stage explicit paths, inspect staged diffs, and keep commits free of attribution trailers. Write plain prose with straight quotes and no em dashes. Keep each prose paragraph on one source line.
