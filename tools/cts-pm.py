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
import importlib.util
_host_spec = importlib.util.spec_from_file_location('cts_host_tools', Path(__file__).parent / 'lib/cts_host_tools.py')
cts_host_tools = importlib.util.module_from_spec(_host_spec)
_host_spec.loader.exec_module(cts_host_tools)

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


def xml_complete(rows):
    def complete(module):
        try:
            total = int(module['declared_total'] or -1)
        except (TypeError, ValueError):
            return False
        return module['done'] and bool(module['tests']) and total == len(module['tests'])
    return bool(rows) and all(complete(module) for module in rows)


def summarize(path, data=None):
    tree = ET.fromstring(data) if data is not None else ET.parse(path).getroot()
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
            'command': tree.attrib.get('command_line_args'), 'modules': rows,
            'counts': dict(collections.Counter(test['result'] for module in rows for test in module['tests'])),
            'complete': xml_complete(rows)}


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


def campaign_provenance(args):
    receipt = args.image / '.overlay-receipt'
    if not receipt.is_file():
        raise ValueError('campaign requires the actual derived image overlay receipt')
    provenance = {'label': args.label, 'release': '16_r1', 'image': str(args.image.resolve()),
            'data': str(args.data.resolve()), 'port': args.port, 'cts_args': args.cts_args,
            'manifest_sha256': digest(MANIFEST), 'modules': [m['name'] for m in modules()],
            'image_receipt_sha256': digest(receipt)}
    if args.linux_run is not None:
        executable = args.linux_run.resolve(strict=True)
        if not os.access(executable, os.X_OK):
            raise ValueError('--linux-run requires an executable path')
        provenance.update(linux_run=str(executable), linux_run_sha256=digest(executable))
    if getattr(args, 'cts_host_toolsdir', None) is not None:
        if any('module-dir-path' in value for value in args.cts_args):
            raise ValueError('conflicting module-dir-path selection')
        provenance['cts_host_tools'] = cts_host_tools.stage(args.cts_host_toolsdir, ROOT, HARNESS)
    return provenance


def validate_result(result, module, args):
    expected = ['cts', '-s', f'127.0.0.1:{args.port}', '--skip-device-info',
                '--skip-preconditions', '-m', module, *args.cts_args,
                *cts_host_tools.arguments(getattr(args, 'host_tool_selection', None), module)]
    if shlex.split(result['command'] or '') != expected:
        raise ValueError(f'{module}: official XML command differs from the full campaign invocation')
    rows = result['modules']
    if not rows or not any(m['name'] == module for m in rows):
        raise ValueError(f'{module}: official XML lacks the requested module')
    for row in rows:
        name = row['name']
        if name != module and not name.startswith(module + '[') and not name.startswith(module + ' ['):
            raise ValueError(f'{module}: official XML includes foreign module {name}')
    return all(m['done'] and m['tests']
               and int(m['declared_total'] or -1) == len(m['tests'])
               and all(t['result'] in ('pass', 'fail') for t in m['tests']) for m in rows)


def resume_state(args, provenance):
    state = json.loads((args.output / 'campaign.json').read_text())
    for key in ('linux_run', 'linux_run_sha256', 'cts_host_tools'):
        if state.get(key) != provenance.get(key):
            raise ValueError(f'resume provenance differs: {key}')
    for key, value in provenance.items():
        old = state.get(key)
        if key in ('image', 'data') and old is not None:
            old = str(Path(old).resolve())
        if old != value:
            raise ValueError(f'resume provenance differs: {key}')
    seen = set()
    for row in state['runs']:
        module = row['module']
        if module not in provenance['modules'] or module in seen:
            raise ValueError(f'unexpected or duplicate campaign module: {module}')
        seen.add(module)
        if row['status'] != 'recorded':
            continue
        xml = Path(row['xml']).resolve()
        if xml.parent != args.output.resolve():
            raise ValueError(f'{module}: result XML is outside this campaign')
        result = summarize(xml)
        sha = digest(xml)
        if row.get('xml_sha256') is not None:
            if row['xml_sha256'] != sha:
                raise ValueError(f'{module}: recorded result XML hash differs')
        else:
            # The running pre-resume runner retained its exact parsed summary.
            # Upgrade only that evidence, never import an arbitrary result XML.
            stored = row.get('summary', {})
            if any(stored.get(k) != result[k] for k in ('release', 'command', 'modules')):
                raise ValueError(f'{module}: legacy XML differs from its recorded summary')
            row['xml_sha256'] = sha
        if not validate_result(result, module, args):
            row['status'] = 'not-run-complete'
    return state


