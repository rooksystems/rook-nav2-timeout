#!/usr/bin/env python3
"""Retain exactly the packages changed from the digest-pinned base image."""
import json
from pathlib import Path
import subprocess
import sys


def installed():
    text = subprocess.check_output(['dpkg-query', '-W', '-f=${binary:Package}\t${Version}\n'], text=True)
    return dict(line.split('\t', 1) for line in text.splitlines())


def main(mode, directory):
    directory.mkdir(parents=True, exist_ok=True)
    if mode == 'before':
        (directory / 'base.json').write_text(json.dumps(installed(), indent=2) + '\n')
    elif mode == 'after':
        before = json.loads((directory / 'base.json').read_text())
        after = installed()
        changed = {name: version for name, version in after.items() if before.get(name) != version}
        (directory / 'installed.json').write_text(json.dumps(after, indent=2) + '\n')
        (directory / 'changed.json').write_text(json.dumps(changed, indent=2) + '\n')
        archives = directory / 'debs'
        archives.mkdir(exist_ok=False)
        if changed:
            subprocess.run(['apt-get', 'download', *(name + '=' + version for name, version in sorted(changed.items()))], cwd=archives, check=True)
    else:
        raise ValueError('expected before or after')


if __name__ == '__main__':
    main(sys.argv[1], Path(sys.argv[2]).resolve())
