#!/usr/bin/env python3
"""Prepare, run sequentially and compare the pinned M4 CTS gate's official XML."""
import argparse
import collections
import fcntl
import datetime
import hashlib
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import sys
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parent.parent
HARNESS = ROOT / '_build/cts-tradefed/android-cts'
MANIFEST = ROOT / 'tools/cts-pm-modules.json'


def modules():
    return json.loads(MANIFEST.read_text())['modules']


def pins(file):
    for line in (ROOT / 'upstream' / file).read_text().splitlines():
        if line.startswith('"'):
            yield line.strip('"').split('|')


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def prepare():
    """Only materialize pinned inputs; do not authorize, install or start a guest."""
    wanted = {m['name'] for m in modules() if m['kind'] != 'host'}
    count = 0
    for entry, sha in pins('cts.lock'):
        parts = entry.split('/')
        if len(parts) < 4 or parts[2] not in wanted:
            continue
        source = ROOT / '_build/cts' / parts[-1]
        if not source.is_file() or digest(source) != sha:
            raise ValueError(f'missing or corrupt pinned device input: {entry}')
        target = HARNESS.parent / entry
        if target.exists():
            if not target.is_file() or digest(target) != sha:
                raise ValueError(f'harness input differs: {target}')
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.symlink_to(source.resolve())
        count += 1
    for entry, sha in pins('cts-tradefed.lock'):
        path = HARNESS.parent / entry
        if not path.is_file() or digest(path) != sha:
            raise ValueError(f'missing or corrupt pinned harness input: {entry}')
    for module in modules():
        if not (HARNESS / 'testcases' / module['name'] / (module['name'] + '.config')).is_file():
            raise ValueError(f'missing config for {module["name"]}')
    return count


def summarize(path):
    tree = ET.parse(path).getroot()
    if (tree.tag != 'Result' or tree.attrib.get('suite_name') != 'CTS'
            or tree.attrib.get('suite_version') != '16_r1'
            or tree.attrib.get('suite_build_number') != '13467367'):
        raise ValueError(f'not pinned CTS16_r1 official result XML: {path}')
    rows = []
    for module in tree.findall('Module'):
        name, abi = module.attrib['name'], module.attrib.get('abi', '')
        counts = collections.Counter()
        tests = []
        for case in module.findall('TestCase'):
            for test in case.findall('Test'):
                key = (case.attrib['name'], test.attrib['name'])
                status = test.attrib.get('result', 'not_executed')
                counts[status] += 1
                tests.append({'class': key[0], 'name': key[1], 'result': status,
                              'failure': '\n'.join((n.text or '') for n in test.findall('.//StackTrace'))})
        identities = [(t['class'], t['name']) for t in tests]
        if len(set(identities)) != len(identities):
            raise ValueError(f'duplicate test identity: {name}/{abi}')
        rows.append({'name': name, 'abi': abi, 'done': module.attrib.get('done') == 'true',
                     'declared_total': module.attrib.get('total_tests'),
                     'counts': dict(counts), 'tests': tests})
    return {'path': str(path), 'release': tree.attrib['suite_version'],
            'command': tree.attrib.get('command_line_args'), 'modules': rows}


def result_for(module, before):
    candidates = []
    for path in (HARNESS / 'results').glob('*/test_result.xml'):
        if path.parent.is_symlink() or str(path.parent) in before:
            continue
        result = summarize(path)
        args = shlex.split(result['command'] or '')
        if any(args[i] in ('-m', '--module') and i + 1 < len(args) and args[i + 1] == module for i in range(len(args))):
            candidates.append(path)
    if len(candidates) != 1:
        raise ValueError(f'{module}: expected one new official XML, found {len(candidates)}')
    return candidates[0]


def save(path, value):
    temp = path.with_suffix(path.suffix + '.partial')
    temp.write_text(json.dumps(value, indent=2) + '\n')
    temp.replace(path)


def run(args):
    if args.output.exists():
        raise ValueError('campaign output already exists; preserve the prior evidence')
    if args.label not in ('original', 'native'):
        raise ValueError('campaign must identify original or native')
    services = (args.image / 'system/etc/aim/native-services').read_text().splitlines()
    native = {line.split()[0] for line in services if line.strip() and not line.lstrip().startswith('#')}
    if ('package' in native) != (args.label == 'native'):
        raise ValueError('image native-services does not match campaign label')
    if any(v.split('=', 1)[0] in ('-m', '--module', '-t', '--test', '--include-filter',
                                  '--exclude-filter', '--retry', '--skip-all-system-status-check',
                                  '--skip-system-status-check') for v in args.cts_args):
        raise ValueError('module/test filtering or retry changes the complete batch scope')
    prepare()
    args.output.mkdir(parents=True)
    plan = [m['name'] for m in modules()]
    state = {'label': args.label, 'release': '16_r1', 'image': str(args.image.resolve()),
             'data': str(args.data.resolve()), 'port': args.port, 'cts_args': args.cts_args,
             'manifest_sha256': digest(MANIFEST), 'modules': plan, 'runs': [],
             'started': datetime.datetime.now(datetime.timezone.utc).isoformat()}
    receipt = args.image / '.overlay-receipt'
    state['image_receipt_sha256'] = digest(receipt) if receipt.is_file() else None
    save(args.output / 'campaign.json', state)
    for index, module in enumerate(plan):
        before = {str(p) for p in (HARNESS / 'results').glob('*') if p.is_dir()}
        log = args.output / f'{index + 1:02d}-{module}.log'
        command = [str(ROOT / 'tools/cts-tradefed.sh'), str(args.data), str(args.port), '-m', module, *args.cts_args]
        with log.open('w') as output:
            child = subprocess.Popen(command, stdout=output, stderr=subprocess.STDOUT)
            try:
                code = child.wait(timeout=args.timeout)
            except (subprocess.TimeoutExpired, KeyboardInterrupt):
                # Only our wrapper PID; its trap cleans its own harness group.
                child.send_signal(signal.SIGTERM)
                child.wait()
                state['runs'].append({'module': module, 'status': 'not-run-complete', 'log': str(log)})
                save(args.output / 'campaign.json', state)
                raise
        row = {'module': module, 'wrapper_exit': code, 'log': str(log)}
        try:
            xml = result_for(module, before)
            result = summarize(xml)
            copied = args.output / f'{index + 1:02d}-{module}.xml'
            copied.write_bytes(xml.read_bytes())
            row.update(status='recorded', xml=str(copied), summary=result)
            if (not result['modules'] or not any(m['name'] == module for m in result['modules'])
                    or any(not m['done'] or not m['tests']
                           or int(m['declared_total'] or -1) != len(m['tests'])
                           for m in result['modules'])):
                row['status'] = 'not-run-complete'
        except (ValueError, ET.ParseError) as error:
            row.update(status='not-run-complete', error=str(error))
        state['runs'].append(row)
        save(args.output / 'campaign.json', state)
        print(module, row['status'], flush=True)
    return 0