def run(args):
    if args.stop_after is not None and args.stop_after < 1:
        raise ValueError('--stop-after must be positive')
    if args.output.exists() and not args.resume:
        raise ValueError('campaign output already exists; use --resume to validate its provenance')
    if args.resume and not (args.output / 'campaign.json').is_file():
        raise ValueError('--resume requires an existing campaign')
    services = (args.image / 'system/etc/aim/native-services').read_text().splitlines()
    native = {line.split()[0] for line in services if line.strip() and not line.lstrip().startswith('#')}
    if ('package' in native) != (args.label == 'native'):
        raise ValueError('image native-services does not match campaign label')
    if any(v.split('=', 1)[0] in ('-m', '--module', '-t', '--test', '--include-filter',
                                  '--exclude-filter', '--retry', '--skip-all-system-status-check',
                                  '--skip-system-status-check') for v in args.cts_args):
        raise ValueError('module/test filtering or retry changes the complete batch scope')
    provenance = campaign_provenance(args)
    args.host_tool_selection = provenance.get('cts_host_tools')
    if args.resume:
        state = resume_state(args, provenance)
    else:
        state = dict(provenance, runs=[], started=datetime.datetime.now(datetime.timezone.utc).isoformat())
    prepare()
    args.output.mkdir(parents=True, exist_ok=args.resume)
    save(args.output / 'campaign.json', state)
    completed = 0
    for index, module in enumerate(state['modules']):
        prior = next((r for r in state['runs'] if r['module'] == module), None)
        if prior and prior['status'] == 'recorded':
            continue
        # This explicit request takes effect between module invocations only.
        if (args.output / 'request-stop').exists():
            print('campaign stopped between modules: request-stop', flush=True)
            return 0
        attempt = 1
        stem = f'{index + 1:02d}-{module}'
        while (args.output / f'{stem}-attempt{attempt}.log').exists():
            attempt += 1
        stem += f'-attempt{attempt}'
        before = {str(p) for p in (HARNESS / 'results').glob('*') if p.is_dir()}
        log = args.output / (stem + '.log')
        shell_args = ['--linux-run', provenance['linux_run']] if args.linux_run is not None else []
        if args.host_tool_selection and module == cts_host_tools.MODULE:
            shell_args += ['--cts-host-toolsdir', args.host_tool_selection['directory']]
        command = [str(ROOT / 'tools/cts-tradefed.sh'), *shell_args, str(args.data), str(args.port), '-m', module, *args.cts_args]
        row = {'module': module, 'log': str(log), 'status': 'not-run-complete'}
        history = list(prior.get('attempts', [])) if prior else []
        if prior:
            history.append({k: v for k, v in prior.items() if k != 'attempts'})
        def record():
            row['attempts'] = history
            state['runs'] = [r for r in state['runs'] if r['module'] != module] + [row]
            save(args.output / 'campaign.json', state)
        with log.open('x') as output:
            child = subprocess.Popen(command, stdout=output, stderr=subprocess.STDOUT)
            try:
                code = child.wait(timeout=args.timeout)
            except (subprocess.TimeoutExpired, KeyboardInterrupt):
                # Only our wrapper PID; its trap cleans its own harness group.
                child.send_signal(signal.SIGTERM)
                child.wait()
                record()
                raise
        row['wrapper_exit'] = code
        try:
            xml = result_for(module, before)
            result = summarize(xml)
            copied = args.output / (stem + '.xml')
            with copied.open('xb') as output:
                output.write(xml.read_bytes())
            row.update(xml=str(copied), xml_sha256=digest(copied), summary=result)
            if validate_result(result, module, args):
                row['status'] = 'recorded'
        except (ValueError, ET.ParseError) as error:
            row['error'] = str(error)
        record()
        completed += 1
        print(module, row['status'], flush=True)
        if args.stop_after is not None and completed >= args.stop_after:
            print('campaign stopped between modules: --stop-after', flush=True)
            return 0
    return 0


