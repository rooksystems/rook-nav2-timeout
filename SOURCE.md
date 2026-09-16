# Source provenance

This experiment contains a source snapshot from Rook revision `be9b2125a71ea20f7dae4ab9bf3e231f541e53bc`. Rook source is licensed under Apache-2.0. Third-party source retains its own licenses and notices. The snapshot was copied from tracked files with fresh Git history.

The Rust workspace, test fixtures, and format specification are retained so readers can build the runner and check its existing behavior. The ROS workspace contains only the Nav2 timeout experiment. Its container setup, documentation, and CI are maintained here.

The format specification and historical ADRs retain their original text. References to the private Rook issue tracker describe development history; every file needed to build and verify this experiment is in this repository or fetched from the identified public upstream sources. The `rook verify` and `rook test` commands are implemented in the included runner, even where historical documents call them planned.
