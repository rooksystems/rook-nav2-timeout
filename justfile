set shell := ["sh", "-cu"]

export CARGO_TARGET_DIR := justfile_directory() / "target"

cell_wasm := "target/wasm32-unknown-unknown/release/echo_count_cell.wasm"
core_wasm := "target/wasm32-unknown-unknown/release/rook_core_wasm.wasm"
rmf_cell_wasm := "cells/rmf-blockade/prebuilt/rmf_blockade_cell.wasm"

build-cell:
    RUSTFLAGS='-C link-arg=--initial-memory=2097152 -C link-arg=--max-memory=2097152 -C link-arg=-zstack-size=1048576' cargo build --release -p echo-count-cell --target wasm32-unknown-unknown

build-core-wasm:
    RUSTFLAGS='-C link-arg=--initial-memory=67108864 -C link-arg=--max-memory=67108864' cargo build --release -p rook-core-wasm --target wasm32-unknown-unknown

# Needs WASI_SDK_PATH (and EIGEN_INCLUDE if not Homebrew). Rewrites the
# committed artifact and its SHA-256; a rebuild from the same toolchain must
# reproduce the committed hash.
build-rmf-cell:
    cells/rmf-blockade/build.sh

# Builds the same shim + vendored moderator natively and byte-compares its
# heartbeats with the wasm cell's on the same recording. Evidence only; the
# guarantee is wasm-to-wasm.
rmf-native-check:
    cells/rmf-blockade/native/build.sh
    cargo run --release -p rook-host -- rmf {{rmf_cell_wasm}} --dump target/rmf-dump
    target/rmf-native/driver target/rmf-dump/deliveries.bin target/rmf-dump/heartbeats.native.bin 0.2617993877991494
    cmp target/rmf-dump/heartbeats.bin target/rmf-dump/heartbeats.native.bin && echo native heartbeats == wasm heartbeats

gate: build-cell build-core-wasm
    cargo fmt --all -- --check
    cargo test -p rook-core -p rook-host -p rook-verify -p rook-native -p rook-adapter-ref
    cargo clippy --workspace --all-targets -- -D warnings
    cargo run -p rook-host -- {{cell_wasm}} {{core_wasm}}
    test "$(shasum -a 256 {{rmf_cell_wasm}} | cut -d' ' -f1)" = "$(cat cells/rmf-blockade/prebuilt/SHA256)"
    cargo run -p rook-host -- rmf {{rmf_cell_wasm}} --goldens cells/rmf-blockade/goldens/synthetic.golden
    python3 tools/reorder_lifecycle.py --self-test
    python3 tools/third_party.py --check
    python3 tools/prose_lint.py
    python3 tools/link_check.py
    cargo run -p rook-host -- rmf-bag {{rmf_cell_wasm}} cells/rmf-blockade/fixtures/battle-royale-bag --goldens cells/rmf-blockade/goldens/battle-royale-bag.golden
    cargo run -p rook-host -- rmf-bag {{rmf_cell_wasm}} cells/rmf-blockade/fixtures/battle-royale-tap --goldens cells/rmf-blockade/goldens/battle-royale-tap.golden
    cargo run -p rook-host -- rmf-bag {{rmf_cell_wasm}} cells/rmf-blockade/fixtures/battle-royale-timer --goldens cells/rmf-blockade/goldens/battle-royale-timer.golden
    cargo run -p rook-host -- rmf-bag {{rmf_cell_wasm}} cells/rmf-blockade/fixtures/battle-royale-holdout --goldens cells/rmf-blockade/goldens/battle-royale-holdout.golden
    # The public capsule must mirror the holdout fixture byte for byte.
    cmp capsules/rmf-blockade-holdout/deliveries.bin cells/rmf-blockade/fixtures/battle-royale-holdout/deliveries.bin
    cmp capsules/rmf-blockade-holdout/heartbeats_recorded.bin cells/rmf-blockade/fixtures/battle-royale-holdout/heartbeats_recorded.bin
    cmp capsules/rmf-blockade-holdout/params cells/rmf-blockade/fixtures/battle-royale-holdout/params
    cmp capsules/rmf-blockade-holdout/BINSHA cells/rmf-blockade/fixtures/battle-royale-holdout/BINSHA
    cmp capsules/rmf-blockade-holdout/PROVENANCE.md cells/rmf-blockade/fixtures/battle-royale-holdout/PROVENANCE.md
    cmp capsules/rmf-blockade-holdout/rmf_blockade_cell.wasm cells/rmf-blockade/prebuilt/rmf_blockade_cell.wasm
    cmp capsules/rmf-blockade-holdout/rmf_blockade_cell.wasm.sha256 cells/rmf-blockade/prebuilt/SHA256
    cargo run -p rook-verify -- capsules/rmf-blockade-holdout
    # A native-adapter capsule is integrity-checked and refused with exit 2;
    # this build reports file integrity but never executes its adapter.
    cargo run -p rook-verify -- fixtures/native-ref/timeout; test "$?" -eq 2

verify-capsule:
    cargo build --release -p rook-verify
    target/release/rook-verify capsules/rmf-blockade-holdout
