#!/usr/bin/env python3
"""Bundle captured cases and exact replay artifacts without executing them."""
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tarfile


def main(source, build, rook, packages):
    paths = {rook.resolve()}
    repository = source.parents[2]
    for case in (build / 'integrated').iterdir():
        if not case.is_dir():
            paths.add(case.resolve())
            continue
        paths.update(p.resolve() for p in case.iterdir())
        for variant in ('old', 'fixed', 'noop', 'always-cancel'):
            manifest = json.loads((case / (variant + '.json')).read_text())
            for name in ('adapter', 'component', 'property'):
                paths.add(Path(manifest[name]['path']).resolve())
            paths.update(Path(a['path']).resolve() for key, a in manifest['dependencies'].items()
                         if not key.startswith('dependency.runtime.') or Path(a['path']).is_relative_to(repository))
    paths.update(p.resolve() for p in source.iterdir() if p.is_file())
    paths.update(p.resolve() for p in (build / 'upstream').iterdir() if p.is_file())
    paths.update(p.resolve() for p in packages.rglob('*') if p.is_file())
    tracked = subprocess.check_output(['git', '-c', f'safe.directory={repository}', 'ls-files', '-z', '--', 'crates', 'cells', 'fixtures',
        'docs/internals', 'docs/glossary.md', 'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml',
        'LICENSE', 'NOTICE', 'THIRD-PARTY.md'], cwd=repository).decode().split('\0')
    paths.update((repository / name).resolve() for name in tracked if name)
    paths.update(p.resolve() for p in (build / '_deps/btcpp-src').rglob('*') if p.is_file())
    registry = Path(os.environ.get('CARGO_HOME', str(Path.home() / '.cargo'))) / 'registry/src'
    paths.update(p.resolve() for p in registry.rglob('*') if p.is_file()
                 and p.name.upper().startswith(('LICENSE', 'COPYING', 'NOTICE', 'COPYRIGHT')))
    # Paths retain their capture-machine locations inside the disposable pinned
    # container. The original case manifests and case identities stay unchanged.
    archive = build / 'nav2-replay.tar.gz'
    with tarfile.open(archive, 'w:gz', dereference=True) as output:
        for path in sorted(paths):
            output.add(path, arcname=str(path).lstrip('/'), recursive=False)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (build / 'nav2-replay.sha256').write_text(digest + '  nav2-replay.tar.gz\n')
    # Generate replay checks from the matrix that just passed, including the
    # baseline refusal status for deliberately invalid response evidence.
    rows = json.loads((build / 'integrated/matrix.json').read_text())
    checks = []
    for scenario in dict.fromkeys(row['scenario'] for row in rows):
        case = build / 'integrated' / scenario
        baseline = json.loads((build / f'integrated/{scenario}-verify-0.json').read_text())
        checks.append(shlex.join(['check_exit', str(baseline['exit_code']), str(rook), 'verify', str(case)]))
        for row in (row for row in rows if row['scenario'] == scenario):
            checks.append(shlex.join(['check_exit', str(row['exit']), str(rook), 'test', str(case),
                                     '--candidate', str(case / (row['variant'] + '.json'))]))
    recipe = f'''#!/usr/bin/env bash
set -euo pipefail
# Run only inside the documented disposable, digest-pinned Jazzy container.
# Extraction restores the captured absolute paths; it never changes a manifest.
(cd /bundle && sha256sum -c nav2-replay.sha256)
tar -xzf /bundle/nav2-replay.tar.gz -C /
dpkg -i {packages}/debs/*.deb
set +u
source /opt/ros/jazzy/setup.bash
set -u
export RMW_IMPLEMENTATION=rmw_fastrtps_cpp
check_exit() {{
  local expected="$1" actual=0
  shift
  "$@" || actual=$?
  test "$actual" -eq "$expected"
}}
{chr(10).join(checks)}
'''
    (build / 'replay-offline.sh').write_text(recipe)
    # Check the same files after packing; a missing path fails before publishing
    # an artifact advertised as replayable.
    subprocess.run([str(rook), 'verify', str(build / 'integrated/timeout')], check=True)


if __name__ == '__main__':
    main(*(Path(arg).resolve() for arg in sys.argv[1:]))
