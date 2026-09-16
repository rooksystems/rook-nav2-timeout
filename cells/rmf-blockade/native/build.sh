#!/bin/sh
# Native build of the same shim + vendored moderator, for the wasm-vs-native
# comparison only. Uses whatever clang++ is on PATH.
set -eu
cd "$(dirname "$0")/.."
: "${EIGEN_INCLUDE:=/opt/homebrew/include/eigen3}"
mkdir -p ../../target/rmf-native
clang++ -std=gnu++17 -O2 -DNDEBUG -Wno-c++11-narrowing-const-reference -include cassert \
  -I vendor/rmf_traffic/include -I vendor/rmf_traffic/src -I vendor/rmf_utils/include \
  -isystem "$EIGEN_INCLUDE" \
  -o ../../target/rmf-native/driver \
  native/driver.cpp src/cell.cpp \
  vendor/rmf_traffic/src/blockade/Moderator.cpp \
  vendor/rmf_traffic/src/blockade/Constraint.cpp \
  vendor/rmf_traffic/src/blockade/conflicts.cpp \
  vendor/rmf_traffic/src/blockade/geometry.cpp
echo built ../../target/rmf-native/driver
