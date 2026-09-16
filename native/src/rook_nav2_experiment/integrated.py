#!/usr/bin/env python3
"""Record once, package, verify repeatedly, and test candidates through rook."""
import json
from pathlib import Path
import re
import subprocess
import sys

from capture import capture


def run(args, expected=0):
    process = subprocess.run([str(a) for a in args], text=True, capture_output=True)
    if process.returncode != expected:
        print(process.stdout, flush=True)
        print(process.stderr, file=sys.stderr, flush=True)
        raise AssertionError(f'{args}: expected exit {expected}, got {process.returncode}')
    return process


def main(source, build, rook, packer):
    output = build / 'integrated'
    output.mkdir(exist_ok=False)
    runtime = {Path(sys.executable).resolve()}
    # Include the selected middleware's plugin dependencies as well as the
    # executable's loader closure, since the RMW implementation is dlopened.
    programs = [build / ('nav2_component_' + v) for v in ('old', 'fixed', 'noop', 'always-cancel')]
    programs += [Path('/opt/ros/jazzy/lib/librmw_fastrtps_cpp.so')]
    for binary in programs:
        if binary not in programs[:4]:
            runtime.add(binary.resolve())
        listing = run(['ldd', binary]).stdout
        if 'not found' in listing:
            raise AssertionError(f'{binary}: unresolved loader dependency\n{listing}')
        for path in re.findall(r'(?:=>\s+)?(/[^\s]+)', listing):
            runtime.add(Path(path).resolve())
    (build / 'runtime-paths.txt').write_text(''.join(str(p) + '\n' for p in sorted(runtime)))
    reports = []
    for scenario in ('timeout', 'timely', 'missing-clock', 'frozen-clock', 'overlap', 'wrong-result', 'cancel-ack', 'cancel-before-goal'):
        raw = output / (scenario + '-capture.json')
        raw.write_text(json.dumps(capture(str(build / 'nav2_component_old'), scenario), indent=2) + '\n')
        case = output / scenario
        run([packer, raw, source, build, case])
        # Fresh replay processes must agree with the same captured effects.
        for repeat in range(3):
            result = run([rook, 'verify', case], 1 if scenario == 'wrong-result' else 0)
            (output / f'{scenario}-verify-{repeat}.json').write_text(result.stdout)
            report = json.loads(result.stdout)
            if scenario == 'wrong-result':
                assert report['baseline_reproduced'] is None, report
                assert 'wrong or stale' in report['evidence']['refusal']['reason'], report
            else:
                assert report['baseline_reproduced'] is True, report
        matrix = {
            'timeout': [('fixed', 0), ('old', 1), ('noop', 1), ('always-cancel', 1)],
            'timely': [('fixed', 0), ('old', 0), ('noop', 1), ('always-cancel', 1)],
            'overlap': [('fixed', 0), ('old', 0), ('noop', 1), ('always-cancel', 1)],
            'cancel-ack': [('fixed', 0), ('old', 0), ('noop', 1)],
            'cancel-before-goal': [('fixed', 3), ('old', 1), ('noop', 1), ('always-cancel', 1)],
            'missing-clock': [('fixed', 3), ('old', 3), ('noop', 1), ('always-cancel', 1)],
            'frozen-clock': [('fixed', 3), ('old', 3), ('noop', 1), ('always-cancel', 1)],
            'wrong-result': [('old', 3)],
        }[scenario]
        for variant, expected in matrix:
            result = run([rook, 'test', case, '--candidate', case / (variant + '.json')], expected)
            (output / f'{scenario}-{variant}.json').write_text(result.stdout)
            report = json.loads(result.stdout)
            if scenario == 'wrong-result':
                assert report['baseline_reproduced'] is None, report
                assert 'wrong or stale' in report['evidence']['refusal']['reason'], report
            else:
                assert report['baseline_reproduced'] is True, report
            if expected:
                assert report['property_result'].get('predicate'), report
            if scenario == 'timeout' and variant == 'fixed':
                assert report['property_result']['unavailable'] == ['cancel_acknowledged', 'goal_terminated'], report
                assert any(p.get('name') == 'cancel_response' for p in report['evidence']['pending_at_finish']), report
            reports.append({'scenario': scenario, 'variant': variant, 'exit': expected, 'result': report['property_result']})
    (output / 'matrix.json').write_text(json.dumps(reports, indent=2) + '\n')
    print(json.dumps(reports, indent=2))


if __name__ == '__main__':
    main(*(Path(arg).resolve() for arg in sys.argv[1:]))
