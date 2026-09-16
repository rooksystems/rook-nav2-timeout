#!/usr/bin/env python3
"""Check local Markdown destinations against the repository boundary.

Checks inline links, images and single-line reference definitions, including
angle-delimited destinations, escaped or balanced parentheses and link titles.
Fenced and inline code are skipped. URL reachability and heading anchors are
not checked. New non-ignored Markdown files are included in local runs.
"""
import re
import subprocess
import sys
from pathlib import Path
from urllib.parse import unquote, urlsplit

INLINE = re.compile(r"\]\(\s*")
REFERENCE = re.compile(r"^\s{0,3}\[[^\]]+\]:\s*")


def destination(text: str, start: int) -> str:
    """Read a destination up to its title or closing delimiter."""
    angle = text[start:start + 1] == "<"
    if angle:
        start += 1
    depth, index = 0, start
    while index < len(text):
        char = text[index]
        if char == "\\" and index + 1 < len(text):
            index += 2
            continue
        if angle:
            if char == ">":
                break
        elif char.isspace() or (char == ")" and depth == 0):
            break
        elif char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
        index += 1
    return re.sub(r"\\([!\"#$%&'()*+,\-./:;<=>?@\[\]\\^_`{|}~])", r"\1", text[start:index])


def targets(text: str):
    fence = None
    list_indents = []
    for number, line in enumerate(text.splitlines(), 1):
        line = line.expandtabs(4)
        # Quoted fences are code too; ordinary quoted links still get checked.
        line = re.sub(r"^(?: {0,3}> ?)+", "", line)
        indent = len(line) - len(line.lstrip(" "))
        if line.strip():
            while list_indents and indent < list_indents[-1]:
                list_indents.pop()
        base = list_indents[-1] if list_indents else 0
        line = line[base:]
        # List indentation belongs to the container, not to a code block.
        item = re.match(r"^ {0,3}(?:[-+*]|[0-9]{1,9}[.)]) +", line)
        if item and fence is None:
            list_indents.append(base + item.end())
            line = line[item.end():]
        marker = re.match(r"^\s{0,3}(`{3,}|~{3,})(.*)$", line)
        if marker:
            run, suffix = marker.groups()
            if fence is None:
                fence = run
            elif run[0] == fence[0] and len(run) >= len(fence) and not suffix.strip():
                fence = None
            continue
        if fence or line.startswith(("    ", "\t")):
            continue
        line = re.sub(r"(`+).*?\1", "", line)
        matches = list(INLINE.finditer(line))
        reference = REFERENCE.match(line)
        if reference:
            matches.append(reference)
        for match in matches:
            yield number, destination(line, match.end())


def problems(root: Path, path: Path) -> list[str]:
    """Resolve symlinks before accepting a destination as part of the tree."""
    relative = path.relative_to(root)
    if not path.resolve().is_relative_to(root):
        return [f"{relative}: Markdown file escapes the repository"]
    found = []
    for number, target in targets(path.read_text(encoding="utf-8")):
        url = urlsplit(target)
        if url.scheme or url.netloc or not url.path:
            continue
        candidate = (path.parent / unquote(url.path)).resolve()
        if not candidate.is_relative_to(root):
            found.append(f"{relative}:{number}: destination escapes repository: {target}")
        elif not candidate.exists():
            found.append(f"{relative}:{number}: missing destination: {target}")
    return found


def main(argv: list[str]) -> int:
    root = Path(argv[0]).resolve() if argv else Path(__file__).resolve().parent.parent
    out = subprocess.check_output(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z", "--", "*.md"], cwd=root
    )
    files = [root / name.decode() for name in dict.fromkeys(out.split(b"\0")) if name]
    files = [path for path in files if path.exists() or path.is_symlink()]
    broken = []
    for path in files:
        try:
            broken.extend(problems(root, path))
        except (OSError, UnicodeError, ValueError) as error:
            broken.append(f"{path.relative_to(root)}: cannot check links: {error}")
    for message in broken:
        print(message)
    print(f"links: {len(files)} files, {len(broken)} errors")
    return int(bool(broken))


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
