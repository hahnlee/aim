#!/usr/bin/env python3
"""Run and verify the deterministic tail of the pinned M4 PackageManager CTS gate.

The fresh full-manifest prefix campaign owns indices 1..3.  A campaign created
by ``run-tail`` owns indices 4..66.  ``aggregate`` credits evidence only inside
those ranges and requires their exact union to be the current pinned manifest.
"""
import argparse
import contextlib
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
import xml.etree.ElementTree as ET


ROOT = Path(__file__).resolve().parent.parent
RUNNER = ROOT / 'tools/cts-pm.py'
MANIFEST = ROOT / 'tools/cts-pm-modules.json'
PREFIX_START, PREFIX_END = 1, 3
TAIL_START = PREFIX_END + 1
EXPECTED_MODULES = 66

_spec = importlib.util.spec_from_file_location('cts_pm_shard_base', RUNNER)
cts_pm = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(cts_pm)


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def full_manifest():
    value = json.loads(MANIFEST.read_text())
    modules = value.get('modules')
    if not isinstance(modules, list) or not modules:
        raise ValueError('empty or invalid PackageManager CTS manifest')
    names = [row.get('name') for row in modules]
    if any(not isinstance(name, str) or not name for name in names):
        raise ValueError('PackageManager CTS manifest contains an invalid module')
    if len(names) != len(set(names)):
        raise ValueError('PackageManager CTS manifest contains duplicate modules')
    if len(names) != EXPECTED_MODULES:
        raise ValueError(f'PackageManager CTS manifest must contain exactly {EXPECTED_MODULES} modules')
    return modules


def module_names(start, end):
    rows = full_manifest()
    if not 1 <= start <= end <= len(rows):
        raise ValueError('shard range is outside the full manifest')
    return [row['name'] for row in rows[start - 1:end]]


def plan():
    rows = full_manifest()
    count = len(rows)
    return {
        'schema': 'AIM_CTS_PM_SHARD_PLAN1',
        'manifest': str(MANIFEST.resolve()),
        'manifest_sha256': digest(MANIFEST),
        'module_count': count,
        'assignments': [
            {'owner': 'fresh-prefix', 'start_index': PREFIX_START,
             'end_index': PREFIX_END,
             'modules': module_names(PREFIX_START, PREFIX_END)},
            {'owner': 'tail', 'start_index': TAIL_START, 'end_index': count,
             'modules': module_names(TAIL_START, count)},
        ],
    }


def tail_provenance(base):
    shard_plan = plan()
    assignment = shard_plan['assignments'][1]
    return {
        **base,
        'shard_runner_sha256': digest(__file__),
        'full_manifest_sha256': shard_plan['manifest_sha256'],
        'full_modules': [row['name'] for row in full_manifest()],
        'shard': {
            'schema': 'AIM_CTS_PM_SHARD1',
            'owner': assignment['owner'],
            'start_index': assignment['start_index'],
            'end_index': assignment['end_index'],
            'modules': assignment['modules'],
            'aggregate_plan': shard_plan,
        },
    }


@contextlib.contextmanager
def selected_tail():
    """Temporarily restrict the unchanged base runner to the owned tail."""
    rows = full_manifest()
    selected = rows[TAIL_START - 1:]
    original_modules = cts_pm.modules
    original_provenance = cts_pm.campaign_provenance

    def provenance(args):
        return tail_provenance(original_provenance(args))

    cts_pm.modules = lambda: selected
    cts_pm.campaign_provenance = provenance
    try:
        yield
    finally:
        cts_pm.modules = original_modules
        cts_pm.campaign_provenance = original_provenance


def run_tail(args):
    if args.label != 'native':
        raise ValueError('the M4 acceptance tail must use the native image')
    lock_path = Path('/tmp') / f'aim-cts-campaign-{args.port}.lock'
    with lock_path.open('a+') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise ValueError('another CTS campaign owns this port') from error
        with selected_tail():
            return cts_pm.run(args)