def compare(original, native):
    expected = {m['name'] for m in modules()}
    def load(path):
        campaign = json.loads((path / 'campaign.json').read_text())
        if set(campaign['modules']) != expected:
            raise ValueError('campaign module scope differs from the current pinned gate')
        if campaign['manifest_sha256'] != digest(MANIFEST):
            raise ValueError('campaign module manifest differs from the current pinned gate')
        values, incomplete, unexecuted, evidence, wrapper_issues = {}, set(), [], [], []
        seen = set()
        for row in campaign['runs']:
            requested = row['module']
            if requested not in expected or requested in seen:
                raise ValueError(f'unexpected or duplicate campaign module: {requested}')
            seen.add(requested)
            issues = []
            if row['status'] != 'recorded': issues.append(f"campaign status: {row['status']}")
            if row.get('wrapper_exit') != 0: issues.append(f"wrapper exit: {row.get('wrapper_exit')}")
            if row.get('error'): issues.append(row['error'])
            record = {'module': requested, 'campaign_status': row['status'],
                      'wrapper_exit': row.get('wrapper_exit'), 'wrapper_error': row.get('error'),
                      'xml_retained': bool(row.get('xml')), 'xml_complete': False,
                      'invocation_matches': False, 'counts': {}, 'acceptance_complete': False}
            evidence.append(record)
            if not row.get('xml'):
                issues.append('no retained official XML')
                incomplete.add(requested)
                wrapper_issues.append({'module': requested, 'issues': issues})
                continue
            xml = Path(row['xml']).resolve(strict=True)
            if xml.parent != path.resolve():
                raise ValueError(f'{requested}: result XML is outside this campaign')
            data = xml.read_bytes()
            sha = hashlib.sha256(data).hexdigest()
            if row.get('xml_sha256') != sha:
                raise ValueError(f'{requested}: retained result XML hash differs or is missing')
            result = summarize(xml, data)
            rows = result['modules']
            for module in rows:
                name = module['name']
                if name != requested and not name.startswith(requested + '[') and not name.startswith(requested + ' ['):
                    raise ValueError(f'{requested}: official XML includes foreign module {name}')
            command = ['cts', '-s', f"127.0.0.1:{campaign['port']}", '--skip-device-info',
                       '--skip-preconditions', '-m', requested, *campaign['cts_args']]
            matches = shlex.split(result['command'] or '') == command
            if not matches: issues.append('official XML invocation differs from campaign')
            has_requested = any(module['name'] == requested for module in rows)
            if not has_requested: issues.append('official XML lacks the requested base module')
            if not result['complete']: issues.append('official XML module incomplete')
            record.update(xml=str(xml), xml_sha256=sha, xml_complete=result['complete'],
                          invocation_matches=matches, counts=result['counts'])
            for module in rows:
                for test in module['tests']:
                    key = (module['name'], module['abi'], test['class'], test['name'])
                    if key in values: raise ValueError(f'duplicate campaign result: {key}')
                    values[key] = test['result']
                    if test['result'] not in ('pass', 'fail'): unexecuted.append(key)
            record['acceptance_complete'] = not issues and has_requested and result['complete'] and all(
                test['result'] in ('pass', 'fail') for module in rows for test in module['tests'])
            if not record['acceptance_complete']: incomplete.add(requested)
            if issues: wrapper_issues.append({'module': requested, 'issues': issues})
        incomplete.update(expected - seen)
        return campaign, values, sorted(incomplete), unexecuted, evidence, wrapper_issues
    a, old, ai, au, ae, aw = load(original); b, new, bi, bu, be, bw = load(native)
    if a['label'] != 'original' or b['label'] != 'native' or a['manifest_sha256'] != b['manifest_sha256'] or a['cts_args'] != b['cts_args']:
        raise ValueError('baseline/native labels, module manifest or harness arguments differ')
    regressions = [key for key in old.keys() & new.keys() if old[key] == 'pass' and new[key] != 'pass']
    changes = [key for key in old.keys() & new.keys() if old[key] != new[key]]
    report = {'regressions': regressions, 'changed_outcomes': changes,
              'missing_native_tests': sorted(old.keys() - new.keys()), 'native_only_tests': sorted(new.keys() - old.keys()),
              'original_incomplete_modules': ai, 'native_incomplete_modules': bi,
              'original_not_run_tests': au, 'native_not_run_tests': bu,
              'original_module_evidence': ae, 'native_module_evidence': be,
              'original_wrapper_issues': aw, 'native_wrapper_issues': bw,
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
    runner.add_argument('--linux-run', type=Path, help='explicit immutable runtime executable for guest shell authorization')
    runner.add_argument('--cts-host-toolsdir', type=Path, help='verified Darwin dependency for the staged-install host module')
    runner.add_argument('--resume', action='store_true')
    runner.add_argument('--stop-after', type=int, help='stop after N completed invocations in this run')
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
