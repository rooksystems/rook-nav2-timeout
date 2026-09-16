# Nav2 experiment workspace

This workspace contains only `rook_nav2_experiment`. Start with the [root walkthrough](../README.md#run-it-yourself); `./run.sh` supplies the container and runs [ci.sh](ci.sh). The script builds the component, runs CTest, installs the pinned Rust toolchain inside the container, captures cases, checks candidates, and retains an offline replay bundle.

The base image is pinned by digest. ROS packages come from the signed Jazzy snapshot dated June 18, 2026. The bundled snapshot signing key has fingerprint `4B63CF8FDE49746E98FA01DDAD19BAB3CBF125EA`. Ubuntu packages use the base image's configured repositories. Installed versions are recorded in `native/build/rook_nav2_experiment/evidence/packages.txt`; changed packages are retained for offline replay.

Build outputs remain under the ignored `native/build/`, `native/install/`, and `native/log/` directories. The Rust container build uses `native/build/rust/`, separate from host builds in `target/`. Linux Docker may create these files with root ownership because installation runs inside the container as root.

The [technical notes](src/rook_nav2_experiment/README.md) document source identities, dependency substitutions, supported checks, and limitations.