def _regular_campaign(directory):
    directory = Path(directory).resolve(strict=True)
    path = directory / 'campaign.json'
    if path.is_symlink() or not path.is_file():
        raise ValueError(f'campaign is missing or linked: {path}')
    data = path.read_bytes()
    return directory, path, data, json.loads(data)


def _validate_static_provenance(campaign):
    if campaign.get('manifest_sha256') != digest(MANIFEST):
        raise ValueError('campaign full manifest hash differs')
    if campaign.get('campaign_script_sha256') != digest(RUNNER):
        raise ValueError('campaign base runner hash differs')
    receipt = Path(campaign['image']) / '.overlay-receipt'
    if not receipt.is_file() or campaign.get('image_receipt_sha256') != digest(receipt):
        raise ValueError('campaign image receipt differs')
    services = (Path(campaign['image']) / 'system/etc/aim/native-services').read_text().splitlines()
    native = {line.split()[0] for line in services
              if line.strip() and not line.lstrip().startswith('#')}
    if 'package' not in native:
        raise ValueError('campaign image does not register the native package service')
    cts_pm.revalidate_execution(campaign)


def _validate_owned_campaign(directory, campaign, owned, *, tail):
    expected_full = [row['name'] for row in full_manifest()]
    if campaign.get('label') != 'native':
        raise ValueError('aggregate accepts only native campaigns')
    if tail:
        expected_shard = tail_provenance({})
        for key in ('shard_runner_sha256', 'full_manifest_sha256', 'full_modules', 'shard'):
            if campaign.get(key) != expected_shard[key]:
                raise ValueError('tail shard ownership or provenance differs: ' + key)
        if campaign.get('modules') != owned:
            raise ValueError('tail campaign module sequence differs from its ownership')
    elif campaign.get('modules') != expected_full:
        raise ValueError('prefix campaign is not the full pinned manifest')

    if campaign.get('cts_args') != []:
        raise ValueError('acceptance campaign CTS arguments must be exactly empty')

    _validate_static_provenance(campaign)
    rows = campaign.get('runs')
    if not isinstance(rows, list):
        raise ValueError('campaign runs are missing')
    by_module = {}
    for row in rows:
        module = row.get('module')
        if module in by_module:
            raise ValueError('duplicate campaign result: ' + str(module))
        if module not in owned:
            raise ValueError('campaign contains evidence outside its assigned ownership: ' + str(module))
        by_module[module] = row
    missing = [name for name in owned if name not in by_module]
    if missing:
        raise ValueError('campaign is missing owned modules: ' + ', '.join(missing))

    evidence = []
    for module in owned:
        row = by_module[module]
        if row.get('status') != 'recorded' or row.get('wrapper_exit') != 0 or row.get('error'):
            raise ValueError(module + ': campaign invocation is incomplete')
        cts_pm.persisted_owner_receipt(campaign, row, directory)
        xml_value = row.get('xml')
        if not xml_value:
            raise ValueError(module + ': retained official XML is missing')
        xml = Path(xml_value)
        if xml.is_symlink():
            raise ValueError(module + ': retained official XML is linked')
        xml = xml.resolve(strict=True)
        if xml.parent != directory:
            raise ValueError(module + ': retained official XML is outside its campaign')
        data = xml.read_bytes()
        sha = hashlib.sha256(data).hexdigest()
        if row.get('xml_sha256') != sha:
            raise ValueError(module + ': retained official XML hash differs')
        result = cts_pm.summarize(xml, data)
        args = argparse.Namespace(port=campaign['port'], cts_args=campaign['cts_args'],
                                  host_tool_selection=campaign.get('cts_host_tools'))
        if not cts_pm.validate_result(result, module, args):
            raise ValueError(module + ': official XML is incomplete or unbound')
        outcomes = [test['result'] for item in result['modules'] for test in item['tests']]
        if not outcomes or any(outcome != 'pass' for outcome in outcomes):
            raise ValueError(module + ': official CTS tests are not all pass')
        owner_receipt = Path(row['log']).with_suffix('.owner.json').resolve(strict=True)
        identities = [(item['name'], item['abi'], test['class'], test['name'])
                      for item in result['modules'] for test in item['tests']]
        evidence.append({'module': module, 'xml': str(xml), 'xml_sha256': sha,
                         'owner_receipt_sha256': row['owner_receipt_sha256'],
                         'tests': len(outcomes), 'pass': len(outcomes),
                         '_owner_receipt': str(owner_receipt),
                         '_test_identities': identities})
    return evidence


