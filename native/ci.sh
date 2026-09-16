#!/usr/bin/env bash
# Run inside the pinned Jazzy container through ../run.sh.
set -euo pipefail
cd /rook
if [[ "$(uname -m)" != x86_64 ]]; then
  echo "This experiment requires the documented linux/amd64 container." >&2
  exit 2
fi
export CARGO_TARGET_DIR=/rook/native/build/rust
export RMW_IMPLEMENTATION=rmw_fastrtps_cpp
source_dir=/rook/native/src/rook_nav2_experiment
build_dir=/rook/native/build/rook_nav2_experiment
python3 "$source_dir/retain_packages.py" before native/build/nav2-packages
install -m 0644 native/ros-snapshot-key.asc /usr/share/keyrings/rook-ros-snapshot.asc
cat > /etc/apt/sources.list.d/rook-ros-snapshot.list <<'EOF'
deb [signed-by=/usr/share/keyrings/rook-ros-snapshot.asc] http://snapshots.ros.org/jazzy/2026-06-18/ubuntu noble main
EOF
cat > /etc/apt/preferences.d/rook-ros-snapshot <<'EOF'
Package: ros-*
Pin: origin snapshots.ros.org
Pin-Priority: 1001

Package: ros-*
Pin: origin packages.ros.org
Pin-Priority: -1
EOF
apt-get update --error-on=any -q
DEBIAN_FRONTEND=noninteractive apt-get install -q -y --no-install-recommends \
  curl ca-certificates build-essential git pkg-config nlohmann-json3-dev \
  ros-jazzy-rclcpp ros-jazzy-rclcpp-action ros-jazzy-action-msgs \
  ros-jazzy-nav2-msgs ros-jazzy-rmw-fastrtps-cpp ros-jazzy-ament-cmake
python3 "$source_dir/retain_packages.py" after native/build/nav2-packages
mkdir -p "$build_dir/evidence"
dpkg-query -W > "$build_dir/evidence/packages.txt"
c++ --version > "$build_dir/evidence/compiler.txt"
set +u
source /opt/ros/jazzy/setup.bash
set -u
(cd native && colcon build --packages-select rook_nav2_experiment --event-handlers console_direct+)
ctest --test-dir "$build_dir" --verbose
# Install the repository-pinned Rust toolchain inside the disposable container.
# The host needs only Git and Docker; Rust is never installed on the host.
curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o /tmp/rook-rustup.sh
sh /tmp/rook-rustup.sh -y --profile minimal --default-toolchain none
source /root/.cargo/env
cargo build --locked -p rook-verify --bin rook --example nav2_pack
python3 "$source_dir/integrated.py" "$source_dir" "$build_dir" \
  "$CARGO_TARGET_DIR/debug/rook" "$CARGO_TARGET_DIR/debug/examples/nav2_pack"
python3 "$source_dir/bundle.py" "$source_dir" "$build_dir" \
  "$CARGO_TARGET_DIR/debug/rook" /rook/native/build/nav2-packages
python3 tools/summarize.py "$build_dir/integrated/matrix.json" | tee "$build_dir/evidence/summary.txt"
