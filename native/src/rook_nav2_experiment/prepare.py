#!/usr/bin/env python3
"""Fetch exact upstream class bytes and check the dependency-only integration diff."""
import difflib
import hashlib
from pathlib import Path
import sys
import urllib.request

SOURCES = {
    "old": ("a3a97043ee93d92fe8d70ec56d183933169beb1c", "56696fb81ffcebff8103713fda84385565242954dd5d5b27108f62a5817bfbf8"),
    "fixed": ("0f2777630471d03c65d73914c4b4a82224687a2c", "14d46b024fb5ef73732ddd192d8a8f2f777e857044432818ad8777a05faf5115"),
}
HEADER = "nav2_behavior_tree/include/nav2_behavior_tree/bt_action_node.hpp"
REPLACEMENTS = {
    '#include "nav2_ros_common/node_utils.hpp"': '#include "control.hpp"',
    '#include "rclcpp_action/rclcpp_action.hpp"\n': "",
    '#include "nav2_behavior_tree/bt_utils.hpp"\n': "",
    '#include "nav2_behavior_tree/json_utils.hpp"\n': "",
    "nav2::LifecycleNode": "control::Node",
    "nav2::ActionClient": "control::Client",
    "rclcpp_action::ClientGoalHandle": "control::GoalHandle",
    "rclcpp::executors::SingleThreadedExecutor": "control::Executor",
}


def prepare(destination):
    destination.mkdir(parents=True, exist_ok=True)
    patches = []
    for variant, (commit, digest) in SOURCES.items():
        original = destination / f"{variant}.upstream.hpp"
        if not original.exists():
            with urllib.request.urlopen(f"https://raw.githubusercontent.com/ros-navigation/navigation2/{commit}/{HEADER}", timeout=60) as response:
                data = response.read()
            # Write the whole download before it can be reused by a later configure.
            partial = original.with_suffix(".partial")
            partial.write_bytes(data)
            partial.replace(original)
        data = original.read_bytes()
        if hashlib.sha256(data).hexdigest() != digest:
            original.unlink()
            raise ValueError(f"{variant} upstream SHA-256 mismatch")
        source = data.decode()
        adapted = source
        for before, after in REPLACEMENTS.items():
            if before not in adapted:
                raise ValueError(f"{variant} missing dependency seam {before!r}")
            adapted = adapted.replace(before, after)
        adapted = "// Rook modification: dependency access only; see integration.diff.\n" + adapted
        patches.extend(difflib.unified_diff(source.splitlines(True), adapted.splitlines(True), fromfile=f"{variant}.upstream.hpp", tofile=f"{variant}.hpp", n=0))
        (destination / f"{variant}.hpp").write_text(adapted)
    return "".join(patches)


if __name__ == "__main__":
    generated = prepare(Path(sys.argv[1]))
    expected = Path(__file__).with_name("integration.diff")
    if len(sys.argv) == 3 and sys.argv[2] == "--write-diff":
        expected.write_text(generated)
    elif expected.read_text() != generated:
        raise SystemExit("dependency integration diff changed; inspect before building")