def _reject_aggregate_duplicates(evidence):
    xml_paths, owner_paths, identities = set(), set(), set()
    for row in evidence:
        xml = str(Path(row['xml']).resolve(strict=True))
        owner = str(Path(row['_owner_receipt']).resolve(strict=True))
        if xml in xml_paths:
            raise ValueError('aggregate reuses retained official XML: ' + xml)
        if owner in owner_paths:
            raise ValueError('aggregate reuses owner receipt: ' + owner)
        xml_paths.add(xml)
        owner_paths.add(owner)
        for identity in row['_test_identities']:
            identity = tuple(identity)
            if identity in identities:
                raise ValueError('aggregate contains duplicate test identity: ' + repr(identity))
            identities.add(identity)


def _same_generation(prefix, tail):
    for key in ('release', 'image', 'image_receipt_sha256', 'cts_args',
                'campaign_script_sha256', 'linux_run', 'linux_run_sha256',
                'runtime_files_sha256', 'cts_host_tools', 'execution_tools'):
        if prefix.get(key) != tail.get(key):
            raise ValueError('campaign generations differ: ' + key)
    if prefix.get('wrapper_bundle', {}).get('identity') != tail.get('wrapper_bundle', {}).get('identity'):
        raise ValueError('campaign wrapper generations differ')


def aggregate(prefix_directory, tail_directory):
    prefix_dir, prefix_path, prefix_data, prefix = _regular_campaign(prefix_directory)
    tail_dir, tail_path, tail_data, tail = _regular_campaign(tail_directory)
    if prefix_dir == tail_dir:
        raise ValueError('prefix and tail outputs must be distinct')
    prefix_data_dir = Path(prefix['data']).resolve()
    tail_data_dir = Path(tail['data']).resolve()
    if prefix_data_dir == tail_data_dir:
        raise ValueError('prefix and tail DATA must be distinct')
    prefix_ports = {prefix['port'], prefix['port'] + 1}
    tail_ports = {tail['port'], tail['port'] + 1}
    if prefix_ports & tail_ports:
        raise ValueError('prefix and tail device/ADB port pairs overlap')
    prefix_bundle = Path(prefix['wrapper_bundle']['directory']).resolve()
    tail_bundle = Path(tail['wrapper_bundle']['directory']).resolve()
    if prefix_bundle == tail_bundle:
        raise ValueError('prefix and tail wrapper bundles must be distinct')
    _same_generation(prefix, tail)

    shard_plan = plan()
    prefix_modules = shard_plan['assignments'][0]['modules']
    tail_modules = shard_plan['assignments'][1]['modules']
    if (prefix_modules + tail_modules != [row['name'] for row in full_manifest()]
            or set(prefix_modules) & set(tail_modules)):
        raise ValueError('shard plan is not an exact disjoint manifest partition')
    # The long tail is the first phase.  Credit the final fresh prefix only
    # after every tail result has survived provenance and outcome validation.
    tail_evidence = _validate_owned_campaign(tail_dir, tail, tail_modules, tail=True)
    prefix_evidence = _validate_owned_campaign(prefix_dir, prefix, prefix_modules, tail=False)
    # Detect changes made while official evidence was being inspected.
    if hashlib.sha256(tail_path.read_bytes()).digest() != hashlib.sha256(tail_data).digest():
        raise ValueError('tail campaign changed during aggregation')
    if hashlib.sha256(prefix_path.read_bytes()).digest() != hashlib.sha256(prefix_data).digest():
        raise ValueError('prefix campaign changed during aggregation')
    evidence = prefix_evidence + tail_evidence
    _reject_aggregate_duplicates(evidence)
    retained_evidence = [{key: value for key, value in row.items() if not key.startswith('_')}
                         for row in evidence]
    return {
        'schema': 'AIM_CTS_PM_SHARD_AGGREGATE1',
        'aggregate_verifier_sha256': digest(__file__),
        'plan': shard_plan,
        'sources': [
            {'owner': 'fresh-prefix', 'directory': str(prefix_dir),
             'campaign_sha256': hashlib.sha256(prefix_data).hexdigest(),
             'data': str(prefix_data_dir), 'port': prefix['port'],
             'wrapper_bundle': str(prefix_bundle)},
            {'owner': 'tail', 'directory': str(tail_dir),
             'campaign_sha256': hashlib.sha256(tail_data).hexdigest(),
             'data': str(tail_data_dir), 'port': tail['port'],
             'wrapper_bundle': str(tail_bundle)},
        ],
        'module_evidence': retained_evidence,
        'counts': {'modules': len(retained_evidence),
                   'tests': sum(row['tests'] for row in retained_evidence),
                   'pass': sum(row['pass'] for row in retained_evidence)},
        'native_all_pass': len(retained_evidence) == len(full_manifest()),
        'acceptance_pass': len(retained_evidence) == len(full_manifest()),
    }


