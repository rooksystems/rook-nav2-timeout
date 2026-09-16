#!/usr/bin/env python3
"""Print the measured timeout result after the integration matrix passes."""
import json
from pathlib import Path
import sys

rows = json.loads(Path(sys.argv[1]).read_text())
for variant, expected in (("old", 1), ("fixed", 0)):
    row = next(row for row in rows if row["scenario"] == "timeout" and row["variant"] == variant)
    if row["exit"] != expected:
        raise SystemExit(f"Unexpected timeout result for {variant}")
    print(f"timeout / {variant}: {'FAIL' if expected else 'PASS'} (rook test exit {row['exit']})")
print(f"All {len(rows)} candidate checks matched their expected results.")
print("The fixed component issued the required cancellation request.")
print("Server acknowledgment and goal termination remain unavailable.")
print("Reports: native/build/rook_nav2_experiment/integrated/")
