#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
if [[ ! -f native/build/rook_nav2_experiment/replay-offline.sh ]]; then
  echo "Run ./run.sh first to build and capture the experiment." >&2
  exit 2
fi
# The original run pulled the base image. Never fetch during offline replay.
docker run --rm --pull never --network none --platform linux/amd64 \
  -v "$PWD/native/build/rook_nav2_experiment":/bundle:ro \
  ros@sha256:2589a8fba5257307857890173c069852c2abf913a0be7970f172478baecb09e4 \
  bash /bundle/replay-offline.sh
