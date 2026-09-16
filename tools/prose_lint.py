#!/usr/bin/env python3
"""Check authored Markdown punctuation and flag words for editorial review.

Em dashes fail the gate, including in quotations and code examples. Upstream
legal text is exempt. The word list is advisory because context determines
whether a word is useful. Words inside code, URLs and quotations are skipped.
Untracked, non-ignored Markdown is included so new docs are checked locally.
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXEMPT = {
    "THIRD-PARTY.md",
    "cells/rmf-blockade/prebuilt/THIRD-PARTY.md",
    "cells/rmf-blockade/vendor/rmf_traffic/LICENSE.md",
    "cells/rmf-blockade/vendor/rmf_utils/LICENSE.md",
}
EM_DASH = "\u2014"
WORD_REVIEW = re.compile(
    r"\b(delve|delves|delving|leverage|leverages|leveraging|seamless|seamlessly|robust|robustly|"
    r"comprehensive|comprehensively|streamline|streamlines|streamlined|cutting-edge|game-changing|"
    r"revolutionize|revolutionizes|revolutionizing)\b",
    re.IGNORECASE,
)


def markdown_paths() -> list[Path]:
    out = subprocess.check_output(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z", "--", "*.md"], cwd=ROOT
    )
    paths = [ROOT / name.decode() for name in dict.fromkeys(out.split(b"\0")) if name]
    return [path for path in paths if path.exists() or path.is_symlink()]


def inspect(path: Path) -> tuple[list[str], list[str]]:
    errors, warnings = [], []
    relative = path.relative_to(ROOT)
    if not path.resolve().is_relative_to(ROOT):
        return [f"{relative}: Markdown file escapes the repository"], []
    fence = None
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if EM_DASH in line:
            errors.append(f"{relative}:{number}: em dash")
        marker = re.match(r"^\s{0,3}(`{3,}|~{3,})(.*)$", line)
        if marker:
            run, suffix = marker.groups()
            if fence is None:
                fence = run
            elif run[0] == fence[0] and len(run) >= len(fence) and not suffix.strip():
                fence = None
            continue
        if fence or line.lstrip().startswith(">"):
            continue
        prose = re.sub(r"(`+).*?\1|https?://\S+", "", line)
        match = WORD_REVIEW.search(prose)
        if match:
            warnings.append(f"{relative}:{number}: review word {match.group(0)!r}")
    return errors, warnings


def main(argv: list[str]) -> int:
    paths = [Path(a).absolute() for a in argv] if argv else markdown_paths()
    errors, warnings = [], []
    for path in paths:
        try:
            if str(path.relative_to(ROOT)) in EXEMPT:
                continue
            failed, advisory = inspect(path)
            errors.extend(failed)
            warnings.extend(advisory)
        except (OSError, UnicodeError, ValueError) as error:
            errors.append(f"prose: cannot check {path}: {error}")
    for message in errors + warnings:
        print(message)
    print(f"prose: {len(paths)} files, {len(errors)} errors, {len(warnings)} editorial warnings")
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