def compare(original, native):
    expected = {m['name'] for m in modules()}
    def load(path):
        campaign = json.loads((path / 'campaign.json').read_text())
        if set(campaign['modules']) != expected:
            raise ValueError('campaign module scope differs from the current pinned gate')
        if campaign['manifest_sha256'] != digest(MANIFEST):
            raise ValueError('campaign module manifest differs from the current pinned gate')
        values, incomplete, unexecuted = {}, [], []
        seen = set()
        for row in campaign['runs']:
            if row['module'] not in expected or row['module'] in seen:
                raise ValueError(f'unexpected or duplicate campaign module: {row["module"]}')
            seen.add(row['module'])
            if row['status'] != 'recorded':
                incomplete.append(row['module']); continue
            rows = summarize(Path(row['xml']))['modules']
            if not any(m['name'] == row['module'] for m in rows):
                incomplete.append(row['module'])
            for module in rows:
                if (not module['done'] or not module['tests']
                        or int(module['declared_total'] or -1) != len(module['tests'])):
                    incomplete.append(module['name'])
                for test in module['tests']:
                    key = (module['name'], module['abi'], test['class'], test['name'])
                    if key in values: raise ValueError(f'duplicate campaign result: {key}')
                    values[key] = test['result']
                    if test['result'] not in ('pass', 'fail'): unexecuted.append(key)
        missing = expected - {r['module'] for r in campaign['runs']}
        return campaign, values, incomplete + sorted(missing), unexecuted
    a, old, ai, au = load(original); b, new, bi, bu = load(native)
    if a['label'] != 'original' or b['label'] != 'native' or a['manifest_sha256'] != b['manifest_sha256'] or a['cts_args'] != b['cts_args']:
        raise ValueError('baseline/native labels, module manifest or harness arguments differ')
    regressions = [key for key in old.keys() & new.keys() if old[key] == 'pass' and new[key] != 'pass']
    changes = [key for key in old.keys() & new.keys() if old[key] != new[key]]
    report = {'regressions': regressions, 'changed_outcomes': changes,
              'missing_native_tests': sorted(old.keys() - new.keys()), 'native_only_tests': sorted(new.keys() - old.keys()),
              'original_incomplete_modules': ai, 'native_incomplete_modules': bi,
              'original_not_run_tests': au, 'native_not_run_tests': bu,
              'counts': {'original': dict(collections.Counter(old.values())), 'native': dict(collections.Counter(new.values()))}}
    report['complete_same_outcomes'] = not any([regressions, changes, report['missing_native_tests'], report['native_only_tests'], ai, bi, au, bu])
    report['native_all_pass'] = bool(new) and not bi and not bu and all(v == 'pass' for v in new.values())
    report['acceptance_pass'] = report['complete_same_outcomes'] and report['native_all_pass']
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__); sub = parser.add_subparsers(dest='action', required=True)
    sub.add_parser('prepare')
    summary = sub.add_parser('summary'); summary.add_argument('xml', type=Path)
    runner = sub.add_parser('run'); runner.add_argument('data', type=Path); runner.add_argument('port', type=int)
    runner.add_argument('--label', choices=['original', 'native'], required=True); runner.add_argument('--image', type=Path, required=True)
    runner.add_argument('--output', type=Path, required=True); runner.add_argument('--timeout', type=int, default=10800)
    runner.add_argument('--cts-args', nargs=argparse.REMAINDER, default=[])
    diff = sub.add_parser('compare'); diff.add_argument('original', type=Path); diff.add_argument('native', type=Path)
    args = parser.parse_args()
    if args.action == 'prepare': print(json.dumps({'prepared_device_paths': prepare()})); return 0
    if args.action == 'summary': print(json.dumps(summarize(args.xml), indent=2)); return 0
    if args.action == 'run':
        # One campaign per device; hold the lock over all sequential modules.
        lock_path = Path('/tmp') / f'aim-cts-campaign-{args.port}.lock'
        with lock_path.open('a+') as lock:
            try: fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError: raise ValueError('another CTS campaign owns this port')
            return run(args)
    report = compare(args.original, args.native); print(json.dumps(report, indent=2)); return 0 if report['acceptance_pass'] else 1


if __name__ == '__main__':
    try: sys.exit(main())
    except (ValueError, ET.ParseError, OSError) as error: sys.exit(str(error))
