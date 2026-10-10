"""Official CTS XML evidence remains visible without relaxing acceptance."""
import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('cts_pm', ROOT / 'tools/cts-pm.py')
pm = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pm)


class ComparisonEvidence(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.manifest = self.root / 'modules.json'
        self.manifest.write_text(json.dumps({'modules': [{'name': 'CtsFixture', 'kind': 'host'}]}))
        self.pin = patch.object(pm, 'MANIFEST', self.manifest)
        self.pin.start()
        self.addCleanup(self.pin.stop)
        self.addCleanup(self.temp.cleanup)

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
        campaign = {'label': label, 'modules': [m['name'] for m in pm.modules()], 'port': 5555, 'cts_args': [],
                    'manifest_sha256': pm.digest(self.manifest), 'runs': [row]}
        if selection is not None: campaign['cts_host_tools'] = selection
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


if __name__ == '__main__':
    unittest.main()
