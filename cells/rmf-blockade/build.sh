#!/bin/sh
# Builds cells/rmf-blockade into a wasm32 cell with the ten-function rook ABI.
#
# Requires: WASI_SDK_PATH (tested with wasi-sdk 33.0, clang 22) and
# EIGEN_INCLUDE (tested with Eigen 5.0.1). Everything else is vendored.
# The output is committed at prebuilt/rmf_blockade_cell.wasm together with its
# SHA-256 in prebuilt/SHA256; `just gate` runs the committed artifact so the
# gate has no C++ toolchain dependency. Rebuild here, then diff the hash.
set -eu
cd "$(dirname "$0")"
: "${WASI_SDK_PATH:?set WASI_SDK_PATH to a wasi-sdk install}"
: "${EIGEN_INCLUDE:=/opt/homebrew/include/eigen3}"
mkdir -p prebuilt
# Exceptions use the standardized exnref/try_table encoding (wasmtime rejects
# the legacy `try` instruction that this clang still emits by default).
# 16 MiB fixed linear memory, 1 MiB stack. The host checks the maximum.
# -Wno-c++11-narrowing-const-reference: upstream keys some maps by size_t
#   (32-bit here) while ids are uint64_t; the shim aborts on ids >= 2^32
#   instead of letting them alias. -include cassert: geometry.cpp uses assert
#   without including it (older Eigen pulled it in transitively).
"$WASI_SDK_PATH/bin/clang++" \
  --target=wasm32-wasip1 -std=gnu++17 -O2 -DNDEBUG \
  -fwasm-exceptions -mllvm -wasm-use-legacy-eh=false -mexec-model=reactor \
  -fno-ident -ffile-prefix-map="$PWD"=. \
  -I vendor/rmf_traffic/include -I vendor/rmf_traffic/src \
  -I vendor/rmf_utils/include -isystem "$EIGEN_INCLUDE" \
  -Wall -Wno-unused-function \
  -Wno-c++11-narrowing-const-reference -include cassert \
  -Wl,--no-entry -Wl,--strip-all \
  -Wl,--initial-memory=16777216 -Wl,--max-memory=16777216 -Wl,-z,stack-size=1048576 \
  -lunwind \
  -o prebuilt/rmf_blockade_cell.wasm \
  src/cell.cpp \
  vendor/rmf_traffic/src/blockade/Moderator.cpp \
  vendor/rmf_traffic/src/blockade/Constraint.cpp \
  vendor/rmf_traffic/src/blockade/conflicts.cpp \
  vendor/rmf_traffic/src/blockade/geometry.cpp
shasum -a 256 prebuilt/rmf_blockade_cell.wasm | awk '{print $1}' > prebuilt/SHA256
cat prebuilt/SHA256
