"""Official CTS XML evidence remains visible without relaxing acceptance."""
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('cts_pm', ROOT / 'tools/cts-pm.py')
pm = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pm)


class ComparisonEvidence(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name).resolve()
        self.manifest = self.root / 'modules.json'
        self.manifest.write_text(json.dumps({'modules': [{'name': 'CtsFixture', 'kind': 'host'}]}))
        self.pin = patch.object(pm, 'MANIFEST', self.manifest)
        self.pin.start()
        self.addCleanup(self.pin.stop)
        self.addCleanup(self.temp.cleanup)
        self.shared = self.root / 'shared-inputs'
        self.shared.mkdir()
        self.runtime = {}
        for name in pm.RUNTIME_FILES:
            executable = self.shared / name
            executable.write_text('runtime fixture: ' + name)
            self.runtime[name] = pm.digest(executable)
        self.tools = {}
        for name in ('java', 'adb', 'aapt2', 'owner'):
            executable = self.shared / name
            executable.write_text('selected tool fixture: ' + name)
            self.tools[name] = {'path': str(executable), 'sha256': pm.digest(executable)}
            if name != 'owner': self.tools[name]['version'] = 'fixture version metadata'

    def provenance(self):
        return {'modules': [m['name'] for m in pm.modules()],
                'manifest_sha256': pm.digest(self.manifest), 'port': 5555, 'cts_args': [],
                'data': str(self.root / 'data'), 'image': str(self.root / 'image'),
                'release': '16_r1', 'campaign_script_sha256': pm.digest(ROOT / 'tools/cts-pm.py'),
                'linux_run': str(self.shared / 'linux-run'), 'linux_run_sha256': self.runtime['linux-run'],
                'runtime_files_sha256': dict(self.runtime), 'execution_tools': copy.deepcopy(self.tools),
                'cts_host_tools': {'directory': str(self.shared), 'provenance_sha256': self.runtime['aim-binderd'],
                                   'validation_sha256': self.runtime['aim-display'], 'components': {'fixture': self.runtime['guest-init']}},
                'wrapper_bundle': {'directory': str(self.shared), 'identity': pm.digest(ROOT / 'tools/cts-tradefed.sh'),
                                   'manifest': {'root': str(ROOT)}}}

    def update_campaign(self, directory, change):
        path = directory / 'campaign.json'
        value = json.loads(path.read_text())
        change(value)
        path.write_text(json.dumps(value))

    def owner_receipt(self, command, pid):
        # Protocol shape from the real foreground-adb/JVM integration, not live processes.
        adb = {'pid': 65867, 'start_seconds': 1791717547, 'start_microseconds': 262545, 'terminal': True}
        jvm = {'pid': 65868, 'start_seconds': 1791717547, 'start_microseconds': 267752, 'terminal': True}
        return {'schema': 'AIM_CTS_OWNER1', 'supervisor_pid': pid,
                'helper_sha256': self.tools['owner']['sha256'], 'wrapper_argv': command[command.index('--') + 1:],
                'jvm_identity': {k: jvm[k] for k in ('pid', 'start_seconds', 'start_microseconds')},
                'cleanup': 'complete', 'processes': [adb, jvm], 'remaining_owned': [], 'uncertainties': []}

    def campaign(self, label, statuses, *, status='recorded', done=True,
                 exit_code=0, mismatch=False, variants=False,
                 module_name='CtsFixture', selection=None, xml_command=None):
        directory = self.root / label
        directory.mkdir()
        command = ['cts', '-s', '127.0.0.1:5555', '--skip-device-info',
                   '--skip-preconditions', '-m', module_name,
                   *pm.cts_host_tools.arguments(selection, module_name)]
        if xml_command is not None: command = xml_command
        if mismatch: command += ['--test', 'filtered']
        tree = ET.Element('Result', suite_name='CTS', suite_version='16_r1',
                          suite_build_number='13467367', command_line_args=' '.join(command))
        for name, results in [(module_name, statuses)] + ([(module_name + '[instant]', ['pass'])] if variants else []):
            module = ET.SubElement(tree, 'Module', name=name, abi='arm64-v8a',
                                   done=str(done).lower(), total_tests=str(len(results)))
            case = ET.SubElement(module, 'TestCase', name='fixture.Class')
            for index, outcome in enumerate(results):
                ET.SubElement(case, 'Test', name=f'test{index}', result=outcome)
        xml = directory / 'official.xml'
        ET.ElementTree(tree).write(xml, encoding='utf-8', xml_declaration=True)
        row = {'module': module_name, 'status': status, 'wrapper_exit': exit_code,
               'xml': str(xml), 'xml_sha256': pm.digest(xml), 'summary': pm.summarize(xml)}
        campaign = {**self.provenance(), 'label': label, 'modules': [m['name'] for m in pm.modules()], 'port': 5555, 'cts_args': [],
                    'manifest_sha256': pm.digest(self.manifest), 'runs': [row]}
        if selection is not None: campaign['cts_host_tools'].update(selection)
        log = directory / '01-attempt.log'
        log.write_text('fixture invocation output')
        receipt_path = log.with_suffix('.owner.json')
        tools = campaign['execution_tools']
        base = Path(campaign['wrapper_bundle']['directory'])
        selected = [value for name in ('java', 'adb', 'aapt2')
                    for value in ['--cts-' + name, tools[name]['path']]]
        shell = ['--linux-run', campaign['linux_run']]
        if module_name == pm.cts_host_tools.MODULE:
            shell += ['--cts-host-toolsdir', campaign['cts_host_tools']['directory']]
        wrapper = [str(base / 'tools/cts-tradefed.sh'), *selected, '--repository-root', str(ROOT), '--image', campaign['image'],
                   '--guest-shell', str(base / 'tools/guest-shell.sh'),
                   '--host-tool-helper', str(base / 'tools/lib/cts_host_tools.py'),
                   '--harness', str(base / 'harness/android-cts'), *shell,
                   campaign['data'], '5555', '-m', module_name]
        owner_command = [tools['owner']['path'], '--receipt', str(receipt_path),
                         '--adb', tools['adb']['path'], '--server-port', '5556',
                         '--java', tools['java']['path'], '--', *wrapper]
        receipt_path.write_text(json.dumps(self.owner_receipt(owner_command, 65866)))
        row.update(log=str(log), wrapper_command=owner_command,
                   wrapper_bundle_identity=campaign['wrapper_bundle']['identity'],
                   owner_receipt_sha256=pm.digest(receipt_path),
                   invocation_scope={'wrapper_pid': 65866, 'serial': '127.0.0.1:5555',
                                     'started_ms': 1791717547000, 'finished_ms': 1791717552000,
                                     'command': pm.invocation_command(module_name, SimpleNamespace(
                                         port=5555, cts_args=[], host_tool_selection=campaign['cts_host_tools']))})
        (directory / 'campaign.json').write_text(json.dumps(campaign))
        return directory

    def test_selected_host_tool_command_matches_runner_and_comparison(self):
        module = pm.cts_host_tools.MODULE
        self.manifest.write_text(json.dumps({'modules': [
            {'name': module, 'kind': 'host'}, {'name': 'CtsUnrunFixture', 'kind': 'host'}]}))
        selection = {'module_arg': module + ':{config-descriptor}metadata:module-dir-path:=' + str(self.root / 'tools')}
        args = SimpleNamespace(port=5555, cts_args=[], host_tool_selection=selection)
        original = self.campaign('original', ['pass'], module_name=module, selection=selection)
        native = self.campaign('native', ['pass'], module_name=module, selection=selection)
        self.assertTrue(pm.validate_result(pm.summarize(original / 'official.xml'), module, args))
        report = pm.compare(original, native)
        self.assertTrue(report['original_module_evidence'][0]['invocation_matches'])
        self.assertTrue(report['native_module_evidence'][0]['invocation_matches'])
        self.assertFalse(report['acceptance_pass'], 'unrun module remains incomplete')

    def test_missing_host_tool_selection_is_rejected(self):
        self.check_wrong_host_tool_command(None)

    def test_changed_host_tool_selection_is_rejected(self):
        self.check_wrong_host_tool_command(pm.cts_host_tools.MODULE + ':{config-descriptor}metadata:module-dir-path:=wrong')

    def test_foreign_host_tool_selection_is_rejected(self):
        self.check_wrong_host_tool_command('CtsCompilationTestCases:{config-descriptor}metadata:module-dir-path:=wrong')

    def check_wrong_host_tool_command(self, actual_option):
        module = pm.cts_host_tools.MODULE
        self.manifest.write_text(json.dumps({'modules': [{'name': module, 'kind': 'host'}]}))
        selection = {'module_arg': module + ':{config-descriptor}metadata:module-dir-path:=' + str(self.root / 'tools')}
        command = ['cts', '-s', '127.0.0.1:5555', '--skip-device-info', '--skip-preconditions', '-m', module]
        if actual_option is not None: command += ['--module-arg', actual_option]
        original = self.campaign('original', ['pass'], module_name=module, selection=selection, xml_command=command)
        native = self.campaign('native', ['pass'], module_name=module, selection=selection, xml_command=command)
        args = SimpleNamespace(port=5555, cts_args=[], host_tool_selection=selection)
        with self.assertRaises(ValueError):
            pm.validate_result(pm.summarize(original / 'official.xml'), module, args)
        report = pm.compare(original, native)
        self.assertFalse(report['original_module_evidence'][0]['invocation_matches'])
        self.assertFalse(report['acceptance_pass'])

    def test_verified_nonrecorded_xml_keeps_failures_and_assumptions_separate_from_completion(self):
        original = self.campaign('original', ['pass', 'fail', 'ASSUMPTION_FAILURE'],
                                 status='not-run-complete', variants=True)
        native = self.campaign('native', ['pass', 'fail', 'ASSUMPTION_FAILURE'],
                               status='not-run-complete', variants=True)
        report = pm.compare(original, native)
        self.assertEqual(report['counts']['original'], {'pass': 2, 'fail': 1, 'ASSUMPTION_FAILURE': 1})
        self.assertEqual(report['counts']['native'], report['counts']['original'])
        self.assertTrue(report['original_module_evidence'][0]['xml_complete'])
        self.assertFalse(report['original_module_evidence'][0]['acceptance_complete'])
        self.assertEqual(len(report['native_not_run_tests']), 1)
        self.assertEqual(report['original_incomplete_modules'], ['CtsFixture'])
        self.assertFalse(report['acceptance_pass'])
        summary = pm.summarize(original / 'official.xml')
        self.assertEqual(summary['counts'], report['counts']['original'])
        self.assertTrue(summary['complete'])

    def test_wrapper_failure_and_invocation_mismatch_keep_observed_passes_but_block_acceptance(self):
        original = self.campaign('original', ['pass'])
        native = self.campaign('native', ['pass'], exit_code=2, mismatch=True)
        report = pm.compare(original, native)
        self.assertEqual(report['counts']['native'], {'pass': 1})
        self.assertFalse(report['native_all_pass'])
        self.assertFalse(report['native_module_evidence'][0]['invocation_matches'])
        self.assertIn('wrapper exit: 2', report['native_wrapper_issues'][0]['issues'])
        self.assertFalse(report['acceptance_pass'])

    def test_official_partial_module_keeps_observed_tests_and_not_run(self):
        original = self.campaign('original', ['pass'])
        native = self.campaign('native', ['pass', 'not_executed'], done=False)
        report = pm.compare(original, native)
        self.assertEqual(report['counts']['native'], {'pass': 1, 'not_executed': 1})
        self.assertFalse(report['native_module_evidence'][0]['xml_complete'])
        self.assertFalse(report['acceptance_pass'])

    def test_tampered_xml_is_rejected_even_when_campaign_says_recorded(self):
        original = self.campaign('original', ['pass'])
        native = self.campaign('native', ['fail'])
        xml = native / 'official.xml'
        xml.write_bytes(xml.read_bytes().replace(b'result="fail"', b'result="pass"'))
        with self.assertRaisesRegex(ValueError, 'hash differs'):
            pm.compare(original, native)

    def test_verified_foreign_module_is_not_imported_as_requested_evidence(self):
        original = self.campaign('original', ['pass'])
        native = self.campaign('native', ['pass'])
        xml = native / 'official.xml'
        xml.write_bytes(xml.read_bytes().replace(b'name="CtsFixture"', b'name="CtsForeign"'))
        campaign = json.loads((native / 'campaign.json').read_text())
        campaign['runs'][0]['xml_sha256'] = pm.digest(xml)
        (native / 'campaign.json').write_text(json.dumps(campaign))
        with self.assertRaisesRegex(ValueError, 'foreign module'):
            pm.compare(original, native)

    def test_complete_verified_all_pass_campaigns_still_pass(self):
        original = self.campaign('original', ['pass'])
        native = self.campaign('native', ['pass'])
        self.assertTrue(pm.compare(original, native)['acceptance_pass'])


    def test_missing_required_execution_provenance_is_rejected(self):
        original = self.campaign('original', ['pass'])
        native = self.campaign('native', ['pass'])
        path = native / 'campaign.json'
        baseline = path.read_text()
        for key in ('release', 'campaign_script_sha256', 'linux_run_sha256',
                    'runtime_files_sha256', 'cts_host_tools', 'execution_tools', 'wrapper_bundle'):
            with self.subTest(key=key):
                path.write_text(baseline)
                self.update_campaign(native, lambda value: value.pop(key))
                with self.assertRaises(ValueError): pm.compare(original, native)

    def test_changed_runtime_script_wrapper_or_selected_tool_is_rejected(self):
        original = self.campaign('original', ['pass'])
        native = self.campaign('native', ['pass'])
        path = native / 'campaign.json'
        baseline = path.read_text()
        changes = [
            lambda value: value.__setitem__('campaign_script_sha256', 'changed'),
            lambda value: value.__setitem__('linux_run_sha256', 'changed'),
            lambda value: value['wrapper_bundle'].__setitem__('identity', 'changed'),
        ]
        changes += [lambda value, name=name: value['runtime_files_sha256'].__setitem__(name, 'changed')
                    for name in pm.RUNTIME_FILES]
        changes += [lambda value, name=name: value['execution_tools'][name].__setitem__('sha256', 'changed')
                    for name in self.tools]
        for index, change in enumerate(changes):
            with self.subTest(change=index):
                path.write_text(baseline)
                self.update_campaign(native, change)
                with self.assertRaises(ValueError): pm.compare(original, native)

    def test_missing_runtime_component_tool_or_wrapper_identity_is_rejected(self):
        original = self.campaign('original', ['pass'])
        native = self.campaign('native', ['pass'])
        path = native / 'campaign.json'
        baseline = path.read_text()
        changes = [lambda value, name=name: value['runtime_files_sha256'].pop(name)
                   for name in pm.RUNTIME_FILES]
        changes += [lambda value, name=name: value['execution_tools'].pop(name)
                    for name in self.tools]
        changes.append(lambda value: value['wrapper_bundle'].pop('identity'))
        for index, change in enumerate(changes):
            with self.subTest(change=index):
                path.write_text(baseline)
                self.update_campaign(native, change)
                with self.assertRaises(ValueError): pm.compare(original, native)

    def test_owner_receipt_requires_exact_run_and_actual_jvm_birth(self):
        command = ['owner', '--receipt', 'receipt.json', '--', 'wrapper', '-m', 'CtsFixture']
        path = self.root / 'owner.json'
        receipt = self.owner_receipt(command, 65866)
        path.write_text(json.dumps(receipt))
        pm.validate_owner_receipt(path, command, 65866, self.tools['owner']['sha256'])
        changes = [lambda value: value.__setitem__('supervisor_pid', 1),
                   lambda value: value.__setitem__('helper_sha256', 'foreign'),
                   lambda value: value.__setitem__('wrapper_argv', ['foreign']),
                   lambda value: value['jvm_identity'].__setitem__('start_microseconds', 1)]
        for index, change in enumerate(changes):
            with self.subTest(change=index):
                broken = copy.deepcopy(receipt)
                change(broken)
                path.write_text(json.dumps(broken))
                with self.assertRaises(ValueError):
                    pm.validate_owner_receipt(path, command, 65866, self.tools['owner']['sha256'])

    def test_unproven_owner_cleanup_blocks_xml_credit_in_runner(self):
        image = self.root / 'image'
        services = image / 'system/etc/aim/native-services'
        services.parent.mkdir(parents=True)
        services.write_text('')
        bundle = {'directory': str(self.shared), 'identity': self.provenance()['wrapper_bundle']['identity']}
        harness = self.root / 'harness'
        (harness / 'results').mkdir(parents=True)
        mutations = [None,
                     lambda value: value.__setitem__('cleanup', 'incomplete'),
                     lambda value: value.__setitem__('uncertainties', ['metadata EPERM']),
                     lambda value: value.__setitem__('remaining_owned', [65867]),
                     lambda value: value['processes'][0].__setitem__('terminal', None),
                     lambda value: value.__setitem__('processes', [])]
        for index, mutation in enumerate(mutations):
            with self.subTest(case=index):
                output = self.root / ('run-' + str(index))
                args = SimpleNamespace(output=output, resume=False, stop_after=None, image=image,
                                       label='original', cts_args=[], data=self.root / 'data', port=5555,
                                       linux_run=self.shared / 'linux-run', timeout=1)
                def spawn(command, **kwargs):
                    if mutation is not None:
                        receipt = self.owner_receipt(command, 65866)
                        mutation(receipt)
                        Path(command[command.index('--receipt') + 1]).write_text(json.dumps(receipt))
                    return SimpleNamespace(pid=65866, wait=lambda **kw: 0)
                selector = Mock(side_effect=AssertionError('unproven cleanup must block XML selection'))
                with patch.object(pm, 'HARNESS', harness), \
                     patch.object(pm, 'campaign_provenance', return_value=self.provenance()), \
                     patch.object(pm.cts_runtime_bundle, 'snapshot', return_value=bundle), \
                     patch.object(pm.cts_runtime_bundle, 'validate', return_value=bundle), \
                     patch.object(pm, 'prepare'), patch.object(pm, 'revalidate_execution'), \
                     patch.object(pm.subprocess, 'Popen', side_effect=spawn), \
                     patch.object(pm, 'result_for', selector):
                    self.assertEqual(pm.run(args), 0)
                selector.assert_not_called()
                row = json.loads((output / 'campaign.json').read_text())['runs'][0]
                self.assertEqual(row['status'], 'not-run-complete')
                self.assertIn('error', row)
                self.assertNotIn('xml', row)


    def test_imported_recorded_xml_without_owner_proof_is_observed_not_accepted(self):
        original = self.campaign('original', ['pass'])
        native = self.campaign('native', ['pass'])
        receipt = native / '01-attempt.owner.json'
        receipt.unlink()
        report = pm.compare(original, native)
        self.assertEqual(report['counts']['native'], {'pass': 1})
        self.assertFalse(report['acceptance_pass'])
        self.assertEqual(report['native_incomplete_modules'], ['CtsFixture'])
        self.assertTrue(any('owner proof:' in issue for issue in report['native_wrapper_issues'][0]['issues']))

    def test_tampered_or_foreign_persisted_owner_proof_blocks_comparison(self):
        original = self.campaign('original', ['pass'])
        native = self.campaign('native', ['pass'])
        campaign_path = native / 'campaign.json'
        baseline = campaign_path.read_text()
        receipt_path = native / '01-attempt.owner.json'
        receipt_baseline = receipt_path.read_text()
        mutations = [
            lambda value: value['runs'][0].pop('owner_receipt_sha256'),
            lambda value: value['runs'][0]['invocation_scope'].__setitem__('wrapper_pid', 1),
            lambda value: value['runs'][0]['wrapper_command'].__setitem__(3, 'foreign-adb'),
            lambda value: value['runs'][0].__setitem__('wrapper_bundle_identity', 'foreign'),
            lambda value: value['runs'][0]['invocation_scope'].__setitem__('started_ms', 1791717548000),
        ]
        for index, mutation in enumerate(mutations):
            with self.subTest(case=index):
                campaign_path.write_text(baseline)
                self.update_campaign(native, mutation)
                report = pm.compare(original, native)
                self.assertEqual(report['counts']['native'], {'pass': 1})
                self.assertFalse(report['acceptance_pass'])
        campaign_path.write_text(baseline)
        receipt = json.loads(receipt_baseline)
        receipt['supervisor_pid'] = 1
        receipt_path.write_text(json.dumps(receipt))
        self.assertFalse(pm.compare(original, native)['acceptance_pass'], 'changed receipt hash')
        self.update_campaign(native, lambda value: value['runs'][0].__setitem__('owner_receipt_sha256', pm.digest(receipt_path)))
        self.assertFalse(pm.compare(original, native)['acceptance_pass'], 'foreign receipt even with refreshed hash')

    def test_current_tool_change_invalidates_persisted_owner_proof(self):
        original = self.campaign('original', ['pass'])
        native = self.campaign('native', ['pass'])
        Path(self.tools['owner']['path']).write_text('changed helper bytes')
        report = pm.compare(original, native)
        self.assertFalse(report['acceptance_pass'])
        self.assertTrue(any('current selected executable differs: owner' in issue
                            for issue in report['native_wrapper_issues'][0]['issues']))

    def test_resume_demotes_unproven_owner_row_and_preserves_observed_xml(self):
        directory = self.campaign('original', ['pass'])
        path = directory / 'campaign.json'
        baseline = json.loads(path.read_text())
        provenance = {key: value for key, value in baseline.items() if key != 'runs'}
        args = SimpleNamespace(output=directory, port=5555, cts_args=[], host_tool_selection=baseline['cts_host_tools'])
        with patch.object(pm.cts_runtime_bundle, 'validate'):
            good = pm.resume_state(args, provenance)
            self.assertEqual(good['runs'][0]['status'], 'recorded')
            (directory / '01-attempt.owner.json').unlink()
            state = pm.resume_state(args, provenance)
        row = state['runs'][0]
        self.assertEqual(row['status'], 'not-run-complete')
        self.assertIn('owner proof:', row['error'])
        self.assertEqual(row['xml_sha256'], baseline['runs'][0]['xml_sha256'])
        self.assertTrue(Path(row['xml']).is_file())


if __name__ == '__main__':
    unittest.main()