def save_new(path, value):
    path = Path(path)
    if path.exists():
        raise ValueError('refusing to replace output: ' + str(path))
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = path.with_name(path.name + f'.partial-{os.getpid()}')
    try:
        with temp.open('x') as stream:
            json.dump(value, stream, indent=2)
            stream.write('\n')
        temp.replace(path)
    finally:
        temp.unlink(missing_ok=True)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='action', required=True)
    show = sub.add_parser('plan')
    show.add_argument('--output', type=Path)
    runner = sub.add_parser('run-tail')
    runner.add_argument('data', type=Path)
    runner.add_argument('port', type=int)
    runner.add_argument('--label', choices=['native'], required=True)
    runner.add_argument('--image', type=Path, required=True)
    runner.add_argument('--output', type=Path, required=True)
    runner.add_argument('--timeout', type=int, default=10800)
    runner.add_argument('--linux-run', type=Path, required=True)
    runner.add_argument('--cts-host-toolsdir', type=Path, required=True)
    runner.add_argument('--cts-java', type=Path, required=True)
    runner.add_argument('--cts-adb', type=Path, required=True)
    runner.add_argument('--cts-aapt2', type=Path, required=True)
    runner.add_argument('--cts-owner', type=Path, required=True)
    runner.add_argument('--resume', action='store_true')
    runner.add_argument('--stop-after', type=int)
    runner.add_argument('--cts-args', nargs=argparse.REMAINDER, default=[])
    combine = sub.add_parser('aggregate')
    combine.add_argument('--prefix', required=True, type=Path)
    combine.add_argument('--tail', required=True, type=Path)
    combine.add_argument('--output', required=True, type=Path)
    args = parser.parse_args(argv)
    if args.action == 'plan':
        value = plan()
        if args.output:
            save_new(args.output, value)
        else:
            print(json.dumps(value, indent=2))
        return 0
    if args.action == 'run-tail':
        return run_tail(args)
    value = aggregate(args.prefix, args.tail)
    save_new(args.output, value)
    print(json.dumps(value['counts']))
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (ValueError, KeyError, TypeError, OSError, ET.ParseError) as error:
        sys.exit(str(error))
