#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
if ! command -v docker >/dev/null; then
  echo "Docker is required. Install Docker Engine or Docker Desktop, then run ./run.sh again." >&2
  exit 2
fi
# Keep all generated files in the ignored native build directory. A fresh
# capture must never replace a previous case and its recorded identities.
if [[ -e native/build/rook_nav2_experiment/integrated ]]; then
  echo "A captured run already exists in native/build/rook_nav2_experiment/integrated." >&2
  echo "Use ./replay.sh to check it again, or use a fresh checkout for another capture." >&2
  exit 2
fi
docker run --rm --platform linux/amd64 \
  -v "$PWD":/rook -w /rook \
  ros@sha256:2589a8fba5257307857890173c069852c2abf913a0be7970f172478baecb09e4 \
  bash native/ci.sh
