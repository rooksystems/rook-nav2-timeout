#!/bin/bash
# Build the tap in a ROS 2 Humble environment with the installed RMF debs.
set -euo pipefail

TAP_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
BUILD_ROOT=${ROOK_TAP_BUILD_ROOT:-/rook_tap_build}
INSTALL_ROOT="$BUILD_ROOT/install"
BINARY="$INSTALL_ROOT/lib/rook_blockade_tap/rook_blockade_tap"

# ROS Humble's generated setup scripts are not safe under nounset.
set +u
source /opt/ros/humble/setup.bash
if [ -f /rmf_ws/install/setup.bash ]; then
  source /rmf_ws/install/setup.bash
fi
set -u

case "$BUILD_ROOT" in
  "$TAP_DIR"|"$TAP_DIR"/*|/|"")
    echo "Unsafe ROOK_TAP_BUILD_ROOT: $BUILD_ROOT" >&2
    exit 1
    ;;
esac
rm -rf "$BUILD_ROOT"
mkdir -p "$BUILD_ROOT/src/rook_blockade_tap"
cp -a \
  "$TAP_DIR/CMakeLists.txt" \
  "$TAP_DIR/package.xml" \
  "$TAP_DIR/include" \
  "$TAP_DIR/src" \
  "$BUILD_ROOT/src/rook_blockade_tap/"

colcon --log-base "$BUILD_ROOT/log" build \
  --merge-install \
  --base-paths "$BUILD_ROOT/src" \
  --build-base "$BUILD_ROOT/build" \
  --install-base "$INSTALL_ROOT" \
  --packages-select rook_blockade_tap \
  --cmake-args -DCMAKE_BUILD_TYPE=Release

test -x "$BINARY"
RMF_TRAFFIC_SO=$(ldd "$BINARY" | awk '$1 ~ /^librmf_traffic\.so/ { print $3; exit }')
if [ -z "$RMF_TRAFFIC_SO" ] || [ ! -f "$RMF_TRAFFIC_SO" ]; then
  echo "Could not resolve linked librmf_traffic.so with ldd" >&2
  ldd "$BINARY" >&2
  exit 1
fi

NODE_SHA256=$(sha256sum "$BINARY" | awk '{print $1}')
RMF_TRAFFIC_SHA256=$(sha256sum "$RMF_TRAFFIC_SO" | awk '{print $1}')
{
  printf '%s  %s\n' "$NODE_SHA256" "$BINARY"
  printf '%s  %s\n' "$RMF_TRAFFIC_SHA256" "$(readlink -f "$RMF_TRAFFIC_SO")"
} | tee "$TAP_DIR/BINSHA"
