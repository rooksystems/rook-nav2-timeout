#!/bin/bash
set -euo pipefail
TAP_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
PATCH="$TAP_DIR/tap.patch"
: > "$PATCH"

diff -u \
  --label upstream/include/rmf_traffic_ros2/blockade/Node.hpp \
  --label include/rmf_traffic_ros2/blockade/Node.hpp \
  "$TAP_DIR/upstream/include/rmf_traffic_ros2/blockade/Node.hpp" \
  "$TAP_DIR/include/rmf_traffic_ros2/blockade/Node.hpp" >> "$PATCH" || test $? -eq 1

diff -u \
  --label upstream/src/rmf_traffic_ros2/blockade/Node.cpp \
  --label src/Node.cpp \
  "$TAP_DIR/upstream/src/rmf_traffic_ros2/blockade/Node.cpp" \
  "$TAP_DIR/src/Node.cpp" >> "$PATCH" || test $? -eq 1

diff -u \
  --label upstream/src/rmf_traffic_blockade/main.cpp \
  --label src/main.cpp \
  "$TAP_DIR/upstream/src/rmf_traffic_blockade/main.cpp" \
  "$TAP_DIR/src/main.cpp" >> "$PATCH" || test $? -eq 1

diff -u \
  --label /dev/null \
  --label src/Recorder.hpp \
  /dev/null "$TAP_DIR/src/Recorder.hpp" >> "$PATCH" || test $? -eq 1

diff -u \
  --label /dev/null \
  --label src/Recorder.cpp \
  /dev/null "$TAP_DIR/src/Recorder.cpp" >> "$PATCH" || test $? -eq 1
